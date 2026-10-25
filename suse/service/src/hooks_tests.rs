use super::*;
#[test]
fn configured_command_indices_match_filtered_upstream_entries_and_restore_flag() {
    let bytes=br#"{"postProcessing":["",null,{"type":"video"},"printf bare",{"command":"printf video","type":"video"}],"postProcessOnRestore":true}"#;
    assert_eq!(command(bytes, 0).unwrap(), ("printf bare".into(), "all".into(), true));
    assert_eq!(command(bytes, 1).unwrap(), ("printf video".into(), "video".into(), true));
    assert!(command(bytes, 2).is_err());
    assert!(command(b"{", 0).is_err());
    assert!(command(br#"{"postProcessing":"printf bad"}"#, 0).is_err());
}
#[test]
fn unknown_and_ambiguous_wire_fields_do_not_reach_policy() {
    for payload in [
        serde_json::json!({"operation":"run_configured_hook","action":{"index":0,"command_sha256":"ab".repeat(32),"path":"/wall.png","kind":"static","restoring":false,"command":"touch marker"}}),
        serde_json::json!({"operation":"authorize_configured_hook","policy":{"index":0,"command_sha256":"ab".repeat(32),"outputs":["arbitrary_path"],"timeout_seconds":1,"allow_restore":false}}),
    ] {
        assert!(serde_json::from_value::<crate::HelperRequest>(payload).is_err());
    }
}
#[test]
fn staged_policy_is_explicit_and_changes_the_authenticated_policy_bytes() {
    let mut value = serde_json::json!({"index":0,"command_sha256":"ab".repeat(32),"outputs":["kitty_user"],"timeout_seconds":1,"allow_restore":false});
    let direct: HookApprovalRequest = serde_json::from_value(value.clone()).unwrap();
    assert!(direct.validate());
    value["execution"] = serde_json::json!("staged_outputs");
    let staged: HookApprovalRequest = serde_json::from_value(value.clone()).unwrap();
    assert!(staged.validate());
    assert_ne!(direct, staged);
    assert_ne!(
        digest(&serde_json::to_vec(&direct).unwrap()),
        digest(&serde_json::to_vec(&staged).unwrap())
    );
    value["execution"] = serde_json::json!("unconfined");
    assert!(serde_json::from_value::<HookApprovalRequest>(value).is_err());
}
#[test]
fn template_read_set_requires_explicit_staging_and_exact_unique_bounded_paths() {
    let input =
        serde_json::json!({"path":".config/matugen/templates/exact.conf","sha256":"ab".repeat(32)});
    let mut value = serde_json::json!({"index":0,"command_sha256":"cd".repeat(32),"outputs":["kitty_user"],"timeout_seconds":1,"allow_restore":false,"execution":"staged_outputs","templates":[input.clone()]});
    let policy: HookApprovalRequest = serde_json::from_value(value.clone()).unwrap();
    assert!(policy.validate());
    value["execution"] = serde_json::json!("direct_write_only");
    assert!(!serde_json::from_value::<HookApprovalRequest>(value.clone()).unwrap().validate());
    value["execution"] = serde_json::json!("staged_outputs");
    value["templates"] = serde_json::json!([input.clone(), input.clone()]);
    assert!(!serde_json::from_value::<HookApprovalRequest>(value.clone()).unwrap().validate());
    value["templates"] = serde_json::json!([{"path":".ssh/id_ed25519","sha256":"ab".repeat(32)}]);
    assert!(!serde_json::from_value::<HookApprovalRequest>(value.clone()).unwrap().validate());
    value["templates"] = serde_json::json!([{"path":".config/matugen/templates/../secret","sha256":"ab".repeat(32)}]);
    assert!(serde_json::from_value::<HookApprovalRequest>(value.clone()).is_err());
    value["templates"] = serde_json::json!((0..33).map(|i|serde_json::json!({"path":format!(".config/matugen/templates/{i}.conf"),"sha256":"ab".repeat(32)})).collect::<Vec<_>>());
    assert!(!serde_json::from_value::<HookApprovalRequest>(value).unwrap().validate());
}
#[test]
fn configured_hook_has_no_approval_without_durable_successful_post_checkpoint() {
    let path = std::env::temp_dir().join(format!("ryoku-hook-journal-{}", std::process::id()));
    let file =
        std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path).unwrap();
    let mut journal = crate::OperationJournal::open(file).unwrap();
    let id = OperationId::new("hook-approval-test").unwrap();
    assert!(!completed(&journal, &id));
    // A started/failed authorization is deliberately unusable, even if its
    // policy bytes reached rollback-independent storage before interruption.
    journal
        .record(crate::OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::AuthorizeConfiguredHook,
            phase: OperationPhase::Accepted,
            progress: None,
            error: None,
        })
        .unwrap();
    assert!(!completed(&journal, &id));
    std::fs::remove_file(path).unwrap();
}
