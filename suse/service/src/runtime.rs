//! Minimal per-user Unix socket service runtime.
//!
//! This runtime exposes read-only discovery, bounded local profile reads,
//! durable operation lookup, and helper-backed checkpoint
//! verification. Privileged managed actions are explicitly refused until an
//! audited provider and linked recovery gate are integrated.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::{Duration, Instant};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
};

use crate::{
    OperationJournal, PROTOCOL_VERSION, PeerIdentity, ProtocolError, Request, RequestId, Response,
    ResponseBody, ServiceError, account_state::AccountHomeRoot, authorize_helper_peer, discovery,
};

const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Serve requests until the listener is closed. Connections are processed
/// serially and each connection carries exactly one bounded request frame.
fn serve(
    listener: UnixListener,
    mut journal: OperationJournal,
    paths: crate::runtime_paths::RuntimePaths,
) -> std::io::Result<()> {
    for accepted in listener.incoming() {
        let stream = accepted?;
        let _ = serve_connection_checked(
            stream,
            &mut journal,
            paths.uid(),
            Path::new(crate::helper_protocol::HELPER_SOCKET_PATH),
            || paths.revalidate().map_err(|_| ServiceError::RecoveryUnavailable),
        );
    }
    Ok(())
}

/// Handle one connected socket. The authenticated socket UID must match this
/// unprivileged daemon's effective UID; requests never execute provider work.
pub fn serve_connection(
    stream: UnixStream,
    journal: &mut OperationJournal,
    uid: u32,
) -> Result<(), ProtocolError> {
    serve_connection_with_helper_path(
        stream,
        journal,
        uid,
        Path::new(crate::helper_protocol::HELPER_SOCKET_PATH),
    )
}

fn serve_connection_with_helper_path(
    stream: UnixStream,
    journal: &mut OperationJournal,
    uid: u32,
    helper_socket_path: &Path,
) -> Result<(), ProtocolError> {
    serve_connection_checked(stream, journal, uid, helper_socket_path, || Ok(()))
}

fn serve_connection_checked(
    mut stream: UnixStream,
    journal: &mut OperationJournal,
    uid: u32,
    helper_socket_path: &Path,
    validate_paths: impl Fn() -> Result<(), ServiceError>,
) -> Result<(), ProtocolError> {
    let deadline = Instant::now() + IO_TIMEOUT;
    stream.set_nonblocking(true).map_err(|_| ProtocolError::Io)?;
    let peer = PeerIdentity::from_unix_socket(&stream)?;
    if authorize_helper_peer(&peer, uid).is_err() {
        return write_response(
            &mut stream,
            Response {
                version: PROTOCOL_VERSION,
                request_id: RequestId::new("invalid").expect("static request id is valid"),
                body: ResponseBody::Error(ServiceError::Unauthorized),
            },
            deadline,
        );
    }
    // Kernel peer authentication precedes NSS/path work and request parsing.
    // Stale namespaces cannot expose replayed lookup results or probe helpers.
    if let Err(error) = validate_paths() {
        return write_response(
            &mut stream,
            Response {
                version: PROTOCOL_VERSION,
                request_id: RequestId::new("invalid").expect("static request id is valid"),
                body: ResponseBody::Error(error),
            },
            deadline,
        );
    }
    let request_id = RequestId::new("invalid").expect("static request id is valid");
    let frame = match read_frame_until(&mut stream, deadline) {
        Ok(frame) => frame,
        Err(ProtocolError::Io) => return Err(ProtocolError::Io),
        Err(error) => {
            let body = match error {
                ProtocolError::TooLarge | ProtocolError::Malformed(_) => {
                    ResponseBody::Error(ServiceError::Malformed("invalid request frame".into()))
                }
                ProtocolError::UnsupportedVersion(_) | ProtocolError::Io => unreachable!(),
            };
            return write_response(
                &mut stream,
                Response { version: PROTOCOL_VERSION, request_id, body },
                deadline,
            );
        }
    };
    let envelope = match crate::read_envelope(&mut std::io::Cursor::new(frame)) {
        Ok(envelope) => envelope,
        Err(error) => {
            let body = match error {
                ProtocolError::UnsupportedVersion(_) => {
                    ResponseBody::Error(ServiceError::Unsupported)
                }
                ProtocolError::Malformed(_) | ProtocolError::TooLarge => {
                    ResponseBody::Error(ServiceError::Malformed("invalid request frame".into()))
                }
                ProtocolError::Io => return Err(ProtocolError::Io),
            };
            return write_response(
                &mut stream,
                Response { version: PROTOCOL_VERSION, request_id, body },
                deadline,
            );
        }
    };
    // Input may have taken the full framing deadline. Revalidate once more
    // immediately before dispatch rather than relying on its earlier check.
    if let Err(error) = validate_paths() {
        return write_response(
            &mut stream,
            Response { version: PROTOCOL_VERSION, request_id, body: ResponseBody::Error(error) },
            deadline,
        );
    }
    let id = envelope.request_id;
    if journal.refresh_after_write_failure().is_err() {
        return write_response(
            &mut stream,
            Response {
                version: PROTOCOL_VERSION,
                request_id: id,
                body: ResponseBody::Error(ServiceError::RecoveryUnavailable),
            },
            deadline,
        );
    }
    let body = dispatch_request(envelope.request, journal, |request| match request {
        Request::GetLocalProfile
        | Request::SetLocalProfile { .. }
        | Request::MigrateLegacyLocalProfile => handle_local_profile_request(request, &peer, uid),
        Request::VerifyLinkedCheckpoint { transaction_id, generation_id } => {
            crate::helper_protocol::verify_linked_checkpoint_at(
                helper_socket_path,
                transaction_id,
                generation_id,
            )
            .map(ResponseBody::LinkedCheckpoint)
            .unwrap_or_else(ResponseBody::Error)
        }
        Request::Start { action } => ResponseBody::Error(
            crate::helper_protocol::check_action_availability_at(helper_socket_path, action.into()),
        ),
        Request::Discover | Request::GetOperation { .. } => {
            unreachable!("non-helper request reached helper callback")
        }
    });
    write_response(
        &mut stream,
        Response { version: PROTOCOL_VERSION, request_id: id, body },
        deadline,
    )
}

