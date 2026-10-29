use super::*;
use std::{os::unix::fs::PermissionsExt, path::PathBuf};
struct Fixture {
    path: PathBuf,
    home: File,
    uid: u32,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "ryogami-provider-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(path.join(".config/ryoku")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = Self {
            home: File::open(&path).unwrap(),
            path,
            uid: rustix::process::geteuid().as_raw(),
        };
        result.write(SETTINGS, br#"{"semantic":{"manifest":"/fixture/selected.json","models":[{"name":"selected","manifest":"/fixture/selected.json"},{"name":"inactive","manifest":"/fixture/inactive.json"}]},"private":"fixture-secret","unchanged":9007199254740993}"#);
        result
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.path.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    fn prepare(&self, action: &ModelReferenceIntent) -> PreparedReference {
        prepare_owned(&self.home, self.uid, action, Sha256::new("a".repeat(64)).unwrap()).unwrap()
    }
    fn persist(&self, prepared: &PreparedReference) {
        let mut value = prepared.original.clone();
        value["semantic"]["models"] = serde_json::to_value(&prepared.commit.models).unwrap();
        self.write(SETTINGS, &serde_json::to_vec(&value).unwrap());
    }
    fn reply(&self, prepared: &PreparedReference) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"id":4,"result":{"committed":true,"models":prepared.commit.models.len()}})).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
