use super::*;
use ryoku_recovery::ryogami_sources_protocol::{Search, SearchFilters, Source, SourceMetadataPage};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

struct Fixture {
    path: PathBuf,
    root: File,
    account: CaptureAccount,
}
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ryoku-source-receipt-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let uid = rustix::process::geteuid().as_raw().max(1000);
        let gid = rustix::process::getegid().as_raw();
        let account = CaptureAccount {
            uid,
            gid,
            principal: AccountPrincipal::from_account(uid, gid, "fixture", path.to_str().unwrap())
                .unwrap(),
        };
        Self { root: File::open(&path).unwrap(), path, account }
    }
    fn store(&self) -> ReceiptStore {
        ReceiptStore::open(&self.root, &self.account, rustix::process::geteuid().as_raw()).unwrap()
    }
    fn request(&self, id: &str) -> RequestRecord {
        RequestRecord {
            version: 1,
            account: Binding::from(&self.account),
            request_id: Identifier::new(id).unwrap(),
            client_correlation: "fixture-correlation".into(),
            created_unix: 1234,
            rpc_id: 7,
            query: PreparedSourceMetadataQuery {
                expected_settings: Some(hash(b"fixture-settings-only")),
                query: SourceMetadataQuery {
                    source: Source::Bing,
                    search: Search {
                        query: "fixture search".into(),
                        page: 1,
                        generation: 5,
                        filters: SearchFilters::Bing { market: "en-US".into() },
                    },
                },
            },
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).unwrap();
    }
}
fn response(request: &PreparedRequest, count: usize) -> Outcome {
    Outcome::Responded {
        actor: ActorIdentity { pid: 42, start_time: 100 },
        response: SourceMetadataResponse {
            id: request.record.rpc_id,
            request_id: request.record.request_id.clone(),
            error: None,
            result: Some(SourceMetadataPage {
                results: (0..count)
                    .map(|index| SourceMetadataItem {
                        id: format!("fixture-{index}"),
                        full_url: "https://fixture.invalid/private?token=private-source-value"
                            .into(),
                        thumb_url: "https://fixture.invalid/private-thumb".into(),
                        resolution: "3840x2160".into(),
                        file_size: 42,
                        title: format!("Fixture {index} {}", "🌲".repeat(128)),
                        track_url: "https://fixture.invalid/private-track".into(),
                        attribution: "Fixture Author".into(),
                        attribution_url: "https://fixture.invalid/author".into(),
                        duration_secs: 12,
                        purity: "sfw".into(),
                        category: "nature".into(),
                    })
                    .collect(),
                last_page: 3,
                current_page: 1,
                next_cursor: "provider-private-cursor".into(),
            }),
        },
    }
}
fn put(file: &File, name: &str, bytes: &[u8], mode: u32) {
    let mut output = File::from(
        fs::openat2(
            file,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::from_raw_mode(mode),
            NO_LINKS,
        )
        .unwrap(),
    );
    output.write_all(bytes).unwrap();
}

#[test]
fn metadata_receipts_reopen_with_account_bound_paginated_redacted_item_locators() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let request = store.prepare(fixture.request("source-fixture-one")).unwrap();
    assert!(store.load(&request.token).is_err(), "started request became a completed result");
    let saved = store.complete(&request, response(&request, 100)).unwrap();
    let expected_token = saved.token.clone();
    drop(saved);
    drop(store);
    let store = fixture.store();
    let mut saved = store.load(&expected_token).unwrap();
    let mut offset = 0;
    let mut seen = BTreeSet::new();
    let mut pages = 0;
    loop {
        let bytes = saved.page(9, offset).unwrap();
        assert!(bytes.len() <= MAX_PUBLIC_REPLY);
        let text = std::str::from_utf8(&bytes).unwrap();
        for private in [
            "private-source-value",
            "fixture.invalid",
            "fullUrl",
            "thumbUrl",
            "trackUrl",
            "attributionUrl",
            "provider-private-cursor",
        ] {
            assert!(!text.contains(private), "{private}");
        }
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["id"], 9);
        assert_eq!(value["result"]["generation"], 5);
        assert_eq!(value["result"]["total"], 100);
        assert_eq!(value["result"]["currentPage"], 1);
        for item in value["result"]["results"].as_array().unwrap() {
            let locator = Sha256::new(item["selection"].as_str().unwrap()).unwrap();
            let selected = saved.item(&locator).unwrap();
            assert_eq!(selected.id, item["id"]);
            assert_eq!(selected.title, item["title"]);
            assert!(seen.insert(selected.id.clone()));
            assert!(store.cursor(&locator).is_err(), "item locator accepted as a page cursor");
        }
        pages += 1;
        let Some(cursor) = value["result"]["next_cursor"].as_str() else { break };
        let (next, next_offset) = store.cursor(&Sha256::new(cursor).unwrap()).unwrap();
        assert!(next_offset > offset);
        assert!(
            next.item(&Sha256::new(cursor).unwrap()).is_err(),
            "page cursor became an item selector"
        );
        saved = next;
        offset = next_offset;
        assert!(pages < 100);
    }
    assert_eq!(seen.len(), 100);
    assert!(pages > 3);
}

