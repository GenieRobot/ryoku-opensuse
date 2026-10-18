//! Kernel-owned writer stops. PTRACE_SEIZE/INTERRUPT never sends SIGSTOP and
//! never enables EXITKILL. Linux detaches and restarts the account's tasks if
//! the tracer thread or helper dies. No memory/register access is performed.
//! See https://man7.org/linux/man-pages/man2/ptrace.2.html, attaching/detaching.

use crate::capture_session::{CaptureSessionError as Error, Result};
use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    sync::mpsc::{self, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TASKS: usize = 4096;
const OPTIONS: usize = (libc::PTRACE_O_TRACEFORK
    | libc::PTRACE_O_TRACEVFORK
    | libc::PTRACE_O_TRACECLONE
    | libc::PTRACE_O_TRACEEXEC) as usize;

enum Target {
    Children {
        parent: i32,
        uid: u32,
        home: String,
        excluded: Vec<i32>,
    },
    #[cfg(test)]
    Process(i32),
}
enum Request {
    Check(SyncSender<Result<()>>),
    Release(SyncSender<Result<()>>),
}

pub(crate) struct WriterFreeze {
    sender: Option<SyncSender<Request>>,
    owner: Option<JoinHandle<()>>,
}
impl WriterFreeze {
    pub(crate) fn for_children(home: &str, excluded: Vec<i32>) -> Result<Self> {
        Self::start(Target::Children {
            parent: std::process::id() as i32,
            uid: rustix::process::geteuid().as_raw(),
            home: home.to_owned(),
            excluded,
        })
    }
    fn start(target: Target) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (ready, received) = mpsc::sync_channel(1);
        let owner =
            thread::Builder::new().name("ryoku-capture-writers".into()).spawn(move || {
                let mut owner = match TraceOwner::freeze(target) {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = ready.send(Err(e));
                        return;
                    }
                };
                if ready.send(Ok(())).is_err() {
                    return;
                }
                while let Ok(request) = receiver.recv() {
                    match request {
                        Request::Check(reply) => {
                            let _ = reply.send(owner.check());
                        }
                        Request::Release(reply) => {
                            let result = owner.detach();
                            let _ = reply.send(result);
                            return;
                        }
                    }
                }
                // Dropping the command channel ends the tracer thread even when
                // an uninterruptible task could not reach ptrace-stop. The kernel
                // then removes all remaining tracing relationships automatically.
            })?;
        match received.recv_timeout(TIMEOUT + Duration::from_secs(1)) {
            Ok(Ok(())) => Ok(Self { sender: Some(sender), owner: Some(owner) }),
            Ok(Err(e)) => {
                drop(sender);
                let _ = owner.join();
                Err(e)
            }
            Err(_) => {
                drop(sender);
                let _ = owner.join();
                Err(Error::WriterUnavailable)
            }
        }
    }
    pub(crate) fn check(&self) -> Result<()> {
        let (sender, reply) = mpsc::sync_channel(1);
        self.sender
            .as_ref()
            .ok_or(Error::WriterUnavailable)?
            .send(Request::Check(sender))
            .map_err(|_| Error::WriterUnavailable)?;
        reply.recv_timeout(TIMEOUT).map_err(|_| Error::WriterUnavailable)?
    }
    pub(crate) fn release(&mut self) -> Result<()> {
        let result = if let Some(sender) = self.sender.take() {
            let (ack, reply) = mpsc::sync_channel(1);
            let result =
                sender.send(Request::Release(ack)).map_err(|_| Error::WriterUnavailable).and_then(
                    |()| reply.recv_timeout(TIMEOUT).map_err(|_| Error::WriterUnavailable)?,
                );
            drop(sender);
            result
        } else {
            Ok(())
        };
        if let Some(owner) = self.owner.take() {
            owner.join().map_err(|_| Error::WriterUnavailable)?;
        }
        result
    }
}
impl Drop for WriterFreeze {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[derive(Clone)]
struct Task {
    tid: i32,
    tracer: i32,
    state: char,
    start: u64,
    uids: Vec<u32>,
}
struct Held {
    start: u64,
    stopped: bool,
    signal: i32,
}
struct TraceOwner {
    target: Target,
    tracer: i32,
    tasks: BTreeMap<i32, Held>,
}
impl TraceOwner {
    fn freeze(target: Target) -> Result<Self> {
        let status = std::fs::read_to_string("/proc/thread-self/status")?;
        let tracer = status
            .lines()
            .find_map(|line| line.strip_prefix("Pid:\t"))
            .ok_or(Error::UnsafeBoundary)?
            .parse()
            .map_err(|_| Error::UnsafeBoundary)?;
        let mut owner = Self { target, tracer, tasks: BTreeMap::new() };
        let deadline = Instant::now() + TIMEOUT;
        loop {
            for task in owner.target.scan()? {
                if owner.tasks.contains_key(&task.tid) {
                    continue;
                }
                if owner.tasks.len() >= MAX_TASKS
                    || task.tracer != 0 && task.tracer != tracer
                    || task.state == 'T'
                {
                    return Err(Error::WriterUnavailable);
                }
                if task.tracer == 0 {
                    match control(libc::PTRACE_SEIZE, task.tid, OPTIONS) {
                        Ok(()) => {}
                        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => continue,
                        Err(e) => return Err(e.into()),
                    }
                }
                owner.tasks.insert(task.tid, Held { start: task.start, stopped: false, signal: 0 });
                if task.tracer == 0 {
                    match control(libc::PTRACE_INTERRUPT, task.tid, 0) {
                        Ok(()) => {}
                        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            owner.poll()?;
            let current = owner.target.scan()?;
            if current.iter().all(|task| {
                owner.tasks.get(&task.tid).is_some_and(|held| {
                    held.start == task.start
                        && held.stopped
                        && task.tracer == tracer
                        && task.state == 't'
                })
            }) {
                owner.check()?;
                return Ok(owner);
            }
            if Instant::now() >= deadline {
                return Err(Error::WriterUnavailable);
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn poll(&mut self) -> Result<()> {
        let tids: Vec<_> = self.tasks.keys().copied().collect();
        for tid in tids {
            if self.tasks[&tid].stopped {
                continue;
            }
            let Some(status) = wait_task(tid)? else {
                continue;
            };
            if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                self.tasks.remove(&tid);
                continue;
            }
            if !libc::WIFSTOPPED(status) {
                return Err(Error::WriterUnavailable);
            }
            let event = status >> 16;
            let held = self.tasks.get_mut(&tid).ok_or(Error::WriterUnavailable)?;
            held.stopped = true;
            held.signal = if event == 0
                || event == libc::PTRACE_EVENT_STOP && libc::WSTOPSIG(status) != libc::SIGTRAP
            {
                libc::WSTOPSIG(status)
            } else {
                0
            };
            if matches!(
                event,
                libc::PTRACE_EVENT_FORK | libc::PTRACE_EVENT_VFORK | libc::PTRACE_EVENT_CLONE
            ) {
                let child = event_child(tid)?;
                if let Some(task) = read_task(child)? {
                    self.tasks.entry(child).or_insert(Held {
                        start: task.start,
                        stopped: false,
                        signal: 0,
                    });
                }
            }
        }
        Ok(())
    }
    fn check(&self) -> Result<()> {
        for task in self.target.scan()? {
            if !self.tasks.get(&task.tid).is_some_and(|held| {
                held.stopped
                    && held.start == task.start
                    && task.tracer == self.tracer
                    && task.state == 't'
            }) {
                return Err(Error::WriterUnavailable);
            }
        }
        for (&tid, held) in &self.tasks {
            if let Some(task) = read_task(tid)?
                && (self.target.account().is_some_and(|(uid, _)| !task.uids.contains(&uid))
                    || task.start != held.start
                    || task.tracer != self.tracer
                    || task.state != 't'
                    || !held.stopped)
            {
                return Err(Error::WriterUnavailable);
            }
        }
        Ok(())
    }
    fn detach(&mut self) -> Result<()> {
        let mut failure = false;
        for (tid, held) in std::mem::take(&mut self.tasks) {
            if let Err(error) = control(libc::PTRACE_DETACH, tid, held.signal as usize)
                && error.raw_os_error() != Some(libc::ESRCH)
            {
                failure = true;
            }
        }
        if failure { Err(Error::WriterUnavailable) } else { Ok(()) }
    }
}
impl Drop for TraceOwner {
    fn drop(&mut self) {
        let _ = self.detach();
    }
}

impl Target {
    fn account(&self) -> Option<(u32, &str)> {
        match self {
            Self::Children { uid, home, .. } => Some((*uid, home)),
            #[cfg(test)]
            Self::Process(_) => None,
        }
    }
    fn scan(&self) -> Result<Vec<Task>> {
        let mut result = Vec::new();
        let pids: Vec<i32> = match self {
            Target::Children { parent, excluded, .. } => child_processes(*parent, excluded)?,
            #[cfg(test)]
            Target::Process(pid) => vec![*pid],
        };
        for pid in pids {
            let entries = match std::fs::read_dir(format!("/proc/{pid}/task")) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in entries {
                let entry = entry?;
                let Some(tid) =
                    entry.file_name().to_str().and_then(|name| name.parse::<i32>().ok())
                else {
                    continue;
                };
                let status = match std::fs::read_to_string(entry.path().join("status")) {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(e) => return Err(e.into()),
                };
                if let Some((uid, home)) = self.account() {
                    let ids = status
                        .lines()
                        .find_map(|line| line.strip_prefix("Uid:"))
                        .ok_or(Error::UnsafeBoundary)?;
                    let ids: Vec<u32> = ids
                        .split_whitespace()
                        .map(str::parse)
                        .collect::<std::result::Result<_, _>>()
                        .map_err(|_| Error::UnsafeBoundary)?;
                    if ids.len() != 4 {
                        return Err(Error::UnsafeBoundary);
                    }
                    if !ids.contains(&uid) {
                        continue;
                    }
                    let mut env = Vec::new();
                    File::open(entry.path().join("environ"))?
                        .take(1024 * 1024 + 1)
                        .read_to_end(&mut env)?;
                    if env.len() > 1024 * 1024 {
                        return Err(Error::Unsupported);
                    }
                    crate::capture_session::check_layout(&env, home)?;
                }
                if let Some(task) = read_task(tid)?
                    && self.account().is_none_or(|(uid, _)| task.uids.contains(&uid))
                {
                    result.push(task);
                }
                if result.len() > MAX_TASKS {
                    return Err(Error::WriterUnavailable);
                }
            }
        }
        Ok(result)
    }
}

pub(crate) fn owned_child_processes(parent: i32) -> Result<Vec<i32>> {
    child_processes(parent, &[])
}
fn child_processes(parent: i32, excluded: &[i32]) -> Result<Vec<i32>> {
    let mut parents = BTreeMap::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<i32>().ok()) else {
            continue;
        };
        let stat = match std::fs::read_to_string(entry.path().join("stat")) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let ppid: i32 = stat
            .rsplit_once(')')
            .ok_or(Error::UnsafeBoundary)?
            .1
            .split_whitespace()
            .nth(1)
            .ok_or(Error::UnsafeBoundary)?
            .parse()
            .map_err(|_| Error::UnsafeBoundary)?;
        parents.insert(pid, ppid);
    }
    let mut result = std::collections::BTreeSet::new();
    loop {
        let before = result.len();
        for (&pid, &ppid) in &parents {
            if !excluded.contains(&pid) && (ppid == parent || result.contains(&ppid)) {
                result.insert(pid);
            }
        }
        if result.len() > MAX_TASKS {
            return Err(Error::WriterUnavailable);
        }
        if result.len() == before {
            break;
        }
    }
    Ok(result.into_iter().collect())
}

fn read_task(tid: i32) -> Result<Option<Task>> {
    let status = match std::fs::read_to_string(format!("/proc/{tid}/status")) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let tracer = status
        .lines()
        .find_map(|line| line.strip_prefix("TracerPid:"))
        .ok_or(Error::UnsafeBoundary)?
        .trim()
        .parse()
        .map_err(|_| Error::UnsafeBoundary)?;
    let stat = match std::fs::read_to_string(format!("/proc/{tid}/stat")) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let fields: Vec<_> =
        stat.rsplit_once(')').ok_or(Error::UnsafeBoundary)?.1.split_whitespace().collect();
    let state = fields.first().and_then(|v| v.chars().next()).ok_or(Error::UnsafeBoundary)?;
    let start =
        fields.get(19).ok_or(Error::UnsafeBoundary)?.parse().map_err(|_| Error::UnsafeBoundary)?;
    let uids: Vec<u32> = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .ok_or(Error::UnsafeBoundary)?
        .split_whitespace()
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| Error::UnsafeBoundary)?;
    if uids.len() != 4 {
        return Err(Error::UnsafeBoundary);
    }
    Ok(Some(Task { tid, tracer, state, start, uids }))
}

#[allow(unsafe_code)] // Scalar ptrace requests only; no tracee memory/register access.
fn control(request: libc::c_uint, tid: i32, value: usize) -> std::io::Result<()> {
    // SAFETY: SEIZE/INTERRUPT/DETACH interpret data as an integer; addr is null.
    let result = unsafe {
        libc::ptrace(request, tid, std::ptr::null_mut::<libc::c_void>(), value as *mut libc::c_void)
    };
    if result == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}
#[allow(unsafe_code)] // Kernel writes one initialized c_int through this local pointer.
fn wait_task(tid: i32) -> std::io::Result<Option<i32>> {
    let mut status = 0;
    // SAFETY: status is valid for the duration of waitpid; tid is a traced task.
    let result = unsafe {
        libc::waitpid(tid, &mut status, libc::WNOHANG | libc::__WALL | libc::__WNOTHREAD)
    };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ECHILD) { Ok(None) } else { Err(error) }
    } else if result == 0 {
        Ok(None)
    } else {
        Ok(Some(status))
    }
}
#[allow(unsafe_code)] // GETEVENTMSG writes one native unsigned long to this local pointer.
fn event_child(tid: i32) -> std::io::Result<i32> {
    let mut child: libc::c_ulong = 0;
    // SAFETY: child is a valid output location; the request follows a fork event.
    let result = unsafe {
        libc::ptrace(
            libc::PTRACE_GETEVENTMSG,
            tid,
            std::ptr::null_mut::<libc::c_void>(),
            &mut child as *mut libc::c_ulong,
        )
    };
    if result == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        i32::try_from(child).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        process::{Child, Command},
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    const TEST: &str =
        "writer_freeze::tests::kernel_guard_resumes_after_release_and_helper_sigkill";
    const ROLE: &str = "RYOKU_TRACE_TEST_ROLE";
    const PATH: &str = "RYOKU_TRACE_TEST_PATH";
    struct Fixture {
        path: PathBuf,
        children: Vec<Child>,
    }
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ryoku-trace-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self { path, children: Vec::new() }
        }
        fn spawn(&mut self, role: &str) -> i32 {
            let child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
                .env(ROLE, role)
                .env(PATH, &self.path)
                .spawn()
                .unwrap();
            let pid = i32::try_from(child.id()).unwrap();
            self.children.push(child);
            pid
        }
        fn wait(&self, name: &str) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !self.path.join(name).exists() {
                assert!(Instant::now() < deadline, "missing {name}");
                thread::sleep(Duration::from_millis(5));
            }
        }
        fn counters(&self) -> Vec<Vec<u8>> {
            (0..3).map(|i| std::fs::read(self.path.join(format!("counter-{i}"))).unwrap()).collect()
        }
        fn resumed(&self, previous: &[Vec<u8>]) {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let current = self.counters();
                if current.iter().zip(previous).all(|(a, b)| a != b) {
                    return;
                }
                assert!(Instant::now() < deadline, "writer remained stopped");
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::write(self.path.join("stop"), "1");
            for child in &mut self.children {
                let _ = child.kill();
                let _ = child.wait();
            }
            // A crash-test writer is orphaned when its tracer parent is killed.
            // Its independent stop marker lets it leave normally; no other PID is killed.
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.path.join("counter-0").exists()
                && !self.path.join("writer-ended").exists()
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(5));
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
    fn writer(path: PathBuf) {
        let mut handles = Vec::new();
        for index in 0..3 {
            let path = path.clone();
            handles.push(thread::spawn(move || {
                for value in 1_u64..=u64::MAX {
                    if path.join("stop").exists() {
                        break;
                    }
                    let next = path.join(format!("counter-{index}.next"));
                    std::fs::write(&next, value.to_string()).unwrap();
                    std::fs::rename(next, path.join(format!("counter-{index}"))).unwrap();
                    thread::sleep(Duration::from_millis(2));
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        std::fs::write(path.join("writer-ended"), "1").unwrap();
    }
    #[test]
    fn kernel_guard_resumes_after_release_and_helper_sigkill() {
        if let Ok(role) = std::env::var(ROLE) {
            let path = PathBuf::from(std::env::var_os(PATH).unwrap());
            match role.as_str() {
                "writer" => writer(path),
                "tracer" => {
                    let child = Command::new(std::env::current_exe().unwrap())
                        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
                        .env(ROLE, "writer")
                        .env(PATH, &path)
                        .spawn()
                        .unwrap();
                    let fixture = Fixture { path: path.clone(), children: vec![child] };
                    for i in 0..3 {
                        fixture.wait(&format!("counter-{i}"));
                    }
                    let guard = WriterFreeze::start(Target::Process(
                        i32::try_from(fixture.children[0].id()).unwrap(),
                    ))
                    .unwrap();
                    guard.check().unwrap();
                    std::fs::write(path.join("traced"), "1").unwrap();
                    loop {
                        thread::sleep(Duration::from_millis(50));
                    }
                }
                _ => panic!("invalid fixture role"),
            }
            return;
        }
        let mut normal = Fixture::new();
        let pid = normal.spawn("writer");
        for i in 0..3 {
            normal.wait(&format!("counter-{i}"));
        }
        let mut guard = WriterFreeze::start(Target::Process(pid)).unwrap();
        guard.check().unwrap();
        let saved = normal.counters();
        thread::sleep(Duration::from_millis(50));
        assert_eq!(normal.counters(), saved);
        guard.release().unwrap();
        normal.resumed(&saved);
        // Existing user group-stops belong to the user. Refusing capture must
        // neither adopt that stop nor resume it as a cleanup side effect.
        let process = rustix::process::Pid::from_raw(pid).unwrap();
        rustix::process::kill_process(process, rustix::process::Signal::STOP).unwrap();
        let deadline = Instant::now() + TIMEOUT;
        while read_task(pid).unwrap().unwrap().state != 'T' {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        assert!(WriterFreeze::start(Target::Process(pid)).is_err());
        assert_eq!(read_task(pid).unwrap().unwrap().state, 'T');
        let saved = normal.counters();
        thread::sleep(Duration::from_millis(30));
        assert_eq!(normal.counters(), saved);
        rustix::process::kill_process(process, rustix::process::Signal::CONT).unwrap();
        normal.resumed(&saved);

        // SIGCONT cannot bypass an active ptrace stop. A concurrent user
        // SIGSTOP remains pending and is honored after the lease is released.
        let mut guard = WriterFreeze::start(Target::Process(pid)).unwrap();
        let saved = normal.counters();
        rustix::process::kill_process(process, rustix::process::Signal::CONT).unwrap();
        thread::sleep(Duration::from_millis(30));
        assert_eq!(normal.counters(), saved);
        rustix::process::kill_process(process, rustix::process::Signal::STOP).unwrap();
        guard.release().unwrap();
        let deadline = Instant::now() + TIMEOUT;
        while read_task(pid).unwrap().unwrap().state != 'T' {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(normal.counters(), saved);
        rustix::process::kill_process(process, rustix::process::Signal::CONT).unwrap();
        normal.resumed(&saved);
        std::fs::write(normal.path.join("stop"), "1").unwrap();
        normal.wait("writer-ended");
        drop(normal);

        let mut crash = Fixture::new();
        crash.spawn("tracer");
        crash.wait("traced");
        let saved = crash.counters();
        thread::sleep(Duration::from_millis(50));
        assert_eq!(crash.counters(), saved);
        crash.children[0].kill().unwrap();
        crash.children[0].wait().unwrap();
        crash.resumed(&saved);
        std::fs::write(crash.path.join("stop"), "1").unwrap();
        crash.wait("writer-ended");
    }
}
