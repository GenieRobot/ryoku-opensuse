//! Manual input/capture aid for issue #25's explicitly owned disposable VM.
//! Native session inspection and image review remain separate acceptance gates.
#[path = "../lab/qmp.rs"]
mod qmp;

use std::{
    error::Error,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    path::Path,
    thread,
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const MARKER: &[u8] = b"issue25-disposable-overlay\n";

fn guard(run: &Path) -> Result<()> {
    if std::env::var("RYOKU_LAB_AUTHORIZED").as_deref() != Ok("existing-wsl-vm") {
        return Err("requires the explicitly authorized existing WSL fixture".into());
    }
    let uid = rustix::process::getuid().as_raw();
    let directory = fs::symlink_metadata(run)?;
    let marker = fs::symlink_metadata(run.join("issue25-owner"))?;
    let socket = fs::symlink_metadata(run.join("qmp.sock"))?;
    if !run.is_absolute()
        || !directory.is_dir()
        || directory.uid() != uid
        || directory.mode() & 0o777 != 0o700
        || !marker.is_file()
        || marker.uid() != uid
        || marker.mode() & 0o777 != 0o600
        || fs::read(run.join("issue25-owner"))? != MARKER
        || !socket.file_type().is_socket()
        || socket.uid() != uid
    {
        return Err("requires a private, owned, marked issue25 QMP run directory".into());
    }
    Ok(())
}

fn password(bytes: &[u8]) -> Result<&[u8]> {
    if bytes.len() == 33 && bytes[32] == b'\n' && bytes[..32].iter().all(u8::is_ascii_lowercase) {
        Ok(&bytes[..32])
    } else {
        Err("requires exactly 32 lowercase fixture-password bytes and newline".into())
    }
}

fn run(args: &[String]) -> Result<()> {
    let [directory, action, rest @ ..] = args else {
        return Err(
            "usage: session-lab-control RUN password|login|capture FILE.ppm|keys KEY...".into()
        );
    };
    let run = Path::new(directory);
    guard(run)?;
    if action == "password" && rest.is_empty() {
        let mut random = [0; 32];
        fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        for byte in &mut random {
            *byte = b'a' + *byte % 26;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(run.join("password-private"))?;
        file.write_all(&random)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        random.fill(0);
        return Ok(());
    }
    let mut control = qmp::QmpClient::connect(&run.join("qmp.sock"))?;
    let port = control.forwarded_ssh_port()?;
    match (action.as_str(), rest) {
        ("capture", [filename]) => control.screendump(filename)?,
        ("keys", keys) if !keys.is_empty() && keys.len() <= 64 => {
            for key in keys {
                control.send_key(key)?;
                thread::sleep(Duration::from_millis(150));
            }
        }
        ("login", []) => {
            let mut secret = fs::read(run.join("password-private"))?;
            for byte in password(&secret)? {
                control.send_key(std::str::from_utf8(std::slice::from_ref(byte))?)?;
                thread::sleep(Duration::from_millis(150));
            }
            control.send_key("ret")?;
            secret.fill(0);
        }
        _ => return Err("unsupported issue25 input/capture request".into()),
    }
    println!("issue25_qmp_request=completed forwarded_ssh_port={port}");
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("session-lab-control: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_fixture_passwords_before_qmp_input() {
        assert!(password(&[b'a'; 32]).is_err());
        assert!(password(&[b'a'; 34]).is_err());
        assert!(password(b"abcdefghijklmnopqrstuvwxyz123456\n").is_err());
        assert!(password(b"abcdefghijklmnopqrstuvwxyzabcdef\n").is_ok());
    }

    #[test]
    fn refuses_an_unmarked_directory() {
        assert!(guard(Path::new("/tmp")).is_err());
    }
}