#[test]
fn metadata_refuses_changed_nonce_source_page_account_and_locator_before_publication() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for kind in ["nonce", "id", "actor", "provider", "both", "neither"] {
        let request = store.prepare(fixture.request(&format!("source-refuse-{kind}"))).unwrap();
        let mut outcome = response(&request, 1);
        let Outcome::Responded { actor, response } = &mut outcome else { unreachable!() };
        match kind {
            "nonce" => response.request_id = Identifier::new("source-other-request").unwrap(),
            "id" => response.id += 1,
            "actor" => actor.start_time = 0,
            "provider" => {
                response.result.as_mut().unwrap().results[0].full_url = "file:///private".into()
            }
            "both" => response.error = Some(SourceMetadataFailure::InvalidResponse),
            "neither" => response.result = None,
            _ => unreachable!(),
        }
        assert!(store.complete(&request, outcome).is_err(), "{kind}");
        assert!(store.load(&request.token).is_err());
    }
    let request = store.prepare(fixture.request("source-good-locator")).unwrap();
    let saved = store.complete(&request, response(&request, 1)).unwrap();
    let locator = saved.locator(0, "ryoku-source-item-v1").unwrap();
    let mut bad = locator.as_str().as_bytes().to_vec();
    bad[42] = if bad[42] == b'0' { b'1' } else { b'0' };
    assert!(saved.item(&Sha256::new(String::from_utf8(bad).unwrap()).unwrap()).is_err());
    let mut other = fixture.account.clone();
    other.principal = AccountPrincipal::from_account(
        other.uid,
        other.gid,
        "other-fixture",
        fixture.path.to_str().unwrap(),
    )
    .unwrap();
    let other_store =
        ReceiptStore::open(&fixture.root, &other, rustix::process::geteuid().as_raw()).unwrap();
    assert!(other_store.load(&request.token).is_err());
    assert!(saved.validate(&Binding::from(&other)).is_err());
}

#[test]
fn request_and_result_publication_are_exclusive_and_interrupted_records_stay_unconfirmed() {
    let fixture = Fixture::new();
    let store = fixture.store();
    let record = fixture.request("source-exclusive-one");
    let request = store.prepare(record.clone()).unwrap();
    assert!(store.prepare(record).is_err());
    assert!(store.load(&request.token).is_err());
    let saved = store.complete(&request, response(&request, 1)).unwrap();
    assert!(store.complete(&request, response(&request, 1)).is_err());
    assert_eq!(store.load(&request.token).unwrap().sha256, saved.sha256);
    let prefix = "f".repeat(40);
    put(&store.directory, &format!("q-{prefix}.request.pending"), b"interrupted", 0o600);
    assert!(store.load(&prefix).is_err());
    let next = store.prepare(fixture.request("source-exclusive-two")).unwrap();
    store.complete(&next, response(&next, 1)).unwrap();
}

#[test]
fn metadata_failure_receipts_are_truthful_and_cannot_select_an_asset() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for (name, outcome) in [
        ("source-transport", Outcome::TransportUnavailable),
        (
            "source-auth",
            Outcome::Responded {
                actor: ActorIdentity { pid: 42, start_time: 100 },
                response: SourceMetadataResponse {
                    id: 7,
                    request_id: Identifier::new("source-auth").unwrap(),
                    result: None,
                    error: Some(SourceMetadataFailure::MissingCredentials),
                },
            },
        ),
    ] {
        let request = store.prepare(fixture.request(name)).unwrap();
        let saved = store.complete(&request, outcome).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&saved.page(7, 0).unwrap()).unwrap();
        assert!(value["error"]["message"].is_string());
        assert_eq!(value["error"]["data"]["request_id"], name);
        assert!(saved.item(&saved.locator(0, "ryoku-source-item-v1").unwrap()).is_err());
    }
}

