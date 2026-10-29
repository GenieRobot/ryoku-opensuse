//! Fixed-key GIO adapter. Run only in the separately bounded account worker;
//! the supplied guard must authenticate its exact root-owned dispatch, session
//! service and writer hold. This adapter is not a launcher or approval source.
#![allow(unsafe_code)] // Narrow dynamically loaded GIO ABI, checked handles and explicit ownership.
use crate::theme_settings::{
    ThemeSettingChange, ThemeSettingKey, ThemeSettingState, ThemeSettingsActor,
    ThemeSettingsBinding,
};
#[cfg(test)]
use std::path::Path;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    fs::File,
    io,
    os::unix::fs::MetadataExt,
    ptr::NonNull,
};
type Ptr = *mut c_void;
type KeyGet = unsafe extern "C" fn(Ptr, *const c_char) -> Ptr;
struct Api {
    handle: NonNull<c_void>,
    _image: File,
    source_default: unsafe extern "C" fn() -> Ptr,
    #[cfg(test)]
    source_directory: unsafe extern "C" fn(*const c_char, Ptr, i32, *mut Ptr) -> Ptr,
    source_lookup: unsafe extern "C" fn(Ptr, *const c_char, i32) -> Ptr,
    #[cfg(test)]
    source_unref: unsafe extern "C" fn(Ptr),
    schema_has_key: unsafe extern "C" fn(Ptr, *const c_char) -> i32,
    schema_unref: unsafe extern "C" fn(Ptr),
    new_full: unsafe extern "C" fn(Ptr, Ptr, *const c_char) -> Ptr,
    object_unref: unsafe extern "C" fn(Ptr),
    get: KeyGet,
    get_user: KeyGet,
    get_default: KeyGet,
    writable: unsafe extern "C" fn(Ptr, *const c_char) -> i32,
    range_check: unsafe extern "C" fn(Ptr, *const c_char, Ptr) -> i32,
    set: unsafe extern "C" fn(Ptr, *const c_char, Ptr) -> i32,
    reset: unsafe extern "C" fn(Ptr, *const c_char),
    sync: unsafe extern "C" fn(),
    variant_new_string: unsafe extern "C" fn(*const c_char) -> Ptr,
    variant_ref_sink: unsafe extern "C" fn(Ptr) -> Ptr,
    variant_type: unsafe extern "C" fn(Ptr) -> *const c_char,
    variant_string: unsafe extern "C" fn(Ptr, *mut usize) -> *const c_char,
    variant_unref: unsafe extern "C" fn(Ptr),
    #[cfg(test)]
    error_free: unsafe extern "C" fn(Ptr),
    #[cfg(test)]
    memory_backend: unsafe extern "C" fn() -> Ptr,
}
impl Api {
    fn open() -> io::Result<Self> {
        let path = [
            "/usr/lib64/libgio-2.0.so.0",
            "/lib64/libgio-2.0.so.0",
            "/usr/lib/x86_64-linux-gnu/libgio-2.0.so.0",
            "/lib/x86_64-linux-gnu/libgio-2.0.so.0",
        ]
        .iter()
        .find_map(|path| std::fs::canonicalize(path).ok())
        .ok_or_else(|| io::Error::other("reviewed GIO runtime unavailable"))?;
        let image = File::from(rustix::fs::open(
            &path,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )?);
        let metadata = image.metadata()?;
        if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o6022 != 0 {
            return Err(io::Error::other(
                "GIO library must be a root-owned immutable regular image",
            ));
        }
        let path = CString::new(path.as_os_str().as_encoded_bytes())?;
        // SAFETY: fixed trusted root-owned library image; no caller library path.
        let handle = NonNull::new(unsafe {
            libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL | libc::RTLD_NODELETE)
        })
        .ok_or_else(|| io::Error::other("GIO runtime could not be loaded"))?;
        macro_rules! symbol {
            ($name:literal,$ty:ty) => {{
                // SAFETY: symbol names and signatures are the stable public GLib/GIO ABI.
                let pointer =
                    unsafe { libc::dlsym(handle.as_ptr(), concat!($name, "\0").as_ptr().cast()) };
                if pointer.is_null() {
                    // SAFETY: no API object was returned; close our sole library handle.
                    unsafe {
                        libc::dlclose(handle.as_ptr());
                    }
                    return Err(io::Error::other(concat!("GIO ABI symbol unavailable: ", $name)));
                }
                // SAFETY: checked non-null symbol with its exact documented C ABI type.
                unsafe { std::mem::transmute::<Ptr, $ty>(pointer) }
            }};
        }
        Ok(Self {
            handle,
            _image: image,
            source_default: symbol!(
                "g_settings_schema_source_get_default",
                unsafe extern "C" fn() -> Ptr
            ),
            #[cfg(test)]
            source_directory: symbol!(
                "g_settings_schema_source_new_from_directory",
                unsafe extern "C" fn(*const c_char, Ptr, i32, *mut Ptr) -> Ptr
            ),
            source_lookup: symbol!(
                "g_settings_schema_source_lookup",
                unsafe extern "C" fn(Ptr, *const c_char, i32) -> Ptr
            ),
            #[cfg(test)]
            source_unref: symbol!("g_settings_schema_source_unref", unsafe extern "C" fn(Ptr)),
            schema_has_key: symbol!(
                "g_settings_schema_has_key",
                unsafe extern "C" fn(Ptr, *const c_char) -> i32
            ),
            schema_unref: symbol!("g_settings_schema_unref", unsafe extern "C" fn(Ptr)),
            new_full: symbol!(
                "g_settings_new_full",
                unsafe extern "C" fn(Ptr, Ptr, *const c_char) -> Ptr
            ),
            object_unref: symbol!("g_object_unref", unsafe extern "C" fn(Ptr)),
            get: symbol!("g_settings_get_value", KeyGet),
            get_user: symbol!("g_settings_get_user_value", KeyGet),
            get_default: symbol!("g_settings_get_default_value", KeyGet),
            writable: symbol!(
                "g_settings_is_writable",
                unsafe extern "C" fn(Ptr, *const c_char) -> i32
            ),
            range_check: symbol!(
                "g_settings_range_check",
                unsafe extern "C" fn(Ptr, *const c_char, Ptr) -> i32
            ),
            set: symbol!(
                "g_settings_set_value",
                unsafe extern "C" fn(Ptr, *const c_char, Ptr) -> i32
            ),
            reset: symbol!("g_settings_reset", unsafe extern "C" fn(Ptr, *const c_char)),
            sync: symbol!("g_settings_sync", unsafe extern "C" fn()),
            variant_new_string: symbol!(
                "g_variant_new_string",
                unsafe extern "C" fn(*const c_char) -> Ptr
            ),
            variant_ref_sink: symbol!("g_variant_ref_sink", unsafe extern "C" fn(Ptr) -> Ptr),
            variant_type: symbol!(
                "g_variant_get_type_string",
                unsafe extern "C" fn(Ptr) -> *const c_char
            ),
            variant_string: symbol!(
                "g_variant_get_string",
                unsafe extern "C" fn(Ptr, *mut usize) -> *const c_char
            ),
            variant_unref: symbol!("g_variant_unref", unsafe extern "C" fn(Ptr)),
            #[cfg(test)]
            error_free: symbol!("g_error_free", unsafe extern "C" fn(Ptr)),
            #[cfg(test)]
            memory_backend: symbol!("g_memory_settings_backend_new", unsafe extern "C" fn() -> Ptr),
        })
    }
    fn string(&self, value: Ptr) -> io::Result<String> {
        let value = NonNull::new(value)
            .ok_or_else(|| io::Error::other("missing GSettings string value"))?;
        // SAFETY: GIO getters return owned valid variants; type is checked before string access.
        let result = unsafe {
            let kind = (self.variant_type)(value.as_ptr());
            if kind.is_null() || CStr::from_ptr(kind).to_bytes() != b"s" {
                Err(io::Error::other("GSettings key is not the approved string type"))
            } else {
                let mut length = 0;
                let pointer = (self.variant_string)(value.as_ptr(), &mut length);
                if pointer.is_null() || length > 4096 {
                    Err(io::Error::other("GSettings value exceeds its bound"))
                } else {
                    std::str::from_utf8(std::slice::from_raw_parts(pointer.cast::<u8>(), length))
                        .map(str::to_string)
                        .map_err(io::Error::other)
                }
            }
        };
        // SAFETY: consume exactly the getter's owned reference, including all error paths.
        unsafe {
            (self.variant_unref)(value.as_ptr());
        }
        result
    }
    fn with_variant<T>(
        &self,
        text: &str,
        call: impl FnOnce(Ptr) -> io::Result<T>,
    ) -> io::Result<T> {
        if text.len() > 4096 {
            return Err(io::Error::other("oversized GSettings string"));
        }
        let text = CString::new(text)?;
        // SAFETY: valid NUL-terminated UTF-8; sink floating reference for explicit ownership.
        let variant = unsafe { (self.variant_ref_sink)((self.variant_new_string)(text.as_ptr())) };
        if variant.is_null() {
            return Err(io::Error::other("GVariant allocation failed"));
        }
        let result = call(variant);
        // SAFETY: release our full reference after the synchronous setter/check returns.
        unsafe {
            (self.variant_unref)(variant);
        }
        result
    }
}
impl Drop for Api {
    fn drop(&mut self) {
        // SAFETY: this API drops after its settings objects have been unreferenced.
        unsafe {
            libc::dlclose(self.handle.as_ptr());
        }
    }
}

