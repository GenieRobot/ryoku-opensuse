//! Explicit host/tool selection; guest acceptance remains shared.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    definition::scalar,
    support::{Result, output, require},
};

pub const WSL: bool = cfg!(feature = "wsl-lab");
pub const KVM: bool =
    cfg!(any(feature = "issue19-lab", feature = "issue20-lab", feature = "issue21-lab"));

pub fn tool(assets: &Path, binary: &str) -> Command {
    if WSL {
        Command::new(Path::new("/usr/bin").join(if binary == "genisoimage" {
            "mkisofs"
        } else {
            binary
        }))
    } else {
        let mut command = Command::new(assets.join("qemu/usr/bin").join(binary));
        command.env("LD_LIBRARY_PATH", assets.join("qemu/usr/lib/x86_64-linux-gnu"));
        command
    }
}

pub fn firmware(assets: &Path, name: &str) -> PathBuf {
    if WSL {
        Path::new("/usr/share/qemu").join(match name {
            "OVMF_CODE_4M.fd" => "ovmf-x86_64-4m-code.bin",
            "OVMF_VARS_4M.fd" => "ovmf-x86_64-4m-vars.bin",
            _ => unreachable!("firmware names are fixed private constants"),
        })
    } else {
        assets.join("qemu/usr/share/OVMF").join(name)
    }
}

pub fn data(assets: &Path) -> PathBuf {
    if WSL { PathBuf::from("/usr/share/qemu") } else { assets.join("qemu/usr/share/qemu") }
}

pub fn vgabios(assets: &Path) -> PathBuf {
    if WSL {
        PathBuf::from("/usr/share/qemu/vgabios-virtio.bin")
    } else {
        assets.join("qemu/usr/share/seabios/vgabios-virtio.bin")
    }
}

pub fn validate_host(assets: &Path, parent: &Path) -> Result<()> {
    let authorization = if WSL { "existing-wsl-vm" } else { "existing-cloud-workspace" };
    require(
        std::env::var("RYOKU_LAB_AUTHORIZED").as_deref() == Ok(authorization),
        "explicit runner authorization does not match the selected build feature",
    )?;
    if WSL {
        let package = output(Command::new("/usr/bin/rpm").args([
            "-q",
            "--qf",
            "%{NAME}-%{VERSION}-%{RELEASE}\\n",
            "qemu-x86",
        ]))?;
        require(package.trim() == scalar("qemu_package")?, "native QEMU RPM release changed")?;
        let bios = output(Command::new("/usr/bin/sha256sum").arg(vgabios(assets)))?;
        require(
            bios.split_whitespace().next() == Some(scalar("vgabios_sha256")?),
            "native VGA BIOS checksum changed",
        )?;
        require(!rustix::process::geteuid().is_root(), "run the WSL lab as an ordinary user")?;
        require(
            fs::read_to_string("/proc/sys/kernel/osrelease")?.to_ascii_lowercase().contains("wsl")
                && fs::read_to_string("/etc/os-release")?.lines().any(|line| {
                    line == "ID=\"opensuse-tumbleweed\"" || line == "ID=opensuse-tumbleweed"
                }),
            "WSL runner requires openSUSE Tumbleweed under WSL",
        )?;
        let workspace = fs::canonicalize(std::env::current_dir()?)?;
        let temporary = fs::canonicalize(std::env::temp_dir())?;
        require(
            (parent.starts_with(workspace) || parent.starts_with(temporary))
                && !parent.starts_with(assets),
            "use a fresh WSL workspace or temporary run directory separate from assets",
        )
    } else {
        require(
            parent.starts_with("/workspace") && !parent.starts_with(assets),
            "use separate authorized cloud workspace run directory",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_fixture_acceleration_matches_the_selected_host_policy() {
        assert_eq!(scalar("accelerator").unwrap(), if KVM { "kvm" } else { "tcg" });
    }

    #[test]
    fn native_runner_does_not_load_extracted_cloud_tools_or_libraries() {
        let assets = Path::new("/untrusted-assets");
        let command = tool(assets, "genisoimage");
        if WSL {
            assert_eq!(command.get_program(), "/usr/bin/mkisofs");
            assert_eq!(command.get_envs().count(), 0);
            assert_eq!(
                firmware(assets, "OVMF_CODE_4M.fd"),
                Path::new("/usr/share/qemu/ovmf-x86_64-4m-code.bin")
            );
            assert_eq!(data(assets), Path::new("/usr/share/qemu"));
        } else {
            assert_eq!(command.get_program(), "/untrusted-assets/qemu/usr/bin/genisoimage");
            assert!(command.get_envs().any(|(name, _)| name == "LD_LIBRARY_PATH"));
            assert_eq!(
                firmware(assets, "OVMF_CODE_4M.fd"),
                assets.join("qemu/usr/share/OVMF/OVMF_CODE_4M.fd")
            );
        }
    }
}
