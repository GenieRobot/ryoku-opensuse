use ryoku_rashin::{
    Error, Result,
    api::MAX_IMAGE_REQUEST_BYTES,
    runtime::{Engine, serve},
    store::Store,
};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "acp-worker" {
        return ryoku_rashin::acp::worker();
    }
    if args.len() != 2 {
        return Err(Error::InvalidRequest);
    }
    match args[0].to_str() {
        Some("usage-claude" | "usage-codex" | "usage-opencode") => {
            let tool = match args[0].to_str() {
                Some("usage-claude") => ryoku_rashin::usage::Tool::Claude,
                Some("usage-codex") => ryoku_rashin::usage::Tool::Codex,
                _ => ryoku_rashin::usage::Tool::Opencode,
            };
            let enrollment = ryoku_rashin::usage::Enrollment::enroll(tool, Path::new(&args[1]))?;
            let at =
                u64::try_from(jiff::Timestamp::now().as_second()).map_err(|_| Error::Storage)?;
            let report = ryoku_rashin::usage::collect(&enrollment, at)?;
            serde_json::to_writer(std::io::stdout(), &report).map_err(|_| Error::Storage)
        }
        Some("logs") => {
            let component = args[1].to_str().ok_or(Error::InvalidRequest)?;
            let evidence = ryoku_rashin::inspection::collect(component)?;
            serde_json::to_writer(std::io::stdout(), &evidence).map_err(|_| Error::Storage)
        }
        Some("serve" | "web") => {
            let store = Store::open(Path::new(&args[1]))?;
            let listener = store.listener()?;
            if args[0] == "web" {
                let token = store.browser_token()?;
                let http = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                    .map_err(|_| Error::Storage)?;
                let origin = format!("http://{}", http.local_addr().map_err(|_| Error::Storage)?);
                let engine = Engine::open(store)?;
                engine.start_usage_scheduler()?;
                let unix_engine = std::sync::Arc::clone(&engine);
                std::thread::Builder::new()
                    .name("rashin-unix".into())
                    .spawn(move || {
                        let _ = serve(listener, unix_engine);
                    })
                    .map_err(|_| Error::Storage)?;
                eprintln!(
                    "Rashin: {origin}; sign in with browser.token from your private state directory"
                );
                ryoku_rashin::http::Gateway::new(engine, token, origin)?.serve(http)
            } else {
                let engine = Engine::open(store)?;
                engine.start_usage_scheduler()?;
                serve(listener, engine)
            }
        }
        Some("request") => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .take(MAX_IMAGE_REQUEST_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| Error::InvalidRequest)?;
            if bytes.len() > MAX_IMAGE_REQUEST_BYTES {
                return Err(Error::Limit);
            }
            let mut stream =
                UnixStream::connect(&args[1]).map_err(|_| Error::ProviderUnavailable)?;
            let peer =
                rustix::net::sockopt::socket_peercred(&stream).map_err(|_| Error::Unauthorized)?;
            if peer.uid != rustix::process::geteuid() {
                return Err(Error::Unauthorized);
            }
            stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|_| Error::Storage)?;
            stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|_| Error::Storage)?;
            stream
                .write_all(&(bytes.len() as u32).to_be_bytes())
                .and_then(|()| stream.write_all(&bytes))
                .map_err(|_| Error::Storage)?;
            let mut length = [0; 4];
            stream.read_exact(&mut length).map_err(|_| Error::ProviderUnavailable)?;
            let length = u32::from_be_bytes(length) as usize;
            if length > 8 * 1024 * 1024 {
                return Err(Error::Limit);
            }
            let mut response = vec![0; length];
            stream.read_exact(&mut response).map_err(|_| Error::ProviderUnavailable)?;
            std::io::stdout()
                .write_all(&response)
                .and_then(|()| std::io::stdout().write_all(b"\n"))
                .map_err(|_| Error::Storage)
        }
        _ => Err(Error::InvalidRequest),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!(
            "ryoku-rashin: {error}; usage: ryoku-rashin serve|web PRIVATE_DIRECTORY | request SOCKET < request.json | logs COMPONENT"
        );
        std::process::exit(1);
    }
}
