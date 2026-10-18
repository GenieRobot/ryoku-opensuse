//! Read-only session-start gate backed by the persistent recovery journal.
//!
//! This endpoint discloses only whether Ryoku may start. It accepts no
//! transaction IDs, paths, or mutation requests and treats missing or invalid
//! recovery state as unavailable.

#[cfg(test)]
use std::io::{BufRead, BufReader, BufWriter};
use std::{
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
};

use crate::{
    PeerIdentity, ProtocolError,
    mutation_gate::{GateError, MutationLock},
};

pub const RECOVERY_GATE_SOCKET_PATH: &str = "/run/ryoku/recovery-gate.sock";
const REQUEST: &[u8] = b"RYOKU-RECOVERY-GATE/1\n";
const MAX_REQUEST_BYTES: usize = 64;
const MAX_ACTIVE_SESSIONS: usize = 32;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryGateState {
    Clear,
    RecoveryRequired,
    Unavailable,
}

impl RecoveryGateState {
    fn wire(self) -> &'static [u8] {
        match self {
            Self::Clear => b"clear\n",
            Self::RecoveryRequired => b"recovery-required\n",
            Self::Unavailable => b"unavailable\n",
        }
    }
}

/// Hold the system's common mutation lock in shared mode while reading
/// persistent recovery state. The guard is released when this check returns;
/// a desktop session must not pin the mutation lock for its whole lifetime.
fn inspect_recovery_gate() -> RecoveryGateState {
    let root = match std::fs::File::open("/") {
        Ok(root) => root,
        Err(_) => return RecoveryGateState::Unavailable,
    };
    let machine = match ryoku_recovery::machine_state::read_machine_state_record(&root) {
        Ok(machine) => machine,
        Err(_) => return RecoveryGateState::Unavailable,
    };
    let state = match ryoku_recovery::state_mount::collect_and_open_state_mount_readonly(
        &root,
        machine.identity(),
    ) {
        Ok((state, _)) => std::fs::File::from(state),
        Err(_) => return RecoveryGateState::Unavailable,
    };
    let lock = match state.try_clone().map_err(|_| RecoveryGateState::Unavailable).and_then(
        |state_for_lock| {
            MutationLock::open_existing(state_for_lock, 0)
                .map_err(|_| RecoveryGateState::Unavailable)
        },
    ) {
        Ok(lock) => lock,
        Err(state) => return state,
    };
    let _guard = match lock.try_acquire_shared() {
        Ok(lease) => lease,
        Err(GateError::Lock(std::io::ErrorKind::WouldBlock)) => {
            return RecoveryGateState::RecoveryRequired;
        }
        Err(_) => return RecoveryGateState::Unavailable,
    };
    let journals =
        match ryoku_recovery::restore::RestoreJournalStore::open_existing_state_directory_readonly(
            &state,
        ) {
            Ok(journals) => journals,
            Err(_) => return RecoveryGateState::Unavailable,
        };
    match journals.incomplete_attempts_under_mutation_lock() {
        Ok(attempts) if attempts.is_empty() => RecoveryGateState::Clear,
        Ok(_) => RecoveryGateState::RecoveryRequired,
        Err(_) => RecoveryGateState::Unavailable,
    }
}

/// Handle one unprivileged, local query. The kernel peer credentials are read
/// before accepting the fixed request; the response contains no attempt data.
pub fn serve_recovery_gate_connection(mut stream: UnixStream) -> Result<(), ProtocolError> {
    let deadline = Instant::now() + IO_TIMEOUT;
    stream.set_nonblocking(true).map_err(|_| ProtocolError::Io)?;
    let peer = PeerIdentity::from_unix_socket(&stream)?;
    if peer.uid() == 0 {
        return Err(ProtocolError::Malformed("recovery gate is for session callers"));
    }

    serve_recovery_gate_connection_until(&mut stream, deadline, inspect_recovery_gate)
}

