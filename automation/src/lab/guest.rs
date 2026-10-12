use crate::definition::scalar;
use crate::support::{Result, require};
use std::{fs, process::Command, time::Duration};

const MARKER: &str = "/etc/ryoku-lab-owned-marker";
const EXPECTED: &str = "verified fixture state\n";

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
    // Preserve repository signatures and TLS; avoid redirects to unknown mirrors.
    for entry in fs::read_dir("/etc/zypp/repos.d")? {
        let path = entry?.path();
        if path.extension().is_some_and(|x| x == "repo") {
            let repo = fs::read_to_string(&path)?;
            fs::write(
                path,
                repo.replace(
                    "http://download.opensuse.org/",
                    "https://downloadcontent2.opensuse.org/",
                )
                .replace(
                    "https://download.opensuse.org/",
                    "https://downloadcontent2.opensuse.org/",
                ),
            )?;
        }
    }
    command("update-ca-certificates", &[])?;
    for args in [
        vec!["--non-interactive", "refresh"],
        vec![
            "--non-interactive",
            "install",
            "--no-recommends",
            "snapper",
            "grub2-snapper-plugin",
            "plasma6-session",
            "sddm",
        ],
    ] {
        let result = Command::new("zypper")
            .args(args)
            .env("HTTP_PROXY", format!("http://{proxy}"))
            .env("HTTPS_PROXY", format!("http://{proxy}"))
            .output()?;
        if !result.status.success() {
            eprintln!(
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return Err(format!("KDE provisioning: zypper exited {}", result.status).into());
        }
    }
    if !std::path::Path::new("/etc/snapper/configs/root").exists() {
        command("snapper", &["-c", "root", "create-config", "/"])?;
    }
    command("systemctl", &["set-default", "graphical.target"])?;
    command("systemctl", &["enable", "--now", "display-manager"])?;
    println!("setup=passed");
    inspect()
}

pub fn inspect() -> Result<()> {
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
    require(!std::path::Path::new(MARKER).exists(), "refuse to overwrite an existing guest file")?;
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
    println!(
        "egress=unreachable\ncontrolled_corruption=observed\noffline_snapper_restore=matched\nfixture_marker_cleanup=passed\nreset_baseline=derived-btrfs-snapper"
    );
    Ok(())
}

pub fn marker_absent() -> Result<()> {
    fixture_only()?;
    let filesystem = command("findmnt", &["-n", "-o", "FSTYPE", "--target", "/"])?;
    require(
        filesystem.trim().eq_ignore_ascii_case(scalar("source_filesystem")?),
        "reset did not return to signed source filesystem",
    )?;
    require(
        !std::path::Path::new("/etc/snapper/configs/root").exists(),
        "reset retained derived Snapper configuration",
    )?;
    require(!std::path::Path::new(MARKER).exists(), "reset retained marker")?;
    let package =
        Command::new("rpm").args(["-q", "plasma6-session"]).env("LC_ALL", "C").output()?;
    require(
        package.status.code() == Some(1)
            && package.stdout == b"package plasma6-session is not installed\n",
        "reset package absence was not established",
    )?;
    println!("reset_marker=absent\nreset_package=absent");
    println!("reset_root={}\nreset_snapper_config=absent", filesystem.trim());
    Ok(())
}