fn dispatch_request(
    request: Request,
    journal: &OperationJournal,
    mut helper: impl FnMut(&Request) -> ResponseBody,
) -> ResponseBody {
    match request {
        Request::Discover => {
            // The root helper opens and validates the protected recovery
            // mount before accepting requests. Its typed Unsupported result
            // means the helper is healthy while this action remains disabled;
            // transport, authorization, or recovery-state failures stay
            // fail-closed as RecoveryUnavailable.
            let probe = Request::Start { action: crate::ManagedAction::CreateCheckpoint };
            let recovery_available =
                matches!(helper(&probe), ResponseBody::Error(ServiceError::Unsupported));
            ResponseBody::Discovery(discovery(recovery_available))
        }
        Request::GetOperation { operation_id } => journal
            .get(&operation_id)
            .cloned()
            .map(ResponseBody::Operation)
            .unwrap_or(ResponseBody::Error(ServiceError::NotFound)),
        Request::Start { .. } if !journal.recovery_required().is_empty() => {
            ResponseBody::Error(ServiceError::RecoveryUnavailable)
        }
        request @ Request::Start { .. } => helper(&request),
        request @ (Request::GetLocalProfile
        | Request::SetLocalProfile { .. }
        | Request::MigrateLegacyLocalProfile) => helper(&request),
        request @ Request::VerifyLinkedCheckpoint { .. } => helper(&request),
        // No executor has passed the linked capture and provider review gates.
        // Never create an accepted operation for work not run.
    }
}

fn handle_local_profile_request(request: &Request, peer: &PeerIdentity, uid: u32) -> ResponseBody {
    let account = match AccountHomeRoot::for_peer(peer) {
        Ok(account) if account.uid() == uid => account,
        _ => return ResponseBody::Error(ServiceError::RecoveryUnavailable),
    };
    handle_local_profile_for_account(request, &account)
}

fn handle_local_profile_for_account(request: &Request, account: &AccountHomeRoot) -> ResponseBody {
    let owner = ryoku_profiles::ProfileOwner { uid: account.uid(), gid: account.primary_gid() };
    if account.revalidate().is_err() {
        return ResponseBody::Error(ServiceError::StaleRequest);
    }

    let create = matches!(request, Request::SetLocalProfile { .. });
    let root = match account.configuration(create) {
        Ok(root) => root,
        Err(_) => return ResponseBody::Error(ServiceError::RecoveryUnavailable),
    };
    let response = handle_local_profile_with_optional_root(request, root.as_ref(), owner);
    if account.revalidate().is_err() {
        return ResponseBody::Error(ServiceError::StaleRequest);
    }
    response
}

fn handle_local_profile_with_optional_root(
    request: &Request,
    root: Option<&std::fs::File>,
    owner: ryoku_profiles::ProfileOwner,
) -> ResponseBody {
    match (request, root) {
        (Request::GetLocalProfile, None) => ResponseBody::LocalProfile(default_local_profile()),
        (Request::MigrateLegacyLocalProfile, None) => ResponseBody::Error(ServiceError::NotFound),
        (_, Some(root)) => handle_local_profile_at_root(request, root, owner),
        _ => ResponseBody::Error(ServiceError::RecoveryUnavailable),
    }
}