fn serve_recovery_gate_connection_until(
    stream: &mut UnixStream,
    deadline: Instant,
    inspect: impl FnOnce() -> RecoveryGateState,
) -> Result<(), ProtocolError> {
    let mut bytes = Vec::with_capacity(REQUEST.len());
    let mut buffer = [0_u8; MAX_REQUEST_BYTES + 1];
    loop {
        wait_ready(stream, PollFlags::IN, deadline)?;
        let count = match stream.read(&mut buffer) {
            Ok(0) => return Err(ProtocolError::Malformed("incomplete recovery gate request")),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => return Err(ProtocolError::Io),
        };
        let newline = buffer[..count].iter().position(|byte| *byte == b'\n');
        let end = newline.map_or(count, |index| index + 1);
        if bytes.len() + end > MAX_REQUEST_BYTES || newline.is_some_and(|index| index + 1 != count)
        {
            return Err(ProtocolError::Malformed("invalid recovery gate request"));
        }
        bytes.extend_from_slice(&buffer[..end]);
        if newline.is_some() {
            break;
        }
    }
    if bytes.as_slice() != REQUEST {
        return Err(ProtocolError::Malformed("invalid recovery gate request"));
    }

    let response = inspect().wire();
    let mut pending = response;
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

#[cfg(test)]
fn serve_recovery_gate_request(
    stream: &mut (impl Read + Write),
    inspect: impl FnOnce() -> RecoveryGateState,
) -> Result<(), ProtocolError> {
    let mut bytes = Vec::with_capacity(REQUEST.len());
    let mut reader = BufReader::new(stream);
    reader
        .by_ref()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| ProtocolError::Io)?;
    if bytes.len() > MAX_REQUEST_BYTES || bytes.as_slice() != REQUEST || !reader.buffer().is_empty()
    {
        return Err(ProtocolError::Malformed("invalid recovery gate request"));
    }

    let state = inspect();
    let mut writer = BufWriter::new(reader.get_mut());
    writer.write_all(state.wire()).map_err(|_| ProtocolError::Io)?;
    writer.flush().map_err(|_| ProtocolError::Io)?;
    Ok(())
}

/// Serve the fixed systemd-activated listener. Only the dedicated socket unit
/// should expose this protocol; it is independent from the operator helper
/// socket and its privileged request language.
pub fn serve_recovery_gate_listener(listener: UnixListener) -> std::io::Result<()> {
    let active = Arc::new(AtomicUsize::new(0));
    for accepted in listener.incoming() {
        let stream = accepted?;
        if active
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_ACTIVE_SESSIONS).then_some(count + 1)
            })
            .is_err()
        {
            continue;
        }
        let active_slot = Arc::clone(&active);
        if thread::Builder::new()
            .name("ryoku-recovery-gate".into())
            .spawn(move || {
                let _slot = ActiveSession(Arc::clone(&active_slot));
                let _ = serve_recovery_gate_connection(stream);
            })
            .is_err()
        {
            active.fetch_sub(1, Ordering::AcqRel);
        }
    }
    Ok(())
}

