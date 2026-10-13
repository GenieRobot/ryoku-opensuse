use std::process::Command;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn output(command: &mut Command) -> Result<String> {
    let result = command.output()?;
    if !result.status.success() {
        // Keep command output private: it can contain identities or proxy values.
        return Err(format!(
            "{} exited {}",
            command.get_program().to_string_lossy(),
            result.status
        )
        .into());
    }
    Ok(String::from_utf8(result.stdout)?)
}

pub fn require(condition: bool, message: &str) -> Result<()> {
    if condition { Ok(()) } else { Err(message.into()) }
}

pub fn safe_component(value: &str) -> Result<&str> {
    require(
        !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "unsafe component",
    )?;
    Ok(value)
}

pub fn public_evidence(line: &str) -> bool {
    let Some((key, value)) = line.split_once('=') else {
        return false;
    };
    let keys = [
        "fixture_sha256",
        "release_signer",
        "host_runner",
        "accelerator",
        "vm_memory_mib",
        "vm_cpus",
        "qemu",
        "image_signature",
        "fresh_overlay",
        "fresh_boot",
        "source_root",
        "derived_root",
        "derived_esp",
        "derived_grub2",
        "derived_initramfs",
        "metadata_acl_context",
        "repository_metadata",
        "derived_btrfs_boot",
        "setup",
        "kernel-default-base",
        "kernel_running",
        "grub2",
        "grub2-x86_64-efi",
        "snapper",
        "grub2-snapper-plugin",
        "plasma6-session",
        "sddm",
        "snapshot",
        "arch",
        "root",
        "boot",
        "selinux",
        "snapper_config_label",
        "snapper_dbus_list",
        "kde_display_manager",
        "kde_provisioning",
        "desktop_login_prepared",
        "desktop_user",
        "desktop_session_service",
        "desktop_session_class",
        "desktop_session_seat",
        "desktop_session_type",
        "desktop_session_state",
        "desktop_session_remote",
        "desktop_session_owner",
        "desktop_pam",
        "desktop_plasmashell",
        "desktop_compositor",
        "desktop_wayland_socket",
        "desktop_authentication",
        "desktop_greeter",
        "desktop_login",
        "reboot",
        "egress",
        "controlled_corruption",
        "offline_snapper_restore",
        "fixture_marker_cleanup",
        "reset_marker",
        "reset_baseline",
        "reset_root",
        "reset_snapper_config",
        "reset_sentinel_before_discard",
        "reset_sentinel",
        "reset_sentinel_bytes",
        "reset_sentinel_sync",
        "reset_sentinel_length",
        "reset_package",
        "reset_fresh_boot",
        "base_unchanged",
        "reset_disk_and_keys",
        "ui_visual_capture",
        "lab_result",
    ];
    keys.contains(&key)
        && !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_reject_command_injection_and_traversal() {
        for invalid in ["", "../overlay", "a;b", "$(id)", "a b", "a/b", "a\nb"] {
            assert!(safe_component(invalid).is_err());
        }
        assert!(safe_component("run-20261012_1").is_ok());
    }

    #[test]
    fn unsuccessful_command_never_becomes_success() {
        assert!(output(&mut Command::new("false")).is_err());
    }

    #[test]
    fn artifacts_reject_unknown_identity_and_network_fields() {
        for line in [
            "access_token=abcdef",
            "desktop_session_id=1",
            "desktop_uid=1001",
            "ssh_host=localhost",
            "proxy=172.31.1.1",
            "root=/private/path",
            "snapshot=",
            "snapper=1.2\nprivate-data",
        ] {
            assert!(!public_evidence(line));
        }
        assert!(public_evidence("snapper=0.12.1-1.2"));
        assert!(public_evidence("sddm=0.21.0+git57-1.1"));
        assert!(public_evidence("desktop_authentication=sddm-pam"));
        assert!(public_evidence("offline_snapper_restore=matched"));
        assert!(public_evidence("kernel-default-base=7.2.9-1.1.36.10"));
        assert!(public_evidence("reset_baseline=derived-btrfs-snapper"));
        assert!(public_evidence("reset_sentinel_before_discard=present"));
        assert!(public_evidence("reset_sentinel=absent"));
        assert!(public_evidence("reset_sentinel_bytes=matched"));
        assert!(public_evidence("reset_sentinel_sync=passed"));
        assert!(public_evidence("ui_visual_capture=passed"));
    }
}
