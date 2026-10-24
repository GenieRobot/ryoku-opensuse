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
    if crate::recovery_gate_results::RESULTS.contains(&line) {
        return true;
    }
    let Some((key, value)) = line.split_once('=') else {
        return false;
    };
    if key == "desktop_login_reason" {
        return desktop_login_reason(line).is_some();
    }
    if matches!(
        key,
        "mutation_probe_sha256"
            | "mutation_console_sha256"
            | "console_restore_probe_sha256"
            | "console_packages_sha256"
            | "console_signing_key_sha256"
            | "ryoku-native24-probe_sha256"
            | "ryoku-native24-packages.tar_sha256"
            | "ryoku-native24-source.txt_sha256"
    ) {
        return value.len() == 64
            && value.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    }
    if matches!(
        key,
        "native_issue21_fixture_installed"
            | "native_issue21_fixture_clock"
            | "native_issue21_offline_cli"
            | "native_issue21_configuration_metadata_absence"
            | "native_issue21_unrelated_home"
            | "native_issue21_interrupted_apply_resume"
            | "native_issue21_pending_graphical_admission_refused"
            | "native_issue21_linked_root_reboot_resume"
            | "native_issue21_system_only_scope"
            | "native_issue21_current_checkpoint_retained"
            | "native_issue21_bad_archive_refused"
            | "native_issue21_unavailable_partner_refused"
            | "native_issue21_complete_graphical_admission"
            | "native_issue21_completed_journals_after_reboot"
    ) {
        return value == "passed";
    }
    if matches!(
        key,
        "native_issue24_clean_install"
            | "native_issue24_repeat_install"
            | "native_issue24_versioned_migration"
            | "native_issue24_remove_and_reinstall"
            | "native_issue24_verified_prechange_checkpoints"
            | "native_issue24_interrupted_before_apply"
            | "native_issue24_interrupted_apply_linked_restore"
            | "native_issue24_wrong_recovery_receipt_refused"
            | "native_issue24_user_overrides_and_unrelated"
            | "native_issue24_distro_and_kde_preserved"
            | "native_issue24_bootstrap_and_dependencies"
            | "native_issue24_completed_evidence_after_reboot"
    ) {
        return value == "passed";
    }
    if key == "native_issue21_provider_actions" {
        return value == "not_exercised";
    }
    if matches!(key, "native_issue21_desktop_login" | "native_issue21_incompatible_packages") {
        return value == "unclaimed";
    }
    if matches!(
        key,
        "native_issue20_runtime_boundary"
            | "native_issue20_changed_path_blocks"
            | "native_issue20_low_space_blocks"
            | "native_issue20_cancel_blocks"
            | "native_issue20_capture_error_blocks"
            | "native_issue20_changed_plan_blocks"
            | "native_issue20_interruption_before_apply"
            | "native_issue20_concurrent_generations"
            | "native_issue20_retained_writer_lease"
            | "native_issue20_interrupted_apply"
            | "native_issue20_post_capture_failure"
            | "native_issue20_first_mutation_order"
            | "native_issue20_console_history"
            | "native_issue20_history_after_reboot"
    ) {
        return value == "passed";
    }
    if key == "native_issue20_provider_capabilities" {
        return value == "disabled";
    }
    if key == "native_issue20_multisubvolume_atomicity" {
        return value == "unclaimed";
    }
    let keys = [
        "fixture_sha256",
        "release_signer",
        "host_runner",
        "accelerator",
        "vm_memory_mib",
        "vm_cpus",
        "qemu",
        "ui_binary_sha256",
        "state_probe_sha256",
        "state_initializer_sha256",
        "state_prepare_sha256",
        "linked_probe_sha256",
        "recovery_cli_sha256",
        "native_linked_layout_observed",
        "native_linked_root_switch",
        "native_linked_boot_compatibility",
        "native_linked_restore_execution",
        "native_linked_duplicate_checkpoint_refused",
        "native_linked_original_identity_recorded",
        "native_linked_catalog_generation_readback",
        "native_linked_exact_snapshot_readback",
        "native_linked_planned_journal_readback",
        "native_linked_writable_snapshot_refused",
        "native_linked_missing_snapshot_refused",
        "native_linked_substituted_snapshot_refused",
        "native_linked_missing_generation_refused",
        "native_linked_owning_journal_cleanup",
        "native_linked_cli_pending",
        "native_linked_cli_verify",
        "native_linked_normal_reboot_readback",
        "native_issue19_full_root_boot_files",
        "native_issue19_configuration_only",
        "native_issue19_unsupported_layout",
        "native_issue19_package_hook_pair",
        "native_issue19_no_nested_pair",
        "native_issue19_additional_catalog",
        "native_issue19_manual_pin",
        "native_issue19_missing_additional",
        "native_issue19_missing_primary_partners",
        "native_issue19_catalog_corruption_refused",
        "native_issue19_catalog_after_rollback",
        "native_issue19_known_working_pin",
        "native_issue19_cleanup_ownership",
        "native_issue19_deleted_partner",
        "native_issue19_esp_coverage",
        "native_issue19_rollback_layout",
        "native_issue19_numbered_root_boot",
        "native_linked_changed_root_historical_readback",
        "native_linked_root_rollback_readback",
        "native_linked_cli_empty_after_cleanup",
        "zypp_probe_sha256",
        "orchestrator_sha256",
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
        "xf86-input-libinput",
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
        "lifecycle_result",
        "reboot",
        "egress",
        "egress_online_baseline",
        "native_state_creation",
        "native_state_unmounted_receipt_resume",
        "native_initializer_fresh",
        "native_initializer_repeat",
        "native_initializer_resume",
        "native_initializer_prior_attempt_refused",
        "native_initializer_wrong_uuid_refused",
        "native_initializer_bad_receipt_not_rewritten",
        "native_initializer_foreign_target_preserved",
        "native_initializer_foreign_unit_preserved",
        "native_initializer_unmounted_resume",
        "native_initializer_missing_boot_unit_refused",
        "native_initializer_changed_boot_unit_preserved",
        "native_initializer_missing_enablement_refused",
        "native_initializer_foreign_enablement_preserved",
        "native_journal_empty_scan",
        "native_journal_missing_store_refused",
        "native_journal_partial_record_preserved",
        "native_journal_pending_attempt_refused",
        "native_initializer_after_reboot",
        "native_state_resume_fresh_process",
        "native_state_creation_killed_process_resume",
        "native_state_existing_refused",
        "native_state_mount_identity",
        "native_state_wrong_identity_refused",
        "native_state_machine_record",
        "native_state_store_reopen",
        "native_state_boot_mount",
        "native_state_boot_unit_install",
        "native_state_store_reopen_after_reboot",
        "native_state_interruption_resume",
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
        "ui_framebuffer_capture",
        "zypp_experiment_is_not_a_protected_write_gate",
        "packagekit_cockpit_myrlyn_yast_write_paths",
        "zypp_inventory",
        "zypp_plugin_directory",
        "zypp_later_modes",
        "lab_result",
    ];
    let dynamic_key = (key.starts_with("zypp_inventory_")
        || key.starts_with("zypp_deny_commit_")
        || key.starts_with("zypp_malformed_commit_")
        || key.starts_with("zypp_timeout_commit_")
        || key.starts_with("zypp_missing_plugin_")
        || key.starts_with("zypp_probe_"))
        && key.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        });
    (keys.contains(&key) || dynamic_key)
        && !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.+".contains(&b))
}