struct ActiveSession(Arc<AtomicUsize>);
impl Drop for ActiveSession {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Validate an inherited systemd listener path without relying on environment
/// supplied by a client.
pub fn validate_recovery_gate_listener_path(path: &Path) -> Result<(), ProtocolError> {
    if path == Path::new(RECOVERY_GATE_SOCKET_PATH) {
        Ok(())
    } else {
        Err(ProtocolError::Malformed("unexpected recovery gate socket path"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    struct MemoryStream {
        input: std::io::Cursor<Vec<u8>>,
        output: Vec<u8>,
        hold_open_after_request: bool,
    }

    impl MemoryStream {
        fn request() -> Self {
            Self::with_input(REQUEST.to_vec())
        }

        fn with_input(input: Vec<u8>) -> Self {
            Self {
                input: std::io::Cursor::new(input),
                output: Vec::new(),
                hold_open_after_request: true,
            }
        }
    }

    impl Read for MemoryStream {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if self.hold_open_after_request
                && self.input.position() >= self.input.get_ref().len() as u64
            {
                return Err(std::io::Error::from(std::io::ErrorKind::WouldBlock));
            }
            self.input.read(output)
        }
    }

    impl Write for MemoryStream {
        fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
            self.output.extend_from_slice(input);
            Ok(input.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn unix_stream_writes_are_available() -> bool {
        let (mut client, _server) = match UnixStream::pair() {
            Ok(pair) => pair,
            Err(error) if error.raw_os_error() == Some(1) => {
                eprintln!("skipping UnixStream transport assertion: socketpair denied with EPERM");
                return false;
            }
            Err(error) => panic!("UnixStream pair probe failed: {error}"),
        };
        match client.write(b"probe") {
            Ok(5) => true,
            Ok(_) => panic!("short probe write to UnixStream pair"),
            Err(error) if error.raw_os_error() == Some(1) => {
                eprintln!("skipping UnixStream transport assertion: write denied with EPERM");
                false
            }
            Err(error) => panic!("UnixStream write probe failed: {error}"),
        }
    }

    #[test]
    fn clear_response_is_a_one_shot_check_and_releases_before_the_session_runs() {
        let mut stream = MemoryStream::request();
        serve_recovery_gate_request(&mut stream, || RecoveryGateState::Clear).unwrap();
        assert_eq!(stream.output, b"clear\n");
    }

    #[test]
    fn denied_response_is_one_shot_and_contains_no_attempt_data() {
        let mut stream = MemoryStream::request();
        serve_recovery_gate_request(&mut stream, || RecoveryGateState::RecoveryRequired).unwrap();
        assert_eq!(stream.output, b"recovery-required\n");
    }

    #[test]
    fn invalid_or_pipelined_requests_are_rejected_before_inspecting_recovery_state() {
        for input in [
            b"RYOKU-RECOVERY-GATE/2\n".to_vec(),
            [REQUEST, b"second-request\n"].concat(),
            vec![b'x'; MAX_REQUEST_BYTES + 1],
        ] {
            let mut stream = MemoryStream::with_input(input);
            let inspected = std::cell::Cell::new(false);
            assert!(
                serve_recovery_gate_request(&mut stream, || {
                    inspected.set(true);
                    RecoveryGateState::Clear
                })
                .is_err()
            );
            assert!(!inspected.get());
            assert!(stream.output.is_empty());
        }
    }

    #[test]
    fn expired_connection_deadline_refuses_before_inspecting_recovery_state() {
        let (_client, mut server) = crate::test_unix_pair_or_skip!();
        server.set_nonblocking(true).unwrap();
        let inspected = std::cell::Cell::new(false);
        let deadline = Instant::now() - Duration::from_millis(1);
        assert_eq!(
            serve_recovery_gate_connection_until(&mut server, deadline, || {
                inspected.set(true);
                RecoveryGateState::Clear
            }),
            Err(ProtocolError::Io)
        );
        assert!(!inspected.get());
    }

    #[test]
    fn trickled_request_cannot_hold_a_gate_worker_past_its_deadline() {
        if !unix_stream_writes_are_available() {
            return;
        }
        let (mut client, mut server) = crate::test_unix_pair_or_skip!();
        server.set_nonblocking(true).unwrap();
        let worker = thread::spawn(move || {
            for byte in REQUEST {
                if client.write_all(std::slice::from_ref(byte)).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        let inspected = std::cell::Cell::new(false);
        let started = Instant::now();
        assert_eq!(
            serve_recovery_gate_connection_until(
                &mut server,
                started + Duration::from_millis(250),
                || {
                    inspected.set(true);
                    RecoveryGateState::Clear
                }
            ),
            Err(ProtocolError::Io)
        );
        assert!(!inspected.get());
        assert!(started.elapsed() < Duration::from_millis(700));
        drop(server);
        worker.join().unwrap();
    }
}
