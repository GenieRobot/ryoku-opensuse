//! Explicit acceleration; capability inspection never creates a VM.
use crate::support::{Result, require};
use rustix::ioctl::{Ioctl, IoctlOutput, Opcode, ioctl, opcode};
use std::{ffi::c_void, fs, os::unix::fs::OpenOptionsExt};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Accelerator {
    #[default]
    Tcg,
    Kvm,
}

impl Accelerator {
    pub(crate) fn parse(value: &str, wsl: bool) -> std::result::Result<Self, &'static str> {
        match value {
            "tcg" => Ok(Self::Tcg),
            "kvm" if wsl => Ok(Self::Kvm),
            "kvm" => Err("KVM selection requires the explicit wsl-lab build"),
            _ => Err("accelerator must be tcg or kvm"),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Tcg => "tcg",
            Self::Kvm => "kvm",
        }
    }

    pub(crate) fn arguments(self) -> [&'static str; 4] {
        match self {
            Self::Tcg => ["-accel", "tcg,thread=multi", "-cpu", "max"],
            Self::Kvm => ["-accel", "kvm", "-cpu", "host"],
        }
    }

    pub(crate) fn preflight(self) -> Result<()> {
        if self == Self::Kvm {
            require(crate::runner::WSL, "KVM selection requires the explicit wsl-lab build")?;
            check_kvm()?;
        }
        Ok(())
    }
}

enum KvmQuery {
    Api,
    Capability(usize),
}

// SAFETY: these two Linux KVM system ioctls return integers and take either
// no argument or an integer capability ID. They never access userspace memory.
// UAPI: linux/kvm.h KVM_GET_API_VERSION, KVM_CHECK_EXTENSION. No CREATE_VM exists here.
#[allow(unsafe_code)]
unsafe impl Ioctl for KvmQuery {
    type Output = IoctlOutput;
    const IS_MUTATING: bool = false;
    fn opcode(&self) -> Opcode {
        opcode::none(
            0xae,
            match self {
                Self::Api => 0x00,
                Self::Capability(_) => 0x03,
            },
        )
    }
    fn as_ptr(&mut self) -> *mut c_void {
        (match self {
            Self::Api => 0,
            Self::Capability(capability) => *capability,
        }) as *mut c_void
    }
    unsafe fn output_from_ptr(
        out: IoctlOutput,
        _: *mut c_void,
    ) -> rustix::io::Result<Self::Output> {
        Ok(out)
    }
}

#[allow(unsafe_code)]
fn check_kvm() -> Result<()> {
    let device = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags((rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as _)
        .open("/dev/kvm")?;
    let stat = rustix::fs::fstat(&device)?;
    require(
        rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::CharacterDevice
            && rustix::fs::major(stat.st_rdev) == 10
            && rustix::fs::minor(stat.st_rdev) == 232,
        "/dev/kvm is not the Linux KVM character device",
    )?;
    // SAFETY: pinned KVM device; fixed read-only system request and no pointer operand.
    let api = unsafe { ioctl(&device, KvmQuery::Api)? };
    require(api == 12, "KVM API version must be exactly 12")?;
    for (capability, name) in [(0, "IRQCHIP"), (1, "HLT"), (3, "USER_MEMORY"), (16, "SYNC_MMU")] {
        // SAFETY: fixed documented KVM capability IDs, integer argument only.
        let present = unsafe { ioctl(&device, KvmQuery::Capability(capability))? };
        require(present > 0, &format!("KVM capability {name} is unavailable"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_is_explicit_cloud_stays_tcg_and_neither_argument_has_a_fallback() {
        assert_eq!(Accelerator::default(), Accelerator::Tcg);
        assert_eq!(Accelerator::parse("kvm", true).unwrap(), Accelerator::Kvm);
        assert!(Accelerator::parse("kvm", false).is_err());
        for value in ["kvm:tcg", "auto", "", "KVM"] {
            assert!(Accelerator::parse(value, true).is_err());
        }
        assert_eq!(Accelerator::Tcg.arguments(), ["-accel", "tcg,thread=multi", "-cpu", "max"]);
        assert_eq!(Accelerator::Kvm.arguments(), ["-accel", "kvm", "-cpu", "host"]);
    }
}