#[test]
fn metadata_receipt_tampering_links_modes_and_oversized_files_refuse() {
    for kind in ["request", "result", "symlink", "hardlink", "mode", "oversized"] {
        let fixture = Fixture::new();
        let store = fixture.store();
        let request = store.prepare(fixture.request("source-tamper")).unwrap();
        let saved = store.complete(&request, response(&request, 1)).unwrap();
        let name = if kind == "request" {
            format!("q-{}.request.json", request.token)
        } else {
            format!("q-{}.result.json", request.token)
        };
        let original = store.read(&name, MAX_RESULT).unwrap();
        let path = PathBuf::from(format!(
            "/proc/self/fd/{}",
            std::os::fd::AsRawFd::as_raw_fd(&store.directory)
        ))
        .join(&name);
        match kind {
            "request" | "result" => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
                std::fs::write(&path, b"{}").unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
            }
            "symlink" => {
                std::fs::remove_file(&path).unwrap();
                std::os::unix::fs::symlink("unrelated-secret", &path).unwrap();
            }
            "hardlink" => {
                std::fs::hard_link(&path, fixture.path.join("alias")).unwrap();
            }
            "mode" => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap()
            }
            "oversized" => {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_len((MAX_RESULT + 1) as u64)
                    .unwrap();
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(store.load(&saved.token).is_err(), "{kind}");
        if kind == "symlink" {
            assert!(!fixture.path.join("unrelated-secret").exists());
        }
        assert!(!original.is_empty());
    }
}

#[test]
fn reserved_result_capacity_prevents_unbounded_unconfirmed_requests_before_dispatch() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for index in 0..(MAX_STORE_BYTES / MAX_RESULT as u64 + 1) {
        put(&store.directory, &format!("q-{index:040x}.request.pending"), b"", 0o600);
    }
    let record = fixture.request("source-capacity");
    let token = record.token().unwrap();
    assert_eq!(
        store.prepare(record).err(),
        Some(ServiceError::Malformed("source_history_capacity".into()))
    );
    assert!(
        fs::statat(
            &store.directory,
            format!("q-{token}.request.json").as_str(),
            fs::AtFlags::SYMLINK_NOFOLLOW
        )
        .is_err()
    );
}

#[test]
fn request_reserves_a_result_directory_entry_before_remote_dispatch() {
    let fixture = Fixture::new();
    let store = fixture.store();
    // Empty completed pairs isolate the entry limit from the separate byte
    // limit. The store does not treat these synthetic files as valid receipts.
    for index in 0..((MAX_STORE_FILES - 1) / 2) {
        for kind in ["request", "result"] {
            put(&store.directory, &format!("q-{index:040x}.{kind}.json"), b"", 0o400);
        }
    }
    let last = format!("{:040x}", MAX_STORE_FILES);
    put(&store.directory, &format!("q-{last}.request.pending"), b"", 0o600);
    let record = fixture.request("source-entry-capacity");
    let token = record.token().unwrap();
    assert_eq!(
        store.prepare(record).err(),
        Some(ServiceError::Malformed("source_history_capacity".into()))
    );
    assert!(store.read(&format!("q-{token}.request.json"), MAX_REQUEST).is_err());
}

#[test]
fn receipt_plan_cannot_supply_process_sources_or_foreign_account_bindings() {
    let fixture = Fixture::new();
    let store = fixture.store();
    for source in [Source::Steam, Source::Youtube] {
        let mut record = fixture.request("source-process");
        record.query.query.source = source;
        assert!(store.prepare(record).is_err());
    }
    let mut record = fixture.request("source-foreign");
    record.account.uid += 1;
    assert!(store.prepare(record).is_err());
    let mut record = fixture.request("source-invalid-correlation");
    record.client_correlation = "../private".into();
    assert!(store.prepare(record).is_err());
}
