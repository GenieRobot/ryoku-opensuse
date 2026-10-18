//! Separately labelled account-binding API probe under the helper's sandbox policy.
//! This does not enable provider dispatch in the production helper.
use super::{
    OBSERVER, OPERATOR, SELF, child,
    support::{Result, checked, require},
};
use ryoku_service::{PeerIdentity, ServiceError, helper_protocol::prepare_operator_account};
use std::{
    fs,
    io::{Read, Write},
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
const SOCKET: &str = "/run/ryoku/account-fixture.sock";

pub(super) fn seed() -> Result<()> {
    let uid = rustix::process::geteuid().as_raw();
    let account = if uid == OPERATOR.uid {
        OPERATOR
    } else if uid == OBSERVER.uid {
        OBSERVER
    } else {
        return Err("unexpected account".into());
    };
    let parent = format!("{}/.config", account.home());
    if !Path::new(&parent).exists() {
        fs::create_dir(&parent)?;
    }
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))?;
    let root = format!("{parent}/ryoku");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let path = format!("{root}/account-marker");
    fs::write(&path, uid.to_string())?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}
pub(super) fn client() -> Result<()> {
    let socket = UnixStream::connect(SOCKET)?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut response = String::new();
    socket.take(128).read_to_string(&mut response)?;
    let expected = if rustix::process::geteuid().as_raw() == OPERATOR.uid {
        "own-account\n"
    } else {
        "unauthorized\n"
    };
    require(response == expected, "account binding probe response differs")
}
pub(super) fn server() -> Result<()> {
    super::require_root()?;
    super::acceptance::capabilities(std::process::id())?;
    let listener = UnixListener::bind(SOCKET)?;
    fs::set_permissions(SOCKET, fs::Permissions::from_mode(0o666))?;
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let operator_gid = ryoku_service::operator_group_gid()?;
    for expected_uid in [OPERATOR.uid, OBSERVER.uid] {
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(error) => return Err(error.into()),
            }
        };
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let peer = PeerIdentity::from_unix_socket(&stream)?;
        require(peer.uid() == expected_uid, "unexpected account probe client")?;
        match prepare_operator_account(&peer, operator_gid) {
            Ok(root) if expected_uid == OPERATOR.uid => {
                require(
                    root.uid() == expected_uid
                        && root.configuration().metadata()?.uid() == expected_uid,
                    "account descriptor owner differs",
                )?;
                let fd = rustix::fs::openat(
                    root.configuration(),
                    "account-marker",
                    rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW,
                    rustix::fs::Mode::empty(),
                )?;
                let mut marker = String::new();
                fs::File::from(fd).take(128).read_to_string(&mut marker)?;
                require(marker == expected_uid.to_string(), "foreign account marker selected")?;
                root.revalidate()?;
                stream.write_all(b"own-account\n")?;
            }
            Err(ServiceError::Unauthorized) if expected_uid == OBSERVER.uid => {
                stream.write_all(b"unauthorized\n")?
            }
            _ => return Err("account binding or authorization failed".into()),
        }
    }
    fs::remove_file(SOCKET)?;
    Ok(())
}
pub(super) fn run() -> Result<()> {
    for account in [OPERATOR, OBSERVER] {
        child(Some(account), "seed-config")?;
    }
    // Only this fixture socket adds a writable runtime path. The account-root
    // and capability restrictions are identical to the installed helper unit.
    let worker = std::thread::spawn(|| -> std::result::Result<(), String> {
        let result = super::support::success(Command::new("/usr/bin/systemd-run").args([
            "--quiet",
            "--wait",
            "--pipe",
            "--collect",
            "--unit=ryoku-account-fixture",
            "--property=User=root",
            "--property=Group=root",
            "--property=NoNewPrivileges=yes",
            "--property=CapabilityBoundingSet=CAP_DAC_READ_SEARCH",
            "--property=AmbientCapabilities=",
            "--property=ProtectSystem=strict",
            "--property=ProtectHome=read-only",
            "--property=PrivateTmp=yes",
            "--property=ReadWritePaths=/run/ryoku /var/lib/ryoku-state",
            "--property=RestrictAddressFamilies=AF_UNIX",
            "--property=LockPersonality=yes",
            "--property=MemoryDenyWriteExecute=yes",
            "--property=SystemCallArchitectures=native",
            SELF,
            "--account-probe",
        ]));
        result.map(|_| ()).map_err(|error| error.to_string())
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !Path::new(SOCKET).exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    let result: Result<()> = (|| {
        child(Some(OPERATOR), "account-binding")?;
        child(Some(OBSERVER), "account-binding")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = checked("/usr/bin/systemctl", &["stop", "ryoku-account-fixture.service"]);
    }
    worker
        .join()
        .map_err(|_| "account probe worker panicked")?
        .map_err(|error| format!("account probe failed: {error}"))?;
    result?;
    for account in [OPERATOR, OBSERVER] {
        super::support::directory(Path::new(&account.home()), account.uid, 0o700)?;
        let root = format!("{}/.config/ryoku", account.home());
        super::support::directory(Path::new(&root), account.uid, 0o700)?;
        require(
            fs::read_to_string(format!("{root}/account-marker"))? == account.uid.to_string(),
            "account marker mutated",
        )?;
    }
    eprintln!("native_service_account_binding_sandbox_fixture=passed");
    Ok(())
}
