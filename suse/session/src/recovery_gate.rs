//! Fixed, read-only check before starting a Ryoku login session.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::sockopt::{socket_error, socket_peercred},
    net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with},
};

const SOCKET: &str = "/run/ryoku/recovery-gate.sock";
const REQUEST: &[u8] = b"RYOKU-RECOVERY-GATE/1\n";
const MAX_RESPONSE_BYTES: usize = 64;
const TIMEOUT: Duration = Duration::from_secs(5);

fn validate_response(response: &[u8]) -> Result<(), &'static str> {
    match response {
        b"clear\n" => Ok(()),
        b"recovery-required\n" => {
            Err("Ryoku recovery is unfinished; use the independent recovery console")
        }
        b"unavailable\n" => {
            Err("Ryoku recovery state cannot be verified; refusing session startup")
        }
        _ => Err("Ryoku recovery gate returned an invalid response"),
    }
}

pub fn ensure_clear() -> Result<(), String> {
    let deadline = Instant::now() + TIMEOUT;
    let mut stream = connect_until(Path::new(SOCKET), deadline)
        .map_err(|_| "Ryoku recovery state is unavailable; refusing session startup")?;

    let peer =
        socket_peercred(&stream).map_err(|_| "Ryoku recovery gate identity is unavailable")?;
    if peer.uid.as_raw() != 0 {
        return Err("Ryoku recovery gate is not the trusted system service".into());
    }

    let response = exchange(&mut stream, deadline)?;
    validate_response(&response).map_err(str::to_owned)
}

fn exchange(stream: &mut UnixStream, deadline: Instant) -> Result<Vec<u8>, String> {
    stream.set_nonblocking(true).map_err(|_| "Ryoku recovery gate is unavailable")?;
    let mut pending = REQUEST;
    while !pending.is_empty() {
        wait_ready(stream, PollFlags::OUT, deadline)?;
        match stream.write(pending) {
            Ok(0) => return Err("Ryoku recovery gate is unavailable".into()),
            Ok(count) => pending = &pending[count..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => return Err("Ryoku recovery gate is unavailable".into()),
        }
    }
    let mut response = Vec::with_capacity(16);
    let mut buffer = [0_u8; MAX_RESPONSE_BYTES + 1];
    loop {
        wait_ready(stream, PollFlags::IN, deadline)?;
        let count = match stream.read(&mut buffer) {
            Ok(0) => return Err("Ryoku recovery gate returned an invalid response".into()),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(_) => return Err("Ryoku recovery gate is unavailable".into()),
        };
        let newline = buffer[..count].iter().position(|byte| *byte == b'\n');
        let end = newline.map_or(count, |index| index + 1);
        if newline.is_some_and(|index| index + 1 != count)
            || response.len() + end > MAX_RESPONSE_BYTES
        {
            return Err("Ryoku recovery gate returned an invalid response".into());
        }
        response.extend_from_slice(&buffer[..end]);
        if newline.is_some() {
            break;
        }
    }
    Ok(response)
}

fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| "Ryoku recovery gate timed out".into())
}

fn wait_ready(stream: &UnixStream, interest: PollFlags, deadline: Instant) -> Result<(), String> {
    loop {
        let timeout = Timespec::try_from(remaining(deadline)?)
            .map_err(|_| "Ryoku recovery gate timed out")?;
        let mut descriptor = [PollFd::new(stream, interest)];
        match poll(&mut descriptor, Some(&timeout)) {
            Ok(0) => return Err("Ryoku recovery gate timed out".into()),
            Ok(_) => return Ok(()),
            Err(Errno::INTR) => continue,
            Err(_) => return Err("Ryoku recovery gate is unavailable".into()),
        }
    }
}

fn connect_until(path: &Path, deadline: Instant) -> Result<UnixStream, &'static str> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC.union(SocketFlags::NONBLOCK),
        None,
    )
    .map_err(|_| "socket creation failed")?;
    let address = SocketAddrUnix::new(path).map_err(|_| "socket address is invalid")?;
    match connect(&socket, &address) {
        Ok(()) => {}
        Err(Errno::INPROGRESS | Errno::AGAIN) => loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|duration| !duration.is_zero())
                .ok_or("connect timed out")?;
            let timeout = Timespec::try_from(remaining).map_err(|_| "invalid timeout")?;
            let mut descriptor = [PollFd::new(&socket, PollFlags::OUT)];
            match poll(&mut descriptor, Some(&timeout)) {
                Ok(0) => return Err("connect timed out"),
                Ok(_) => match socket_error(&socket).map_err(|_| "connect status unavailable")? {
                    Ok(()) => break,
                    Err(_) => return Err("connect failed"),
                },
                Err(Errno::INTR) => continue,
                Err(_) => return Err("connect failed"),
            }
        },
        Err(_) => return Err("connect failed"),
    }
    if Instant::now() >= deadline {
        return Err("connect timed out");
    }
    Ok(UnixStream::from(socket))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_clear_allows_startup() {
        assert_eq!(validate_response(b"clear\n"), Ok(()));
        assert!(validate_response(b"recovery-required\n").is_err());
        assert!(validate_response(b"unavailable\n").is_err());
        assert!(validate_response(b"clear\nextra\n").is_err());
        assert!(validate_response(b"").is_err());
    }

    fn unix_stream_writes_are_available() -> bool {
        let (mut client, _server) = UnixStream::pair().unwrap();
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
    fn slow_gate_response_cannot_extend_the_end_to_end_deadline() {
        use std::{
            io::{BufRead, BufReader},
            thread,
        };

        if !unix_stream_writes_are_available() {
            return;
        }
        let (mut server, mut client) = UnixStream::pair().unwrap();
        let worker = thread::spawn(move || {
            let mut request = String::new();
            BufReader::new(server.try_clone().unwrap()).read_line(&mut request).unwrap();
            if !request.is_empty() {
                for _ in 0..20 {
                    if server.write_all(b" ").is_err() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
            request
        });

        let started = Instant::now();
        let result = exchange(&mut client, started + Duration::from_millis(250));
        assert_eq!(result, Err("Ryoku recovery gate timed out".into()));
        assert!(started.elapsed() < Duration::from_millis(700));
        drop(client);
        let request = worker.join().unwrap();
        assert_eq!(request.as_bytes(), REQUEST);
    }

    #[test]
    fn expired_deadline_is_rejected_before_socket_io() {
        let (_server, mut client) = UnixStream::pair().unwrap();
        assert_eq!(
            exchange(&mut client, Instant::now() - Duration::from_millis(1)),
            Err("Ryoku recovery gate timed out".into())
        );
    }
}