#[test]
fn provider_prepares_owned_manifest_identity_and_revalidates_real_persisted_settings() {
    let f = Fixture::new();
    f.write("models/test/manifest.json", br#"{"id":"named-model","unrelated":"not-in-plan"}"#);
    let action = ModelReferenceIntent::Add { path: Some("~/models/test/manifest.json".into()) };
    let prepared = f.prepare(&action);
    assert_eq!(prepared.commit.models.last().unwrap().name, "named-model");
    assert!(prepared.source_identity.is_some());
    let actor = ActorIdentity { pid: 42, start_time: 7 };
    let plan =
        prepared.plan(&action, 4, &actor, &Sha256::new("b".repeat(64)).unwrap(), &[]).unwrap();
    assert!(!serde_json::to_string(&plan).unwrap().contains("fixture-secret"));
    assert!(
        !String::from_utf8(prepared.commit.frame(4).unwrap()).unwrap().contains("fixture-secret")
    );
    assert!(prepared.verify(&f.home, f.uid, &f.reply(&prepared), 4).is_err());
    f.persist(&prepared);
    prepared.verify(&f.home, f.uid, &f.reply(&prepared), 4).unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.path.join(SETTINGS)).unwrap()).unwrap();
    assert_eq!(value["private"], "fixture-secret");
    assert_eq!(value["unchanged"].as_u64(), Some(9007199254740993));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&prepared.public_reply(&action, 4).unwrap())
            .unwrap()["result"]["index"],
        2
    );
}
#[test]
fn provider_forget_preserves_selection_and_requires_real_matching_actor_result() {
    let f = Fixture::new();
    let action = ModelReferenceIntent::Forget { index: 1 };
    let prepared = f.prepare(&action);
    assert_eq!(prepared.commit.expected_selected, "/fixture/selected.json");
    f.persist(&prepared);
    for reply in [
        br#"{"id":4,"result":{"committed":false,"models":1}}"#.as_slice(),
        br#"{"id":5,"result":{"committed":true,"models":1}}"#,
        br#"{"id":4,"result":{"committed":true,"models":2}}"#,
        br#"{"id":4,"error":"fixture-secret"}"#,
        br#"{"id":4,"result":{"committed":true,"models":1,"other":true}}"#,
    ] {
        assert!(prepared.verify(&f.home, f.uid, reply, 4).is_err());
    }
    prepared.verify(&f.home, f.uid, &f.reply(&prepared), 4).unwrap();
    let value: serde_json::Value =
        serde_json::from_slice(&prepared.public_reply(&action, 4).unwrap()).unwrap();
    assert_eq!(value["result"]["removed"], true);
    assert!(matches!(
        prepare_owned(
            &f.home,
            f.uid,
            &ModelReferenceIntent::Forget { index: 0 },
            Sha256::new("a".repeat(64)).unwrap()
        ),
        Err(ReferenceError::SelectionCallbackRequired)
    ));
}
#[test]
fn provider_plan_detects_manifest_settings_actor_and_stage_changes() {
    let f = Fixture::new();
    f.write("models/test/manifest.json", br#"{"id":"first"}"#);
    let action = ModelReferenceIntent::Add {
        path: Some(f.path.join("models/test/manifest.json").to_str().unwrap().into()),
    };
    let first = f.prepare(&action);
    let actor = ActorIdentity { pid: 42, start_time: 7 };
    let bundle = Sha256::new("b".repeat(64)).unwrap();
    let plan = first.plan(&action, 4, &actor, &bundle, &[]).unwrap();
    f.write("models/test/manifest.json", br#"{"id":"second"}"#);
    assert_ne!(plan, f.prepare(&action).plan(&action, 4, &actor, &bundle, &[]).unwrap());
    assert_ne!(
        plan,
        first.plan(&action, 4, &ActorIdentity { pid: 42, start_time: 8 }, &bundle, &[]).unwrap()
    );
    f.write(&first.commit.staging_name(), b"interrupted sensitive settings");
    assert!(prepare_owned(&f.home, f.uid, &action, first.commit.staging_token.clone()).is_err());
    assert_eq!(
        std::fs::read(f.path.join(first.commit.staging_name())).unwrap(),
        b"interrupted sensitive settings"
    );
}
#[test]
fn provider_health_refuses_unrelated_changes_mode_changes_and_residual_stage() {
    for fault in ["secret", "mode", "stage", "selection"] {
        let f = Fixture::new();
        let prepared = f.prepare(&ModelReferenceIntent::Add { path: None });
        f.persist(&prepared);
        match fault {
            "mode" => std::fs::set_permissions(
                f.path.join(SETTINGS),
                std::fs::Permissions::from_mode(0o640),
            )
            .unwrap(),
            "stage" => f.write(&prepared.commit.staging_name(), b"partial"),
            _ => {
                let mut value: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(f.path.join(SETTINGS)).unwrap()).unwrap();
                if fault == "secret" {
                    value["private"] = "changed".into();
                } else {
                    value["semantic"]["manifest"] = "".into();
                }
                f.write(SETTINGS, &serde_json::to_vec(&value).unwrap());
            }
        }
        assert!(prepared.verify(&f.home, f.uid, &f.reply(&prepared), 4).is_err());
    }
}
#[test]
fn provider_reads_only_bounded_owned_regular_inputs_and_retains_fallback_names() {
    let f = Fixture::new();
    let action = ModelReferenceIntent::Add { path: Some("~/missing/model.json".into()) };
    assert_eq!(f.prepare(&action).commit.models.last().unwrap().name, "missing");
    f.write("outside.json", br#"{"id":"must-not-follow-link"}"#);
    std::os::unix::fs::symlink(f.path.join("outside.json"), f.path.join("linked.json")).unwrap();
    let link = ModelReferenceIntent::Add { path: Some("~/linked.json".into()) };
    assert!(f.prepare(&link).source_identity.is_none());
    assert!(read_owned(&f.home, SETTINGS, f.uid + 1, SETTINGS_LIMIT, false).is_err());
    assert!(read_owned(&f.home, SETTINGS, f.uid, 2, false).is_err());
    std::fs::remove_file(f.path.join(SETTINGS)).unwrap();
    std::os::unix::fs::symlink(f.path.join("outside.json"), f.path.join(SETTINGS)).unwrap();
    assert!(prepare_owned(&f.home, f.uid, &action, Sha256::new("a".repeat(64)).unwrap()).is_err());
}

#[test]
#[ignore = "requires the pinned patched Go actor test binary; see issue175 provider receipt"]
fn rust_provider_to_native_go_writer_verifies_success_reopen_and_failed_stage() {
    let binary = std::env::var_os("RYOKU_MODEL_ACTOR_TEST_BINARY")
        .expect("set explicit locally built patched Go actor test binary");
    for fault in ["none", "forget", "stale", "collision", "xattr"] {
        let f = Fixture::new();
        let action = if fault == "forget" {
            ModelReferenceIntent::Forget { index: 1 }
        } else {
            ModelReferenceIntent::Add { path: None }
        };
        let prepared = f.prepare(&action);
        f.write(".fixture-commit.json", &prepared.commit.frame(4).unwrap());
        if fault == "stale" {
            let mut value = prepared.original.clone();
            value["private"] = "newer-private-value".into();
            f.write(SETTINGS, &serde_json::to_vec(&value).unwrap());
        } else if fault == "collision" {
            f.write(&prepared.commit.staging_name(), b"prior-interrupted-stage");
        } else if fault == "xattr" {
            fs::setxattr(
                f.path.join(SETTINGS),
                "user.fixture",
                b"unenrolled",
                fs::XattrFlags::empty(),
            )
            .unwrap();
        }
        let before = std::fs::read(f.path.join(SETTINGS)).unwrap();
        let status = std::process::Command::new(&binary)
            .args(["-test.run", "^TestBrokerModelsRustProviderFixture$", "-test.count=1"])
            .env("RYOKU_MODEL_PROVIDER_FIXTURE", &f.path)
            .status()
            .unwrap();
        assert!(status.success());
        let reply = std::fs::read(f.path.join(".fixture-reply.json")).unwrap();
        let verified = prepared.verify(&f.home, f.uid, &reply, 4);
        if matches!(fault, "none" | "forget") {
            verified.unwrap();
            let reopened = f.prepare(&ModelReferenceIntent::Add { path: None });
            assert_eq!(reopened.commit.models.len(), prepared.commit.models.len() + 1);
            assert_eq!(reopened.commit.expected_selected, prepared.commit.expected_selected);
        } else {
            assert!(verified.is_err());
            assert_eq!(std::fs::read(f.path.join(SETTINGS)).unwrap(), before);
            if fault == "collision" {
                assert_eq!(
                    std::fs::read(f.path.join(prepared.commit.staging_name())).unwrap(),
                    b"prior-interrupted-stage"
                );
            } else if fault == "xattr" {
                assert!(
                    !std::fs::read(f.path.join(prepared.commit.staging_name())).unwrap().is_empty()
                );
            }
        }
    }
}