fn handle_local_profile_at_root(
    request: &Request,
    root: &std::fs::File,
    owner: ryoku_profiles::ProfileOwner,
) -> ResponseBody {
    let document = match request {
        Request::GetLocalProfile => match ryoku_profiles::load_local_profile(root, owner) {
            Ok(document) => document,
            Err(ryoku_profiles::ProfileLoadError::Filesystem(Errno::NOENT)) => {
                default_local_profile()
            }
            Err(_) => return ResponseBody::Error(ServiceError::RecoveryUnavailable),
        },
        Request::SetLocalProfile { settings } => {
            match ryoku_profiles::load_local_profile(root, owner) {
                Ok(existing) if existing.id.as_str() != ryoku_profiles::GENERIC_PROFILE_ID => {
                    return ResponseBody::Error(ServiceError::Unsupported);
                }
                Ok(_) => (),
                Err(ryoku_profiles::ProfileLoadError::Filesystem(Errno::NOENT)) => (),
                Err(_) => return ResponseBody::Error(ServiceError::RecoveryUnavailable),
            }
            let document = default_local_profile_with(*settings);
            match ryoku_profiles::save_local_profile(root, owner, &document) {
                Ok(()) => document,
                Err(ryoku_profiles::ProfileSaveError::DirectorySyncFailed(_)) => {
                    return ResponseBody::Error(ServiceError::Failed);
                }
                Err(_) => return ResponseBody::Error(ServiceError::RecoveryUnavailable),
            }
        }
        Request::MigrateLegacyLocalProfile => {
            match ryoku_profiles::migrate_legacy_local_profile(root, owner) {
                Ok(document) => document,
                Err(ryoku_profiles::ProfileMigrationError::InvalidLegacy) => {
                    return ResponseBody::Error(ServiceError::Unsupported);
                }
                Err(ryoku_profiles::ProfileMigrationError::DestinationExists) => {
                    return ResponseBody::Error(ServiceError::StaleRequest);
                }
                Err(ryoku_profiles::ProfileMigrationError::Load(
                    ryoku_profiles::ProfileLoadError::Filesystem(Errno::NOENT),
                )) => return ResponseBody::Error(ServiceError::NotFound),
                Err(ryoku_profiles::ProfileMigrationError::Save(
                    ryoku_profiles::ProfileSaveError::DirectorySyncFailed(_),
                )) => return ResponseBody::Error(ServiceError::Failed),
                Err(_) => return ResponseBody::Error(ServiceError::RecoveryUnavailable),
            }
        }
        _ => unreachable!("local profile handler received a different request"),
    };
    if document.id.as_str() != ryoku_profiles::GENERIC_PROFILE_ID {
        return ResponseBody::Error(ServiceError::Unsupported);
    }
    ResponseBody::LocalProfile(document)
}

fn default_local_profile() -> ryoku_profiles::ProfileDocument {
    default_local_profile_with(ryoku_profiles::SettingsLayer::default())
}

fn default_local_profile_with(
    settings: ryoku_profiles::SettingsLayer,
) -> ryoku_profiles::ProfileDocument {
    ryoku_profiles::ProfileDocument::new(
        ryoku_profiles::ProfileId::new(ryoku_profiles::GENERIC_PROFILE_ID)
            .expect("the built-in generic profile id is valid"),
        settings,
    )
}

fn read_frame_until(stream: &mut UnixStream, deadline: Instant) -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 4096];
    loop {
        wait_ready(stream, PollFlags::IN, deadline)?;
        let count = match stream.read(&mut buffer) {
            Ok(0) => return Err(ProtocolError::Malformed("incomplete request frame")),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => return Err(ProtocolError::Io),
        };
        let newline = buffer[..count].iter().position(|byte| *byte == b'\n');
        let end = newline.map_or(count, |index| index + 1);
        if bytes.len() + end > crate::MAX_REQUEST_BYTES + 1 {
            return Err(ProtocolError::TooLarge);
        }
        if newline.is_some_and(|index| index + 1 != count) {
            return Err(ProtocolError::Malformed("trailing request data"));
        }
        bytes.extend_from_slice(&buffer[..end]);
        if newline.is_some() {
            return Ok(bytes);
        }
        if bytes.len() > crate::MAX_REQUEST_BYTES {
            return Err(ProtocolError::TooLarge);
        }
    }
}

fn write_response(
    stream: &mut UnixStream,
    response: Response,
    deadline: Instant,
) -> Result<(), ProtocolError> {
    let mut bytes = serde_json::to_vec(&response).map_err(|_| ProtocolError::Io)?;
    bytes.push(b'\n');
    let mut pending = bytes.as_slice();
    while !pending.is_empty() {
        wait_ready(stream, PollFlags::OUT, deadline)?;
        match stream.write(pending) {
            Ok(0) => return Err(ProtocolError::Io),
            Ok(count) => pending = &pending[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => return Err(ProtocolError::Io),
        }
    }
    Ok(())
}

fn wait_ready(
    stream: &UnixStream,
    interest: PollFlags,
    deadline: Instant,
) -> Result<(), ProtocolError> {
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or(ProtocolError::Io)?;
        let timeout = Timespec::try_from(remaining).map_err(|_| ProtocolError::Io)?;
        let mut descriptor = [PollFd::new(stream, interest)];
        match poll(&mut descriptor, Some(&timeout)) {
            Ok(0) => return Err(ProtocolError::Io),
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => continue,
            Err(_) => return Err(ProtocolError::Io),
        }
    }
}

/// Run using `/run/user/<kernel UID>` and the NSS account's fixed default
/// `.local/state/ryoku`. Environment variables cannot select a journal/socket.
pub fn run_native() -> Result<(), String> {
    let (paths, listener, file) = crate::runtime_paths::RuntimePaths::open()?;
    // Path validation happened before replay. Repeat after replay and before
    // each authenticated connection can query the in-memory operation state.
    let journal = OperationJournal::open(file).map_err(|_| "operation journal replay failed")?;
    paths.revalidate()?;
    serve(listener, journal, paths).map_err(|_| "service listener failed".into())
}