pub struct GioThemeSettingsActor {
    api: Api,
    settings: NonNull<c_void>,
    expected: ThemeSettingsBinding,
    guard: Box<dyn FnMut() -> io::Result<ThemeSettingsBinding>>,
}
impl GioThemeSettingsActor {
    /// The worker's guard must enforce the root dispatch and deadline. The
    /// worker must separately pin the complete runtime/schema/service identity
    /// and provide the exact authenticated session address before this call.
    pub fn for_authenticated_worker(
        expected: ThemeSettingsBinding,
        mut guard: Box<dyn FnMut() -> io::Result<ThemeSettingsBinding>>,
    ) -> io::Result<Self> {
        if guard()? != expected
            || rustix::process::geteuid().as_raw() != expected.uid
            || expected.uid == 0
        {
            return Err(io::Error::other("GSettings worker account/dispatch mismatch"));
        }
        if std::env::var("GSETTINGS_BACKEND").as_deref() != Ok("dconf") {
            return Err(io::Error::other(
                "GSettings worker requires its explicit reviewed dconf backend",
            ));
        }
        let address = format!("unix:path=/run/user/{}/bus", expected.uid);
        if std::env::var("DBUS_SESSION_BUS_ADDRESS").as_deref() != Ok(address.as_str())
            || [
                "GIO_EXTRA_MODULES",
                "GIO_MODULE_DIR",
                "GSETTINGS_SCHEMA_DIR",
                "LD_PRELOAD",
                "LD_LIBRARY_PATH",
                "DBUS_STARTER_ADDRESS",
                "DBUS_STARTER_BUS_TYPE",
            ]
            .iter()
            .any(|name| std::env::var_os(name).is_some())
        {
            return Err(io::Error::other(
                "GSettings worker session address/environment differs from its fixed policy",
            ));
        }
        let api = Api::open()?;
        // SAFETY: borrowed process-default schema source lives for the GIO runtime lifetime.
        let source = unsafe { (api.source_default)() };
        let settings = Self::new_settings(&api, source, std::ptr::null_mut())?;
        Ok(Self { api, settings, expected, guard })
    }
    fn new_settings(api: &Api, source: Ptr, backend: Ptr) -> io::Result<NonNull<c_void>> {
        if source.is_null() {
            return Err(io::Error::other("approved desktop-interface schema unavailable"));
        }
        // SAFETY: fixed schema name; nonrecursive lookup and checked source handle.
        let schema =
            unsafe { (api.source_lookup)(source, c"org.gnome.desktop.interface".as_ptr(), 0) };
        if schema.is_null() {
            return Err(io::Error::other("approved desktop-interface schema unavailable"));
        }
        for key in [
            ThemeSettingKey::ColorScheme,
            ThemeSettingKey::AccentColor,
            ThemeSettingKey::GtkTheme,
            ThemeSettingKey::IconTheme,
        ] {
            let name = Self::key(key);
            // SAFETY: checked schema handle and fixed typed key; avoid fatal GIO lookup on a missing key.
            if unsafe { (api.schema_has_key)(schema, name.as_ptr()) } == 0 {
                unsafe {
                    (api.schema_unref)(schema);
                }
                return Err(io::Error::other("approved theme key unavailable in pinned schema"));
            }
        }
        // SAFETY: owned looked-up schema; constructor refs it, backend and path are valid.
        let settings = unsafe { (api.new_full)(schema, backend, std::ptr::null()) };
        // SAFETY: release the lookup reference; settings retains its own schema reference.
        unsafe {
            (api.schema_unref)(schema);
        }
        NonNull::new(settings).ok_or_else(|| io::Error::other("GSettings object unavailable"))
    }
    fn check(&mut self) -> io::Result<()> {
        if (self.guard)()? != self.expected {
            return Err(io::Error::other("GSettings worker/service identity changed"));
        }
        Ok(())
    }
    fn key(key: ThemeSettingKey) -> CString {
        CString::new(key.name()).expect("fixed key has no NUL")
    }
    fn validate_string(&self, key: &CStr, value: &str) -> io::Result<()> {
        self.api.with_variant(value, |variant| {
            // SAFETY: settings/key/value live through the synchronous schema range check.
            if unsafe { (self.api.range_check)(self.settings.as_ptr(), key.as_ptr(), variant) } == 0
            {
                Err(io::Error::other("desired setting is outside the approved schema range"))
            } else {
                Ok(())
            }
        })
    }
    #[cfg(test)]
    fn memory(directory: &Path, expected: ThemeSettingsBinding) -> io::Result<Self> {
        let api = Api::open()?;
        let directory = CString::new(directory.as_os_str().as_encoded_bytes())?;
        let mut error = std::ptr::null_mut();
        // SAFETY: isolated test directory is a compiled schema fixture, trusted=false validates it.
        let source = unsafe {
            (api.source_directory)(directory.as_ptr(), std::ptr::null_mut(), 0, &mut error)
        };
        if source.is_null() {
            if !error.is_null() {
                unsafe {
                    (api.error_free)(error);
                }
            }
            return Err(io::Error::other("test schema could not be opened"));
        }
        // SAFETY: explicit private memory backend never contacts the desktop bus or dconf database.
        let backend = unsafe { (api.memory_backend)() };
        let settings = Self::new_settings(&api, source, backend);
        // SAFETY: new_settings retains backend/schema references when successful.
        unsafe {
            (api.source_unref)(source);
            (api.object_unref)(backend);
        }
        let settings = settings?;
        let identity = expected.clone();
        Ok(Self { api, settings, expected, guard: Box::new(move || Ok(identity.clone())) })
    }
}
impl ThemeSettingsActor for GioThemeSettingsActor {
    fn binding(&mut self) -> io::Result<ThemeSettingsBinding> {
        self.check()?;
        Ok(self.expected.clone())
    }
    fn check_lifetime(&mut self) -> io::Result<()> {
        self.check()
    }
    fn read(&mut self, key: ThemeSettingKey) -> io::Result<ThemeSettingState> {
        self.check()?;
        let key = Self::key(key);
        // SAFETY: fixed typed string keys on the retained schema/settings object.
        let user = unsafe { (self.api.get_user)(self.settings.as_ptr(), key.as_ptr()) };
        let user_value = if user.is_null() { None } else { Some(self.api.string(user)?) };
        let effective_value =
            self.api.string(unsafe { (self.api.get)(self.settings.as_ptr(), key.as_ptr()) })?;
        let default_value = self
            .api
            .string(unsafe { (self.api.get_default)(self.settings.as_ptr(), key.as_ptr()) })?;
        let writable = unsafe { (self.api.writable)(self.settings.as_ptr(), key.as_ptr()) } != 0;
        self.check()?;
        Ok(ThemeSettingState { user_value, effective_value, default_value, writable })
    }
    fn validate_change(
        &mut self,
        key: ThemeSettingKey,
        change: &ThemeSettingChange,
    ) -> io::Result<()> {
        self.check()?;
        let key = Self::key(key);
        match change {
            ThemeSettingChange::Reset => Ok(()),
            ThemeSettingChange::Set(value) => self.validate_string(&key, value),
            ThemeSettingChange::NudgeGtk(value) => {
                self.validate_string(
                    &key,
                    if value == "Adwaita" { "Adwaita-dark" } else { "Adwaita" },
                )?;
                self.validate_string(&key, value)
            }
        }
    }
    fn replace_user_value(&mut self, key: ThemeSettingKey, value: Option<&str>) -> io::Result<()> {
        self.check()?;
        let key = Self::key(key);
        match value {
            None => {
                unsafe {
                    (self.api.reset)(self.settings.as_ptr(), key.as_ptr());
                }
                Ok(())
            }
            Some(value) => self.api.with_variant(value, |variant| {
                // SAFETY: fixed key and checked string value; no command or GVariant text parsing.
                if unsafe { (self.api.set)(self.settings.as_ptr(), key.as_ptr(), variant) } == 0 {
                    Err(io::Error::other("GSettings backend refused the exact value"))
                } else {
                    Ok(())
                }
            }),
        }
    }
    fn synchronize(&mut self) -> io::Result<()> {
        self.check()?;
        // SAFETY: must run in the deadline-controlled worker, never the root coordinator thread.
        unsafe {
            (self.api.sync)();
        }
        self.check()
    }
}
impl Drop for GioThemeSettingsActor {
    fn drop(&mut self) {
        // SAFETY: release settings before Api and its dlopen handle drop.
        unsafe {
            (self.api.object_unref)(self.settings.as_ptr());
        }
    }
}

#[cfg(test)]
#[path = "theme_gio_tests.rs"]
mod tests;
