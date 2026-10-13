use crate::definition::scalar;
use crate::process;
use crate::support::{Result, require};
use std::{fs, io::Write, path::Path, process::Command, time::Duration};

const MARKER: &str = "/etc/ryoku-lab-owned-marker";
const EXPECTED: &str = "verified fixture state\n";
const RESET_SENTINEL: &str = "/etc/ryoku-lab-reset-sentinel";
const RESET_STATE: &[u8] = b"retained fixture reset state\n";
const DISPLAY_MANAGER_ALIAS: &str = "/etc/systemd/system/display-manager.service";
const LEGACY_DISPLAY_MANAGER: &str = "/usr/lib/systemd/system/display-manager-legacy.service";
const SDDM_UNIT: &str = "/usr/lib/systemd/system/sddm.service";
const ZYPPER_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const ZYPPER_OUTPUT_LIMIT: usize = 1024 * 1024;

fn require_absent(path: impl AsRef<std::path::Path>, message: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(message.into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn fixture_only() -> Result<()> {
    require(
        fs::read_to_string("/etc/ryoku-lab-fixture")? == "disposable-issue13\n",
        "not an explicitly seeded disposable lab guest",
    )?;
    require(
        fs::read_to_string("/sys/class/dmi/id/sys_vendor")?.trim() == "QEMU",
        "not a QEMU guest",
    )
}

pub(crate) fn command(program: &str, args: &[&str]) -> Result<String> {
    let result = Command::new(program).args(args).output()?;
    if !result.status.success() {
        eprintln!(
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return Err(format!("{program} exited {}", result.status).into());
    }
    Ok(String::from_utf8(result.stdout)?)
}

pub fn setup(proxy: &str) -> Result<()> {
    base_setup()?;
    fixture_only()?;
    let _: std::net::SocketAddr = proxy.parse()?;
    require(
        command("findmnt", &["-n", "-o", "FSTYPE", "--target", "/"])?.trim() == "btrfs",
        "fixture root must be Btrfs",
    )?;
    crate::repositories::pin()?;
    command("update-ca-certificates", &[])?;
    for args in [
        vec!["--non-interactive", "refresh", "--force"],
        vec![
            "--non-interactive",
            "--no-refresh",
            "install",
            "--no-recommends",
            "snapper",
            "grub2-snapper-plugin",
            "plasma6-session",
            "sddm",
            "xorg-x11-server",
        ],
    ] {
        let mut zypper = Command::new("zypper");
        zypper
            .args(&args)
            .env("HTTP_PROXY", format!("http://{proxy}"))
            .env("HTTPS_PROXY", format!("http://{proxy}"));
        let result = process::run(&mut zypper, ZYPPER_TIMEOUT, ZYPPER_OUTPUT_LIMIT)?;
        if result.timed_out {
            return Err("KDE_PROVISIONING_TIMEOUT".into());
        }
        if !result.status.success() {
            eprintln!(
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return Err(format!("KDE provisioning: zypper exited {}", result.status).into());
        }
        if args[1] == "refresh" {
            crate::repositories::verify_cache()?;
        }
    }
    if !std::path::Path::new("/etc/snapper/configs/root").exists() {
        command("snapper", &["-c", "root", "create-config", "/"])?;
    }
    command("systemctl", &["set-default", "graphical.target"])?;
    select_sddm_display_manager()?;
    println!("setup=passed");
    inspect_provisioned()
}

fn missing_runtime_dependencies(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter(|line| line.contains("=> not found"))
        .filter_map(|line| line.split_whitespace().next())
        .collect()
}

/// Check the supplied proof binary against the Tumbleweed guest's dynamic
/// loader after installing the GTK runtime.
fn verify_ui_runtime(binary: &Path) -> Result<()> {
    let result = Command::new("/usr/bin/ldd").arg(binary).output()?;
    require(
        result.status.success(),
        "GTK proof binary is incompatible with this Linux runtime; ldd could not inspect it",
    )?;
    let dependencies = String::from_utf8(result.stdout)?;
    let missing = missing_runtime_dependencies(&dependencies);
    require(
        missing.is_empty(),
        &format!(
            "GTK proof binary cannot run on Tumbleweed; missing runtime libraries: {}",
            missing.join(", ")
        ),
    )?;
    println!("ui_runtime_dependencies=resolved");
    Ok(())
}

/// Install GTK only when the opt-in visual proof is requested. Normal #13
/// runs keep the original KDE-only package set.
pub fn install_ui_runtime() -> Result<()> {
    fixture_only()?;
    require(
        command("findmnt", &["-n", "-o", "FSTYPE", "--target", "/"])?.trim() == "btrfs",
        "fixture root must be Btrfs",
    )?;
    let result = Command::new("zypper")
        .args(["--non-interactive", "--no-refresh", "install", "--no-recommends", "libgtk-4-1"])
        .output()?;
    if !result.status.success() {
        return Err("optional GTK runtime installation failed; inspect private guest log".into());
    }
    command("rpm", &["-q", "libgtk-4-1"])?;
    verify_ui_runtime(Path::new("/tmp/ryoku-ui-gtk.upload"))?;
    command(
        "install",
        &["-Z", "-m", "0755", "/tmp/ryoku-ui-gtk.upload", "/usr/local/bin/ryoku-ui-gtk"],
    )?;
    command("restorecon", &["-F", "/usr/local/bin/ryoku-ui-gtk"])?;
    command("rm", &["-f", "/tmp/ryoku-ui-gtk.upload"])?;
    println!("ui_runtime=installed");
    Ok(())
}

fn select_sddm_display_manager() -> Result<()> {
    replace_legacy_display_manager_alias(
        DISPLAY_MANAGER_ALIAS.as_ref(),
        LEGACY_DISPLAY_MANAGER.as_ref(),
        SDDM_UNIT.as_ref(),
    )?;
    command("systemctl", &["daemon-reload"])?;
    // Enable the canonical unit; `display-manager.service` is a systemd alias.
    command("systemctl", &["enable", "--now", "sddm.service"])?;
    Ok(())
}

fn replace_legacy_display_manager_alias(
    alias: &std::path::Path,
    legacy_unit: &std::path::Path,
    sddm_unit: &std::path::Path,
) -> Result<()> {
    match fs::read_link(alias) {
        Ok(target) => {
            let resolved = if target.is_absolute() {
                target
            } else {
                alias.parent().ok_or("display-manager alias has no parent")?.join(target)
            }
            .canonicalize()?;
            let legacy = legacy_unit.canonicalize()?;
            let sddm = sddm_unit.canonicalize()?;
            if resolved == legacy {
                // The minimal appliance pre-enables its console fallback under
                // this alias. Replace only that exact symlink in this marked
                // disposable fixture; preserve and reject every other provider.
                fs::remove_file(alias)?;
            } else {
                require(resolved == sddm, "unexpected display-manager provider")?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub fn inspect() -> Result<()> {
    inspect_provisioned()?;
    crate::desktop::inspect()
}

fn inspect_provisioned() -> Result<()> {
    inspect_base()?;
    command("systemctl", &["is-active", "display-manager"])?;
    for package in ["plasma6-session", "sddm"] {
        let version = command("rpm", &["-q", "--qf", "%{VERSION}-%{RELEASE}", package])?;
        println!("{package}={}", version.trim());
    }
    println!("kde_display_manager=active");
    Ok(())
}

pub fn base_setup() -> Result<()> {
    fixture_only()?;
    command("matchpathcon", &["-V", "/etc/sysconfig/snapper"])?;
    require(
        command("findmnt", &["-n", "-o", "FSTYPE", "--target", "/"])?.trim() == "btrfs",
        "fixture root must be Btrfs",
    )?;
    if !std::path::Path::new("/etc/snapper/configs/root").exists() {
        command("snapper", &["-c", "root", "create-config", "/"])?;
    }
    inspect_base()
}

pub fn inspect_base() -> Result<()> {
    fixture_only()?;
    require(
        fs::read_to_string("/sys/fs/selinux/enforce")?.trim() == "1",
        "SELinux must remain enforcing",
    )?;
    command("matchpathcon", &["-V", "/etc/sysconfig/snapper", "/etc/snapper/configs/root"])?;
    let os = fs::read_to_string("/etc/os-release")?;
    require(
        os.lines().any(|x| x == "ID=\"opensuse-tumbleweed\"" || x == "ID=opensuse-tumbleweed"),
        "wrong guest OS",
    )?;
    let snapshot = scalar("snapshot")?;
    require(
        os.lines().any(|x| {
            x == format!("VERSION_ID=\"{snapshot}\"") || x == format!("VERSION_ID={snapshot}")
        }),
        "wrong guest snapshot",
    )?;
    require(command("uname", &["-m"])?.trim() == "x86_64", "wrong architecture")?;
    require(
        command("findmnt", &["-n", "-o", "FSTYPE", "--target", "/"])?.trim() == "btrfs",
        "non-Btrfs root",
    )?;
    require(std::path::Path::new("/sys/firmware/efi").exists(), "guest did not boot UEFI")?;
    require(std::path::Path::new("/boot/grub2/grub.cfg").exists(), "missing GRUB2 config")?;
    command("snapper", &["-c", "root", "list"])?;
    let running_kernel = command("uname", &["-r"])?;
    require(running_kernel.trim() == scalar("kernel_release")?, "unexpected running kernel")?;
    let kernel_image = format!("/usr/lib/modules/{}/vmlinuz", running_kernel.trim());
    let kernel_package = command("rpm", &["-qf", "--qf", "%{NAME}", &kernel_image])?;
    require(
        kernel_package == scalar("kernel_package")?,
        "running kernel image has unexpected RPM owner",
    )?;
    let kernel_version = command("rpm", &["-qf", "--qf", "%{VERSION}-%{RELEASE}", &kernel_image])?;
    require(
        kernel_version == scalar("kernel_package_version")?,
        "running kernel package version changed",
    )?;
    println!("kernel_running={}\n{kernel_package}={kernel_version}", running_kernel.trim());
    for package in ["grub2", "grub2-x86_64-efi", "snapper", "grub2-snapper-plugin"] {
        let version = command("rpm", &["-q", "--qf", "%{VERSION}-%{RELEASE}", package])?;
        if package.starts_with("grub2") {
            require(
                version.trim() == scalar("grub2_package_version")?,
                "GRUB2 fixture version changed",
            )?;
        }
        println!("{package}={}", version.trim());
    }
    println!("snapshot={snapshot}\narch=x86_64\nroot=btrfs\nboot=UEFI-GRUB2");
    println!("selinux=enforcing\nsnapper_config_label=verified\nsnapper_dbus_list=passed");
    Ok(())
}

pub fn recover(proxy_address: &str) -> Result<()> {
    fixture_only()?;
    // Offline phase retains loopback host-forwarded SSH, but blocks egress.
    let address: std::net::SocketAddr = proxy_address.parse()?;
    require(
        std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)).is_err(),
        "egress is reachable; offline recovery claim rejected",
    )?;
    require_absent(MARKER, "refuse to overwrite an existing guest file")?;
    fs::write(MARKER, EXPECTED)?;
    let number = command(
        "snapper",
        &[
            "-c",
            "root",
            "create",
            "--print-number",
            "--description",
            "ryoku disposable lab fixture",
        ],
    )?;
    let number = number.trim().parse::<u64>()?;
    fs::write(MARKER, "controlled corruption\n")?;
    require(fs::read_to_string(MARKER)? != EXPECTED, "controlled failure was not injected")?;
    command("snapper", &["-c", "root", "undochange", &format!("{number}..0"), MARKER])?;
    require(fs::read_to_string(MARKER)? == EXPECTED, "offline snapshot restoration mismatch")?;
    fs::remove_file(MARKER)?;
    command("snapper", &["-c", "root", "delete", &number.to_string()])?;
    require(
        std::path::Path::new("/etc/snapper/configs/root").exists(),
        "recovery lost Snapper config before disk reset",
    )?;
    require(
        command("findmnt", &["-n", "-o", "FSTYPE", "--target", "/"])?.trim() == "btrfs",
        "recovery root is not derived Btrfs",
    )?;
    // Keep a distinct, durable state change until the entire disk is discarded.
    // Exclusive creation refuses to overwrite any existing guest data.
    let mut sentinel = fs::OpenOptions::new().write(true).create_new(true).open(RESET_SENTINEL)?;
    sentinel.write_all(RESET_STATE)?;
    sentinel.sync_all()?;
    fs::File::open("/etc")?.sync_all()?;
    require(
        fs::read(RESET_SENTINEL)? == RESET_STATE,
        "reset sentinel contents were not established",
    )?;
    println!(
        "egress=unreachable\ncontrolled_corruption=observed\noffline_snapper_restore=matched\nfixture_marker_cleanup=passed\nreset_baseline=derived-btrfs-snapper\nreset_sentinel_before_discard=present\nreset_sentinel_bytes=matched\nreset_sentinel_sync=passed"
    );
    println!("reset_sentinel_length={}", RESET_STATE.len());
    Ok(())
}

pub fn marker_absent() -> Result<()> {
    fixture_only()?;
    let filesystem = command("findmnt", &["-n", "-o", "FSTYPE", "--target", "/"])?;
    require(
        filesystem.trim().eq_ignore_ascii_case(scalar("source_filesystem")?),
        "reset did not return to signed source filesystem",
    )?;
    require_absent("/etc/snapper/configs/root", "reset retained derived Snapper configuration")?;
    require_absent(RESET_SENTINEL, "reset retained the dedicated sentinel")?;
    require_absent(MARKER, "reset retained marker")?;
    let package =
        Command::new("rpm").args(["-q", "plasma6-session"]).env("LC_ALL", "C").output()?;
    require(
        package.status.code() == Some(1)
            && package.stdout == b"package plasma6-session is not installed\n",
        "reset package absence was not established",
    )?;
    println!("reset_marker=absent\nreset_package=absent");
    println!(
        "reset_root={}\nreset_snapper_config=absent\nreset_sentinel=absent",
        filesystem.trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_only_loader_dependencies_marked_not_found() {
        assert_eq!(
            missing_runtime_dependencies(
                "\tlibgtk-4.so.1 => /usr/lib64/libgtk-4.so.1 (0x1)\n\tlibmissing.so.9 => not found\n"
            ),
            ["libmissing.so.9"]
        );
        assert!(missing_runtime_dependencies("linux-vdso.so.1 (0x1)\n").is_empty());
    }

    #[test]
    fn absence_requires_not_found_and_rejects_dangling_links() {
        let unique =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory =
            std::env::temp_dir().join(format!("ryoku-reset-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("sentinel");
        assert!(require_absent(&path, "present").is_ok());
        fs::write(&path, RESET_STATE).unwrap();
        assert!(require_absent(&path, "present").is_err());
        fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(directory.join("missing-target"), &path).unwrap();
        assert!(require_absent(&path, "present").is_err());
        assert!(require_absent("invalid\0path", "present").is_err());
        fs::remove_file(&path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn replaces_only_the_exact_legacy_display_manager_symlink() {
        let unique =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir()
            .join(format!("ryoku-display-manager-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let legacy = directory.join("display-manager-legacy.service");
        let sddm = directory.join("sddm.service");
        let alias = directory.join("display-manager.service");
        fs::write(&legacy, "legacy").unwrap();
        fs::write(&sddm, "sddm").unwrap();
        std::os::unix::fs::symlink(&legacy, &alias).unwrap();
        replace_legacy_display_manager_alias(&alias, &legacy, &sddm).unwrap();
        assert!(require_absent(&alias, "legacy alias remains").is_ok());

        std::os::unix::fs::symlink(&sddm, &alias).unwrap();
        replace_legacy_display_manager_alias(&alias, &legacy, &sddm).unwrap();
        assert!(fs::read_link(&alias).is_ok());
        fs::remove_file(&alias).unwrap();

        let unexpected = directory.join("unexpected.service");
        fs::write(&unexpected, "unexpected").unwrap();
        std::os::unix::fs::symlink(&unexpected, &alias).unwrap();
        assert!(replace_legacy_display_manager_alias(&alias, &legacy, &sddm).is_err());
        assert!(fs::symlink_metadata(&alias).unwrap().file_type().is_symlink());
        fs::remove_file(alias).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }
}
