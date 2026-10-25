use super::*;
use std::{
    os::unix::fs::{PermissionsExt, symlink},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
    home: File,
    account: CaptureAccount,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryogami-library-paths-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let uid = rustix::process::geteuid().as_raw();
        let gid = rustix::process::getegid().as_raw();
        let account = CaptureAccount {
            uid,
            gid,
            principal: ryoku_recovery::AccountPrincipal::from_account(
                uid,
                gid,
                "fixture",
                path.to_str().unwrap(),
            )
            .unwrap(),
        };
        let home = File::open(&path).unwrap();
        Self { path, home, account }
    }
    fn settings(&self, value: serde_json::Value) {
        std::fs::create_dir_all(self.path.join(".config/ryoku")).unwrap();
        let path = self.path.join(".config/ryoku/ryogami.json");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn resolve(&self, pictures: Option<&str>) -> Result<LibraryRoots> {
        resolve(&self.home, &self.account, pictures)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}

#[test]
fn library_roots_preserve_default_localized_english_fallback_and_livewall_selection() {
    let fixture = Fixture::new();
    let default = fixture.resolve(None).unwrap();
    assert_eq!(default.images().relative().as_str(), "Pictures/Wallpapers");
    assert_eq!(default.videos().relative().as_str(), "Pictures/Wallpapers");
    assert!(!default.images().exists());
    let localized = fixture.path.join("Bilder");
    let localized = localized.to_str().unwrap();
    let missing = fixture.resolve(Some(localized)).unwrap();
    assert_eq!(missing.images().relative().as_str(), "Bilder/Wallpapers");
    std::fs::create_dir_all(fixture.path.join("Pictures/Wallpapers")).unwrap();
    let legacy = fixture.resolve(Some(localized)).unwrap();
    assert_eq!(legacy.images().relative().as_str(), "Pictures/Wallpapers");
    std::fs::create_dir_all(fixture.path.join("Bilder/Wallpapers")).unwrap();
    std::fs::create_dir_all(fixture.path.join("Pictures/livewalls")).unwrap();
    let actual = fixture.resolve(Some(localized)).unwrap();
    assert_eq!(actual.images().relative().as_str(), "Bilder/Wallpapers");
    assert_eq!(actual.videos().relative().as_str(), "Pictures/livewalls");
    assert_ne!(actual.input_digest(), legacy.input_digest());
    assert!(actual.images().exists());
}

#[test]
fn configured_owned_roots_preserve_credentials_and_bind_exact_asset_capture() {
    let fixture = Fixture::new();
    fixture.settings(serde_json::json!({"paths":{"wallpaper":"~/.wallpapers","videoWallpaper":"~/Clips"},"sources":{"pexels":{"apiKey":"private-fixture-key"}}}));
    std::fs::create_dir_all(fixture.path.join(".wallpapers")).unwrap();
    std::fs::write(fixture.path.join(".wallpapers/fixture.png"), b"synthetic fixture").unwrap();
    std::fs::write(fixture.path.join("unrelated-secret"), b"private").unwrap();
    let before = std::fs::read(fixture.path.join(".config/ryoku/ryogami.json")).unwrap();
    let roots = fixture.resolve(None).unwrap();
    assert_eq!(roots.images().relative().as_str(), ".wallpapers");
    assert_eq!(roots.videos().relative().as_str(), "Clips");
    let asset = roots.images().asset(RelativePath::new("fixture.png").unwrap()).unwrap();
    assert_eq!(asset.home_relative().unwrap().as_str(), ".wallpapers/fixture.png");
    let captured = ryoku_recovery::ryogami_assets::capture_assets(
        &fixture.home,
        &fixture.account,
        std::slice::from_ref(&asset),
        Default::default(),
    )
    .unwrap();
    assert_eq!(captured.inventory().entries().len(), 1);
    let stage = asset
        .with_relative(RelativePath::new(format!(".part-{}", "a".repeat(64))).unwrap())
        .unwrap();
    assert!(stage.home_relative().unwrap().as_str().starts_with(".wallpapers/.part-"));
    let exported = serde_json::to_string(&asset).unwrap();
    assert!(!exported.contains("private-fixture-key"));
    assert!(!roots.input_digest().contains("private-fixture-key"));
    assert_eq!(std::fs::read(fixture.path.join(".config/ryoku/ryogami.json")).unwrap(), before);
    assert_eq!(std::fs::read(fixture.path.join("unrelated-secret")).unwrap(), b"private");
    assert!(!fixture.path.join("Clips").exists(), "resolution cannot create directories");
}

#[test]
fn external_credential_traversal_link_owner_and_malformed_roots_refuse_without_writes() {
    let fixture = Fixture::new();
    for root in [
        "/etc",
        "~/.ssh",
        "~/.config",
        "~/.local/state",
        "~/Pictures/../private",
        "~",
        "relative/path",
    ] {
        fixture.settings(serde_json::json!({"paths":{"wallpaper":root}}));
        assert!(fixture.resolve(None).is_err(), "{root}");
    }
    fixture.settings(serde_json::json!({"paths":{"wallpaper":42}}));
    assert!(fixture.resolve(None).is_err());
    fixture.settings(serde_json::json!({"paths":{"wallpaper":"~/Linked"}}));
    symlink("/tmp", fixture.path.join("Linked")).unwrap();
    assert!(fixture.resolve(None).is_err());
    std::fs::remove_file(fixture.path.join("Linked")).unwrap();
    std::fs::create_dir(fixture.path.join("Linked")).unwrap();
    std::fs::set_permissions(fixture.path.join("Linked"), std::fs::Permissions::from_mode(0o777))
        .unwrap();
    assert!(fixture.resolve(None).is_err());
    let mut wrong = fixture.account.clone();
    wrong.uid += 1;
    assert!(resolve(&fixture.home, &wrong, None).is_err());
    assert!(!fixture.path.join("Pictures").exists());
}

#[test]
fn custom_cache_is_reported_without_weakening_fixed_capture_or_touching_config() {
    let fixture = Fixture::new();
    fixture.settings(serde_json::json!({"paths":{"cache":"~/CustomCache"}}));
    let before = std::fs::read(fixture.path.join(".config/ryoku/ryogami.json")).unwrap();
    assert!(fixture.resolve(None).unwrap().relocated_cache());
    assert_eq!(std::fs::read(fixture.path.join(".config/ryoku/ryogami.json")).unwrap(), before);
    assert!(!fixture.path.join("CustomCache").exists());
    fixture.settings(serde_json::json!({"paths":{"cache":"~/.cache/ryogami"}}));
    assert!(!fixture.resolve(None).unwrap().relocated_cache());
}