pub fn desktop_login_reason(line: &str) -> Option<&'static str> {
    let value = line.strip_prefix("desktop_login_reason=")?;
    match value {
        "inspection_precondition_failed" => Some("inspection_precondition_failed"),
        "active_session_absent_or_mismatch" => Some("active_session_absent_or_mismatch"),
        "plasma_shell_absent" => Some("plasma_shell_absent"),
        "kwin_absent" => Some("kwin_absent"),
        "wayland_socket_absent" => Some("wayland_socket_absent"),
        "wayland_socket_connect_failure" => Some("wayland_socket_connect_failure"),
        "inspection_timeout" => Some("inspection_timeout"),
        _ => None,
    }
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
        assert!(public_evidence("native_linked_root_switch=unproven"));
        assert!(public_evidence("native_linked_normal_reboot_readback=passed"));
        assert!(!public_evidence("boot_fact={\"file\":\"/proc/cmdline\"}"));
        assert!(!public_evidence("native_linked_snapshot_id=42"));
        assert!(!public_evidence("native_linked_root_rollback_target=42"));
        for key in [
            "native_issue19_full_root_boot_files",
            "native_issue19_configuration_only",
            "native_issue19_unsupported_layout",
            "native_issue19_package_hook_pair",
            "native_issue19_no_nested_pair",
            "native_issue19_additional_catalog",
            "native_issue19_manual_pin",
            "native_issue19_missing_additional",
            "native_issue19_missing_primary_partners",
            "native_issue19_catalog_corruption_refused",
            "native_issue19_catalog_after_rollback",
            "native_issue19_known_working_pin",
            "native_issue19_cleanup_ownership",
            "native_issue19_deleted_partner",
            "native_issue19_esp_coverage",
            "native_issue19_rollback_layout",
            "native_issue19_numbered_root_boot",
            "native_linked_changed_root_historical_readback",
            "native_linked_root_rollback_readback",
        ] {
            assert!(public_evidence(&format!("{key}=passed")));
        }
        assert!(public_evidence("sddm=0.21.0+git57-1.1"));
        assert!(public_evidence("desktop_authentication=sddm-pam"));
        assert!(public_evidence("desktop_login_reason=active_session_absent_or_mismatch"));
        assert!(public_evidence("desktop_login_reason=inspection_timeout"));
        assert!(!public_evidence("desktop_login_reason=private-session-id"));
        assert!(!public_evidence("desktop_login_reason=failed:raw-error"));
        assert!(public_evidence("offline_snapper_restore=matched"));
        assert!(public_evidence("egress_online_baseline=reachable"));
        assert!(public_evidence("kernel-default-base=7.2.9-1.1.36.10"));
        assert!(public_evidence("reset_baseline=derived-btrfs-snapper"));
        assert!(public_evidence("reset_sentinel_before_discard=present"));
        assert!(public_evidence("reset_sentinel=absent"));
        assert!(public_evidence("reset_sentinel_bytes=matched"));
        assert!(public_evidence("reset_sentinel_sync=passed"));
        assert!(public_evidence("ui_visual_capture=passed"));
        assert!(public_evidence("ui_framebuffer_capture=passed"));
        assert!(public_evidence("zypp_inventory_path_1=usr_lib64_zypp_plugins_commit"));
        assert!(public_evidence("zypp_inventory_kind_1=symlink"));
        assert!(public_evidence("zypp_inventory_symlink_target_1=usr_lib_systemd_systemd"));
        assert!(public_evidence("zypp_inventory_acl_1=present"));
        assert!(public_evidence(
            "zypp_inventory_acl_sha256_1=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
        assert!(public_evidence("zypp_inventory_xattr_selinux_1=absent"));
        assert!(!public_evidence("zypp_inventory_path_1=/usr/lib64/zypp/plugins/commit"));
        assert!(public_evidence("zypp_inventory_zypper_version=1.14.80"));
        assert!(public_evidence("zypp_inventory_cockpit_socket_enabled=disabled"));
        assert!(public_evidence("zypp_inventory_cockpit_socket_active=inactive"));
        assert!(public_evidence("zypp_inventory_cockpit_socket_listen_scope=loopback-only"));
        assert!(public_evidence("zypp_inventory_host_tcp_listener_scope=non-loopback-present"));
        assert!(!public_evidence("zypp_inventory_cockpit_socket_listen_scope=127.0.0.1:9090"));
    }

    #[test]
    fn kde_timeout_evidence_exposes_only_the_bounded_category() {
        assert!(public_evidence("kde_provisioning=timeout"));
        assert!(public_evidence("kde_provisioning=blocked"));
        assert!(!public_evidence("kde_provisioning=zypper timed out after 20 minutes"));
    }
}
#[test]
fn mutation_evidence_accepts_only_verified_outcomes_and_explicit_limits() {
    assert!(public_evidence("native_issue20_low_space_blocks=passed"));
    assert!(public_evidence("native_issue20_provider_capabilities=disabled"));
    assert!(public_evidence("native_issue20_multisubvolume_atomicity=unclaimed"));
    for rejected in [
        "native_issue20_low_space_blocks=private-diagnostic",
        "native_issue20_provider_capabilities=enabled",
        "native_issue20_multisubvolume_atomicity=atomic",
        "mutation_probe_sha256=not-a-digest",
    ] {
        assert!(!public_evidence(rejected));
    }
}