/// Compatibility entry point; all paths now come from native identity.
pub fn run_from_environment() -> Result<(), String> {
    run_native()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Envelope, ManagedAction, ManagedActionKind, PackageId, Request, ResponseBody};
    use std::thread;
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::fs::{MetadataExt, PermissionsExt},
    };

    #[test]
    fn local_profile_reads_defaults_and_saves_personal_settings_without_helper_or_checkpoint() {
        let root_path = std::env::temp_dir().join(format!(
            "ryoku-service-profile-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&root_path).unwrap();
        std::fs::set_permissions(&root_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = std::fs::File::open(&root_path).unwrap();
        let metadata = std::fs::metadata(&root_path).unwrap();
        let owner = ryoku_profiles::ProfileOwner { uid: metadata.uid(), gid: metadata.gid() };
        let journal_path = root_path.join("operations.log");
        let journal_file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&journal_path)
            .unwrap();
        let journal = OperationJournal::open(journal_file).unwrap();

        let defaults = dispatch_request(Request::GetLocalProfile, &journal, |request| {
            handle_local_profile_at_root(request, &root, owner)
        });
        let ResponseBody::LocalProfile(defaults) = defaults else {
            panic!("missing local profile did not resolve to defaults");
        };
        assert_eq!(defaults.id.as_str(), ryoku_profiles::GENERIC_PROFILE_ID);
        assert_eq!(defaults.settings, ryoku_profiles::SettingsLayer::default());
        assert!(!root_path.join(ryoku_profiles::LOCAL_PROFILE_FILE).exists());

        let settings = ryoku_profiles::SettingsLayer {
            desktop: Some(ryoku_profiles::Desktop::Kde),
            theme: Some(ryoku_profiles::Theme::System),
            density: Some(ryoku_profiles::Density::Compact),
        };
        let saved = dispatch_request(Request::SetLocalProfile { settings }, &journal, |request| {
            handle_local_profile_at_root(request, &root, owner)
        });
        let expected = default_local_profile_with(settings);
        assert_eq!(saved, ResponseBody::LocalProfile(expected.clone()));
        let profile_path = root_path.join(ryoku_profiles::LOCAL_PROFILE_FILE);
        assert_eq!(std::fs::metadata(&profile_path).unwrap().mode() & 0o777, 0o600);
        assert!(matches!(
            dispatch_request(Request::GetLocalProfile, &journal, |request| {
                handle_local_profile_at_root(request, &root, owner)
            }),
            ResponseBody::LocalProfile(document) if document == expected
        ));
        assert_eq!(std::fs::metadata(&journal_path).unwrap().len(), 0);

        drop(journal);
        drop(root);
        std::fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn missing_profile_directories_read_defaults_but_cannot_be_saved_without_creation() {
        let owner = ryoku_profiles::ProfileOwner { uid: 1000, gid: 1000 };
        assert_eq!(
            handle_local_profile_with_optional_root(&Request::GetLocalProfile, None, owner),
            ResponseBody::LocalProfile(default_local_profile())
        );
        assert_eq!(
            handle_local_profile_with_optional_root(
                &Request::SetLocalProfile { settings: ryoku_profiles::SettingsLayer::default() },
                None,
                owner,
            ),
            ResponseBody::Error(ServiceError::RecoveryUnavailable)
        );
        assert_eq!(
            handle_local_profile_with_optional_root(
                &Request::MigrateLegacyLocalProfile,
                None,
                owner
            ),
            ResponseBody::Error(ServiceError::NotFound)
        );
    }

    #[test]
    fn local_profile_write_refuses_corrupt_or_foreign_profile_documents() {
        let root_path = std::env::temp_dir().join(format!(
            "ryoku-service-profile-preserve-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&root_path).unwrap();
        std::fs::set_permissions(&root_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = std::fs::File::open(&root_path).unwrap();
        let metadata = std::fs::metadata(&root_path).unwrap();
        let owner = ryoku_profiles::ProfileOwner { uid: metadata.uid(), gid: metadata.gid() };
        let profile_path = root_path.join(ryoku_profiles::LOCAL_PROFILE_FILE);
        let requested = Request::SetLocalProfile {
            settings: ryoku_profiles::SettingsLayer {
                theme: Some(ryoku_profiles::Theme::RyokuLight),
                ..ryoku_profiles::SettingsLayer::default()
            },
        };

        let malformed = b"not a recognized profile document";
        std::fs::write(&profile_path, malformed).unwrap();
        std::fs::set_permissions(&profile_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            handle_local_profile_at_root(&requested, &root, owner),
            ResponseBody::Error(ServiceError::RecoveryUnavailable)
        ));
        assert_eq!(std::fs::read(&profile_path).unwrap(), malformed);

        let other = ryoku_profiles::ProfileDocument::new(
            ryoku_profiles::ProfileId::new("workstation").unwrap(),
            ryoku_profiles::SettingsLayer::default(),
        )
        .canonical_bytes()
        .unwrap();
        std::fs::write(&profile_path, &other).unwrap();
        assert!(matches!(
            handle_local_profile_at_root(&requested, &root, owner),
            ResponseBody::Error(ServiceError::Unsupported)
        ));
        assert_eq!(std::fs::read(&profile_path).unwrap(), other);
        assert_eq!(std::fs::read_dir(&root_path).unwrap().count(), 1);

        drop(root);
        std::fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn legacy_personal_profile_migration_is_local_atomic_and_preserves_its_source() {
        let root_path = std::env::temp_dir().join(format!(
            "ryoku-service-profile-migration-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&root_path).unwrap();
        std::fs::set_permissions(&root_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = std::fs::File::open(&root_path).unwrap();
        let metadata = std::fs::metadata(&root_path).unwrap();
        let owner = ryoku_profiles::ProfileOwner { uid: metadata.uid(), gid: metadata.gid() };
        let legacy = ryoku_profiles::ProfileDocument::new(
            ryoku_profiles::ProfileId::new(ryoku_profiles::GENERIC_PROFILE_ID).unwrap(),
            ryoku_profiles::SettingsLayer {
                theme: Some(ryoku_profiles::Theme::System),
                density: Some(ryoku_profiles::Density::Compact),
                ..ryoku_profiles::SettingsLayer::default()
            },
        )
        .canonical_bytes()
        .unwrap();
        let legacy_path = root_path.join("profile.json");
        std::fs::write(&legacy_path, &legacy).unwrap();
        std::fs::set_permissions(&legacy_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let journal_path = root_path.join("operations.log");
        let journal_file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&journal_path)
            .unwrap();
        let journal = OperationJournal::open(journal_file).unwrap();

        let response = dispatch_request(Request::MigrateLegacyLocalProfile, &journal, |request| {
            handle_local_profile_at_root(request, &root, owner)
        });
        assert_eq!(
            response,
            ResponseBody::LocalProfile(ryoku_profiles::ProfileDocument::parse(&legacy).unwrap())
        );
        assert_eq!(std::fs::read(&legacy_path).unwrap(), legacy);
        assert_eq!(
            std::fs::read(root_path.join(ryoku_profiles::LOCAL_PROFILE_FILE)).unwrap(),
            legacy
        );
        assert_eq!(
            handle_local_profile_at_root(&Request::MigrateLegacyLocalProfile, &root, owner),
            ResponseBody::LocalProfile(ryoku_profiles::ProfileDocument::parse(&legacy).unwrap())
        );
        assert_eq!(std::fs::read(&legacy_path).unwrap(), legacy);
        assert_eq!(std::fs::metadata(&journal_path).unwrap().len(), 0);

        drop(journal);
        drop(root);
        std::fs::remove_dir_all(root_path).unwrap();
    }

    #[test]
    fn expired_socket_deadlines_fail_before_reading_or_writing() {
        let (_client, mut server) = crate::test_unix_pair_or_skip!();
        server.set_nonblocking(true).unwrap();
        let expired = Instant::now() - Duration::from_millis(1);
        assert_eq!(read_frame_until(&mut server, expired), Err(ProtocolError::Io));
        assert_eq!(
            write_response(
                &mut server,
                Response {
                    version: PROTOCOL_VERSION,
                    request_id: RequestId::new("expired").unwrap(),
                    body: ResponseBody::Error(ServiceError::Unsupported),
                },
                expired,
            ),
            Err(ProtocolError::Io)
        );
    }

    #[test]
    fn dispatch_routes_linked_verification_without_journaling() {
        let journal_path = std::env::temp_dir()
            .join(format!("ryoku-dispatch-verification-{}.log", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&journal_path)
            .unwrap();
        let journal = OperationJournal::open(file).unwrap();
        let request = Request::VerifyLinkedCheckpoint {
            transaction_id: "txn-1".into(),
            generation_id: "generation-1".into(),
        };
        let mut invoked = false;
        let response = dispatch_request(request, &journal, |request| {
            invoked = true;
            assert!(matches!(request, Request::VerifyLinkedCheckpoint { .. }));
            ResponseBody::LinkedCheckpoint(crate::helper_protocol::VerifiedLinkedCheckpoint {
                transaction_id: "txn-1".into(),
                generation_id: "generation-1".into(),
                manifest_sha256: "a".repeat(64),
                snapper_config: "root".into(),
                snapshot_id: "42".into(),
                filesystem_uuid: "01234567-89ab-cdef-0123-456789abcdef".into(),
            })
        });
        assert!(invoked);
        assert!(matches!(response, ResponseBody::LinkedCheckpoint(_)));
        assert_eq!(std::fs::metadata(&journal_path).unwrap().len(), 0);
        drop(journal);
        std::fs::remove_file(journal_path).unwrap();
    }

    #[test]
    fn discovery_reflects_helper_reachability_without_advertising_writes() {
        let path = std::env::temp_dir()
            .join(format!("ryoku-dispatch-discovery-{}.log", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let journal = OperationJournal::open(file).unwrap();
        let mut probed = false;
        let response = dispatch_request(Request::Discover, &journal, |request| {
            probed = true;
            assert!(matches!(request, Request::Start { action: ManagedAction::CreateCheckpoint }));
            // A healthy helper rejects this still-disabled action with the
            // typed Unsupported result after validating protected state.
            ResponseBody::Error(ServiceError::Unsupported)
        });
        assert!(probed);
        let ResponseBody::Discovery(discovery) = response else {
            panic!("unexpected discovery response");
        };
        assert_eq!(discovery.service_state, crate::ServiceState::Ready);
        assert!(discovery.supported_actions.is_empty());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);

        let unavailable = dispatch_request(Request::Discover, &journal, |_| {
            ResponseBody::Error(ServiceError::RecoveryUnavailable)
        });
        let ResponseBody::Discovery(unavailable) = unavailable else {
            panic!("unexpected unavailable discovery response");
        };
        assert_eq!(unavailable.service_state, crate::ServiceState::RecoveryUnavailable);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    fn exchange(journal: &mut OperationJournal, request: Request) -> Option<ResponseBody> {
        let unavailable_socket =
            std::env::temp_dir().join(format!("ryoku-no-helper-{}.sock", std::process::id()));
        exchange_with_helper_path(journal, request, &unavailable_socket)
    }

    fn exchange_with_helper_path(
        journal: &mut OperationJournal,
        request: Request,
        helper_socket_path: &Path,
    ) -> Option<ResponseBody> {
        if !crate::test_unix_sockets_available() {
            eprintln!(
                "skipping Unix service exchange: socket I/O or SO_PEERCRED denied by sandbox"
            );
            return None;
        }
        let (server, client) = match UnixStream::pair() {
            Ok(pair) => pair,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping Unix service exchange: socketpair denied by sandbox");
                return None;
            }
            Err(error) => panic!("UnixStream pair failed unexpectedly: {error}"),
        };
        let uid = rustix::process::geteuid().as_raw();
        let envelope = Envelope {
            version: PROTOCOL_VERSION,
            request_id: RequestId::new("test").unwrap(),
            request,
        };
        let mut client = client;
        serde_json::to_writer(&mut client, &envelope).unwrap();
        client.write_all(b"\n").unwrap();
        serve_connection_with_helper_path(server, journal, uid, helper_socket_path).unwrap();
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).unwrap();
        Some(serde_json::from_str::<Response>(&line).unwrap().body)
    }

    #[test]
    fn discovery_and_start_are_live_but_provider_actions_stay_unavailable() {
        let path = std::env::temp_dir().join(format!("ryoku-runtime-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let Some(discovery_response) = exchange(&mut journal, Request::Discover) else {
            return;
        };
        match discovery_response {
            ResponseBody::Discovery(discovery) => {
                assert!(discovery.supported_actions.is_empty());
                assert_eq!(discovery.service_state, crate::ServiceState::RecoveryUnavailable);
            }
            other => panic!("unexpected response: {other:?}"),
        }
        let action = ManagedAction::InstallPackage { package: PackageId::new("firefox").unwrap() };
        let Some(start_response) = exchange(&mut journal, Request::Start { action }) else {
            return;
        };
        assert_eq!(start_response, ResponseBody::Error(ServiceError::RecoveryUnavailable));
        assert_eq!(journal.get(&crate::OperationId::new("not-created").unwrap()), None);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unresolved_operation_blocks_new_start_without_contacting_helper() {
        let path =
            std::env::temp_dir().join(format!("ryoku-runtime-pending-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        journal
            .record(crate::OperationStatus {
                operation_id: crate::OperationId::new("pending-operation").unwrap(),
                action: crate::ManagedActionKind::CreateCheckpoint,
                phase: crate::OperationPhase::Accepted,
                progress: None,
                error: None,
            })
            .unwrap();

        let mut helper_called = false;
        let response = dispatch_request(
            Request::Start {
                action: crate::ManagedAction::InstallPackage {
                    package: crate::PackageId::new("firefox").unwrap(),
                },
            },
            &journal,
            |_| {
                helper_called = true;
                ResponseBody::Error(ServiceError::Unsupported)
            },
        );
        assert_eq!(response, ResponseBody::Error(ServiceError::RecoveryUnavailable));
        assert!(!helper_called);

        let mut verification_called = false;
        let verification = dispatch_request(
            Request::VerifyLinkedCheckpoint {
                transaction_id: "source-transaction".into(),
                generation_id: "source-generation".into(),
            },
            &journal,
            |_| {
                verification_called = true;
                ResponseBody::LinkedCheckpoint(crate::helper_protocol::VerifiedLinkedCheckpoint {
                    transaction_id: "source-transaction".into(),
                    generation_id: "source-generation".into(),
                    manifest_sha256: "a".repeat(64),
                    snapper_config: "root".into(),
                    snapshot_id: "42".into(),
                    filesystem_uuid: "01234567-89ab-cdef-0123-456789abcdef".into(),
                })
            },
        );
        assert!(verification_called);
        assert!(matches!(verification, ResponseBody::LinkedCheckpoint(_)));

        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn user_service_probes_helper_and_rejection_does_not_write_journal() {
        let _pair_probe = crate::test_unix_pair_or_skip!();
        let socket_path = std::env::temp_dir()
            .join(format!("ryoku-helper-integration-{}.sock", std::process::id()));
        let journal_path = std::env::temp_dir()
            .join(format!("ryoku-helper-integration-{}.log", std::process::id()));
        let listener = crate::test_unix_listener_or_skip!(&socket_path);
        let helper_thread = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            crate::helper_protocol::serve_operator_helper_connection(
                stream,
                rustix::process::getegid().as_raw(),
            )
            .unwrap();
        });

        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&journal_path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let action = ManagedAction::InstallPackage { package: PackageId::new("firefox").unwrap() };
        let expected = if rustix::process::geteuid().as_raw() == 0 {
            ServiceError::Unauthorized
        } else {
            ServiceError::Unsupported
        };
        assert_eq!(
            exchange_with_helper_path(&mut journal, Request::Start { action }, &socket_path,),
            Some(ResponseBody::Error(expected))
        );
        helper_thread.join().unwrap();
        assert_eq!(std::fs::metadata(&journal_path).unwrap().len(), 0);
        drop(journal);
        std::fs::remove_file(journal_path).unwrap();
        std::fs::remove_file(socket_path).unwrap();
    }

    #[test]
    fn user_service_returns_only_correlated_linked_checkpoint_evidence() {
        let _pair_probe = crate::test_unix_pair_or_skip!();
        let socket_path =
            std::env::temp_dir().join(format!("ryoku-linked-helper-{}.sock", std::process::id()));
        let journal_path =
            std::env::temp_dir().join(format!("ryoku-linked-helper-{}.log", std::process::id()));
        let listener = crate::test_unix_listener_or_skip!(&socket_path);
        let helper_thread = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request =
                crate::helper_protocol::read_helper_envelope(&mut BufReader::new(&mut stream))
                    .unwrap();
            assert_eq!(
                request.request,
                crate::HelperRequest::VerifyLinkedCheckpoint {
                    transaction_id: "txn-1".into(),
                    generation_id: "generation-1".into(),
                }
            );
            let response = crate::helper_protocol::HelperResponse {
                version: crate::helper_protocol::HELPER_PROTOCOL_VERSION,
                request_id: request.request_id,
                outcome: crate::helper_protocol::HelperOutcome::VerifiedLinkedCheckpoint(
                    crate::helper_protocol::VerifiedLinkedCheckpoint {
                        transaction_id: "txn-1".into(),
                        generation_id: "generation-1".into(),
                        manifest_sha256: "a".repeat(64),
                        snapper_config: "root".into(),
                        snapshot_id: "42".into(),
                        filesystem_uuid: "01234567-89ab-cdef-0123-456789abcdef".into(),
                    },
                ),
            };
            serde_json::to_writer(&mut stream, &response).unwrap();
            stream.write_all(b"\n").unwrap();
        });

        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&journal_path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let response = exchange_with_helper_path(
            &mut journal,
            Request::VerifyLinkedCheckpoint {
                transaction_id: "txn-1".into(),
                generation_id: "generation-1".into(),
            },
            &socket_path,
        )
        .expect("socketpair was probed before helper thread creation");
        assert_eq!(
            response,
            ResponseBody::LinkedCheckpoint(crate::helper_protocol::VerifiedLinkedCheckpoint {
                transaction_id: "txn-1".into(),
                generation_id: "generation-1".into(),
                manifest_sha256: "a".repeat(64),
                snapper_config: "root".into(),
                snapshot_id: "42".into(),
                filesystem_uuid: "01234567-89ab-cdef-0123-456789abcdef".into(),
            })
        );
        helper_thread.join().unwrap();
        assert_eq!(std::fs::metadata(&journal_path).unwrap().len(), 0);
        drop(journal);
        std::fs::remove_file(journal_path).unwrap();
        std::fs::remove_file(socket_path).unwrap();
    }

    #[test]
    fn reconnect_lookup_returns_state_after_journal_reopen() {
        let path =
            std::env::temp_dir().join(format!("ryoku-runtime-journal-{}", std::process::id()));
        let id = crate::OperationId::new("durable-op").unwrap();
        let accepted = crate::OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::CreateCheckpoint,
            phase: crate::OperationPhase::Accepted,
            progress: None,
            error: None,
        };
        let capturing = crate::OperationStatus {
            operation_id: id.clone(),
            action: ManagedActionKind::CreateCheckpoint,
            phase: crate::OperationPhase::Capturing,
            progress: Some(crate::Progress { completed: 2, total: Some(3) }),
            error: None,
        };

        // Simulate the service stopping after a durable write, then starting
        // again and replaying the on-disk journal before a client reconnects.
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        journal.record(accepted).unwrap();
        journal
            .record(crate::OperationStatus {
                operation_id: id.clone(),
                action: ManagedActionKind::CreateCheckpoint,
                phase: crate::OperationPhase::Capturing,
                progress: Some(crate::Progress { completed: 1, total: Some(3) }),
                error: None,
            })
            .unwrap();
        journal.record(capturing.clone()).unwrap();
        drop(journal);

        let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let restarted_journal = OperationJournal::open(file).unwrap();
        assert_eq!(restarted_journal.get(&id), Some(&capturing));
        let response = dispatch_request(
            Request::GetOperation { operation_id: id },
            &restarted_journal,
            |_| unreachable!("status lookup does not contact the helper"),
        );
        assert_eq!(response, ResponseBody::Operation(capturing));
        drop(restarted_journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn reconnect_replays_uncertain_journal_before_lookup_or_fails_closed() {
        for (label, tail, expected_error) in
            [("torn", &b"{\"version\":1,"[..], false), ("corrupt", &b"not-json\n"[..], true)]
        {
            let path = std::env::temp_dir()
                .join(format!("ryoku-runtime-reconnect-{label}-{}", std::process::id()));
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            let mut journal = OperationJournal::open(file).unwrap();
            let id = crate::OperationId::new("durable-before-error").unwrap();
            let durable = crate::OperationStatus {
                operation_id: id.clone(),
                action: ManagedActionKind::CreateCheckpoint,
                phase: crate::OperationPhase::Accepted,
                progress: None,
                error: None,
            };
            journal.record(durable.clone()).unwrap();
            let valid_len = std::fs::metadata(&path).unwrap().len();
            let mut interrupted = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
            interrupted.write_all(tail).unwrap();
            interrupted.sync_all().unwrap();
            drop(interrupted);
            journal.simulate_uncertain_write_failure();

            let Some(response) = exchange(&mut journal, Request::GetOperation { operation_id: id })
            else {
                return;
            };
            if expected_error {
                assert_eq!(response, ResponseBody::Error(ServiceError::RecoveryUnavailable));
                assert!(journal.get(&durable.operation_id).is_some());
            } else {
                assert_eq!(response, ResponseBody::Operation(durable));
                assert_eq!(std::fs::metadata(&path).unwrap().len(), valid_len);
            }
            drop(journal);
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn wrong_peer_uid_and_malformed_frames_get_structured_errors() {
        let path = std::env::temp_dir().join(format!("ryoku-runtime-deny-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        let (server, mut client) = crate::test_unix_pair_or_skip!();
        // A rejected UID is answered before parsing, even if its bytes are
        // malformed. The untrusted request ID is not reflected.
        client.write_all(b"not-json\n").unwrap();
        assert_eq!(serve_connection(server, &mut journal, u32::MAX), Ok(()));
        let mut line = String::new();
        BufReader::new(&mut client).read_line(&mut line).unwrap();
        let response: Response = serde_json::from_str(&line).unwrap();
        assert_eq!(response.request_id.as_str(), "invalid");
        assert_eq!(response.body, ResponseBody::Error(ServiceError::Unauthorized));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);

        let (server, mut client) = crate::test_unix_pair_or_skip!();
        client.write_all(b"not-json\n").unwrap();
        serve_connection(server, &mut journal, rustix::process::geteuid().as_raw()).unwrap();
        line.clear();
        BufReader::new(&mut client).read_line(&mut line).unwrap();
        assert!(matches!(
            serde_json::from_str::<Response>(&line).unwrap().body,
            ResponseBody::Error(ServiceError::Malformed(_))
        ));
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn peer_denial_precedes_path_work_and_stale_paths_precede_parsing_or_lookup() {
        if !crate::test_unix_sockets_available() {
            eprintln!("skipping Unix socket test: socket I/O or SO_PEERCRED denied by sandbox");
            return;
        }
        let path = std::env::temp_dir().join(format!("ryoku-runtime-stale-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut journal = OperationJournal::open(file).unwrap();
        for unauthorized in [true, false] {
            let (server, mut client) = crate::test_unix_pair_or_skip!();
            client.write_all(b"not-json\n").unwrap();
            serve_connection_checked(
                server,
                &mut journal,
                if unauthorized { u32::MAX } else { rustix::process::geteuid().as_raw() },
                Path::new("/unavailable-helper"),
                || {
                    assert!(!unauthorized, "denied peer reached namespace validation");
                    Err(ServiceError::RecoveryUnavailable)
                },
            )
            .unwrap();
            let mut line = String::new();
            BufReader::new(client).read_line(&mut line).unwrap();
            let response: Response = serde_json::from_str(&line).unwrap();
            assert_eq!(response.request_id.as_str(), "invalid");
            assert_eq!(
                response.body,
                ResponseBody::Error(if unauthorized {
                    ServiceError::Unauthorized
                } else {
                    ServiceError::RecoveryUnavailable
                })
            );
        }
        let checks = std::cell::Cell::new(0);
        let (server, mut client) = crate::test_unix_pair_or_skip!();
        serde_json::to_writer(
            &mut client,
            &Envelope {
                version: PROTOCOL_VERSION,
                request_id: RequestId::new("must-not-reflect").unwrap(),
                request: Request::Discover,
            },
        )
        .unwrap();
        client.write_all(b"\n").unwrap();
        serve_connection_checked(
            server,
            &mut journal,
            rustix::process::geteuid().as_raw(),
            Path::new("/unavailable-helper"),
            || {
                checks.set(checks.get() + 1);
                if checks.get() == 1 { Ok(()) } else { Err(ServiceError::RecoveryUnavailable) }
            },
        )
        .unwrap();
        assert_eq!(checks.get(), 2);
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).unwrap();
        let response: Response = serde_json::from_str(&line).unwrap();
        assert_eq!(response.request_id.as_str(), "invalid");
        assert_eq!(response.body, ResponseBody::Error(ServiceError::RecoveryUnavailable));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);
        drop(journal);
        std::fs::remove_file(path).unwrap();
    }
}
