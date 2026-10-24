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
    RunActor(ActorExecution, SyncSender<Result<()>>),
    Refreeze(SyncSender<Result<()>>),
    Release(SyncSender<Result<()>>),
}

/// Constructed by the fixed native broker, never decoded from a UI request.
pub(crate) struct ActorExecution {
    pub(crate) actor_pid: i32,
    pub(crate) executables: Vec<ExecutablePin>,
}

pub(crate) struct ExecutablePin {
    file: File,
    dev: u64,
    ino: u64,
    size: u64,
    changed: (i64, i64),
}
impl ExecutablePin {
    pub(crate) fn open(path: &std::path::Path) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let root = File::open("/")?;
        let relative = path.strip_prefix("/").map_err(|_| Error::UnsafeBoundary)?;
        let file = File::from(rustix::fs::openat2(
            &root,
            relative,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::CLOEXEC | rustix::fs::OFlags::NONBLOCK,
            rustix::fs::Mode::empty(),
            rustix::fs::ResolveFlags::BENEATH
                | rustix::fs::ResolveFlags::NO_SYMLINKS
                | rustix::fs::ResolveFlags::NO_MAGICLINKS,
        )?);
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(Error::UnsafeBoundary);
        }
        Ok(Self {
            file,
            dev: metadata.dev(),
            ino: metadata.ino(),
            size: metadata.size(),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
    pub(crate) fn matches(&self, pid: i32) -> Result<bool> {
        use std::os::unix::fs::MetadataExt;
        let current = self.file.metadata()?;
        let executable = File::open(format!("/proc/{pid}/exe"))?.metadata()?;
        Ok((current.dev(), current.ino(), current.size(), current.ctime(), current.ctime_nsec())
            == (self.dev, self.ino, self.size, self.changed.0, self.changed.1)
            && (executable.dev(), executable.ino()) == (self.dev, self.ino))
    }
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
                let mut executing: Option<ActorExecution> = None;
                let mut poisoned = false;
                loop {
                    let request = if executing.is_some() {
                        match receiver.recv_timeout(Duration::from_millis(2)) {
                            Ok(request) => Some(request),
                            Err(mpsc::RecvTimeoutError::Timeout) => None,
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match receiver.recv() {
                            Ok(request) => Some(request),
                            Err(_) => break,
                        }
                    };
                    if let Some(execution) = &executing
                        && let Err(error) = owner.run_tick(execution)
                    {
                        #[cfg(test)]
                        eprintln!(
                            "actor tick failure {error:?}; actor={} task={:?}; held={:?}",
                            execution.actor_pid,
                            read_task(execution.actor_pid),
                            owner
                                .tasks
                                .iter()
                                .map(|(pid, h)| (
                                    *pid,
                                    h.tgid,
                                    h.start,
                                    h.stopped,
                                    h.exec,
                                    h.actor_owned
                                ))
                                .collect::<Vec<_>>()
                        );
                        #[cfg(not(test))]
                        let _ = error;
                        poisoned = true;
                        let _ = owner.refreeze_actor(execution);
                        executing = None;
                    }
                    let Some(request) = request else { continue };
                    match request {
                        Request::Check(reply) => {
                            let _ = reply.send(if poisoned || executing.is_some() {
                                Err(Error::WriterUnavailable)
                            } else {
                                owner.check()
                            });
                        }
                        Request::RunActor(execution, reply) => {
                            let result = if poisoned || executing.is_some() {
                                Err(Error::WriterUnavailable)
                            } else {
                                owner.begin_actor(&execution)
                            };
                            if result.is_ok() {
                                executing = Some(execution);
                            } else {
                                poisoned = true;
                                let _ = owner.refreeze_actor(&execution);
                            }
                            let _ = reply.send(result);
                        }
                        Request::Refreeze(reply) => {
                            let result = if let Some(execution) = &executing {
                                owner.refreeze_actor(execution)
                            } else {
                                owner.refreeze()
                            };
                            executing = None;
                            if result.is_err() {
                                poisoned = true;
                            }
                            let _ = reply.send(if poisoned {
                                Err(Error::WriterUnavailable)
                            } else {
                                result
                            });
                        }
                        Request::Release(reply) => {
                            let stopped = if let Some(execution) = &executing {
                                owner.refreeze_actor(execution)
                            } else {
                                Ok(())
                            };
                            let result = stopped.and_then(|()| owner.detach());
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
    pub(crate) fn run_actor(&self, execution: ActorExecution) -> Result<()> {
        let (sender, reply) = mpsc::sync_channel(1);
        self.sender
            .as_ref()
            .ok_or(Error::WriterUnavailable)?
            .send(Request::RunActor(execution, sender))
            .map_err(|_| Error::WriterUnavailable)?;
        reply.recv_timeout(TIMEOUT).map_err(|_| Error::WriterUnavailable)?
    }
    pub(crate) fn refreeze(&self) -> Result<()> {
        let (sender, reply) = mpsc::sync_channel(1);
        self.sender
            .as_ref()
            .ok_or(Error::WriterUnavailable)?
            .send(Request::Refreeze(sender))
            .map_err(|_| Error::WriterUnavailable)?;
        reply
            .recv_timeout(TIMEOUT + Duration::from_secs(1))
            .map_err(|_| Error::WriterUnavailable)?
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

#[derive(Clone, Debug)]
struct Task {
    tid: i32,
    tgid: i32,
    tracer: i32,
    state: char,
    start: u64,
    uids: Vec<u32>,
}
struct Held {
    start: u64,
    tgid: i32,
    stopped: bool,
    signal: i32,
    exec: bool,
    actor_owned: bool,
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
                owner.tasks.insert(
                    task.tid,
                    Held {
                        start: task.start,
                        tgid: task.tgid,
                        stopped: false,
                        signal: 0,
                        exec: false,
                        actor_owned: false,
                    },
                );
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
            #[cfg(test)]
            if event == libc::PTRACE_EVENT_EXEC {
                eprintln!(
                    "exec event tid={tid} previous={:?} actual={:?}",
                    event_child(tid),
                    read_task(tid)
                );
            }

            let held = self.tasks.get_mut(&tid).ok_or(Error::WriterUnavailable)?;
            held.stopped = true;
            held.exec = event == libc::PTRACE_EVENT_EXEC;
            let actor_owned = held.actor_owned;
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
                        tgid: task.tgid,
                        stopped: false,
                        signal: 0,
                        exec: false,
                        actor_owned,
                    });
                }
            }
        }
        Ok(())
    }
    fn begin_actor(&mut self, execution: &ActorExecution) -> Result<()> {
        self.check()?;
        let mut processes = child_processes(execution.actor_pid, &[])?;
        processes.push(execution.actor_pid);
        if !self.tasks.values().any(|held| held.tgid == execution.actor_pid) {
            return Err(Error::WriterUnavailable);
        }
        for held in self.tasks.values_mut() {
            held.actor_owned = processes.contains(&held.tgid);
        }
        self.run_tick(execution)
    }
    fn run_tick(&mut self, execution: &ActorExecution) -> Result<()> {
        self.poll()?;
        self.validate_actor(execution)?;
        for task in self.target.scan()? {
            if task.tracer != self.tracer
                || self.tasks.get(&task.tid).is_none_or(|held| {
                    held.start != task.start
                        || held.tgid != task.tgid
                        || (!held.actor_owned && (!held.stopped || task.state != 't'))
                })
            {
                return Err(Error::WriterUnavailable);
            }
        }
        for (&tid, held) in &mut self.tasks {
            if !held.actor_owned {
                continue;
            }
            if !held.stopped {
                continue;
            }
            control(libc::PTRACE_CONT, tid, held.signal as usize)?;
            held.stopped = false;
            held.signal = 0;
            held.exec = false;
        }
        Ok(())
    }
    // A final exec/exit can arrive inside the refreeze poll, after the last
    // running tick. Validate again while all tasks are stopped, before any
    // post-capture or successful response; preserve failure even after exit.
    fn refreeze_actor(&mut self, execution: &ActorExecution) -> Result<()> {
        let result = self.refreeze_using(Some(execution));
        if result.is_err() {
            let _ = self.refreeze();
        }
        result
    }
    fn validate_actor(&mut self, execution: &ActorExecution) -> Result<()> {
        let rejected = self.tasks.iter().find_map(|(&tid, held)| {
            (held.actor_owned
                && held.exec
                && (held.tgid == execution.actor_pid
                    || !execution.executables.iter().any(|pin| pin.matches(tid).unwrap_or(false))))
            .then_some(tid)
        });
        if let Some(tid) = rejected {
            self.terminate_rejected_image(tid)?;
            return Err(Error::WriterUnavailable);
        }
        let actor = read_task(execution.actor_pid)?.ok_or(Error::WriterUnavailable)?;
        if matches!(actor.state, 'Z' | 'X')
            || !self.tasks.get(&execution.actor_pid).is_some_and(|held| {
                held.actor_owned
                    && held.start == actor.start
                    && held.tgid == actor.tgid
                    && actor.tracer == self.tracer
            })
        {
            return Err(Error::WriterUnavailable);
        }
        Ok(())
    }

    fn terminate_rejected_image(&mut self, tid: i32) -> Result<()> {
        use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
        // EXEC is an event stop, not a signal-delivery stop. Pin the owned
        // process and send real SIGKILL; PTRACE_CONT signal injection can be
        // ignored here. A reused numeric PID can never receive this signal.
        let pid = Pid::from_raw(tid).ok_or(Error::WriterUnavailable)?;
        let process = pidfd_open(pid, PidfdFlags::empty()).map_err(std::io::Error::from)?;
        let observed = read_task(tid)?.ok_or(Error::WriterUnavailable)?;
        let held = self.tasks.get(&tid).ok_or(Error::WriterUnavailable)?;
        if observed.tracer != self.tracer
            || observed.tgid != held.tgid
            || !held.actor_owned
            || !held.exec
        {
            return Err(Error::WriterUnavailable);
        }
        pidfd_send_signal(&process, Signal::KILL).map_err(std::io::Error::from)?;
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let exited = wait_task(tid)?
                .is_some_and(|status| libc::WIFEXITED(status) || libc::WIFSIGNALED(status));
            let dead = read_task(tid)?
                .is_none_or(|task| task.start != observed.start || matches!(task.state, 'Z' | 'X'));
            if exited || dead {
                self.tasks.remove(&tid);
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::WriterUnavailable);
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn refreeze(&mut self) -> Result<()> {
        self.refreeze_using(None)
    }
    fn refreeze_using(&mut self, execution: Option<&ActorExecution>) -> Result<()> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            for (&tid, held) in &self.tasks {
                if held.actor_owned && !held.stopped {
                    match control(libc::PTRACE_INTERRUPT, tid, 0) {
                        Ok(()) => (),
                        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => (),
                        Err(error) => return Err(error.into()),
                    }
                }
            }
            self.poll()?;
            // Keep rejecting exec stops during emergency refreeze as well:
            // a transient de-thread race may have failed the preceding tick.
            if let Some(execution) = execution {
                self.validate_actor(execution)?;
            }
            if self.check().is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Error::WriterUnavailable);
            }
            thread::sleep(Duration::from_millis(2));
        }
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
                // A confirmed zombie cannot write and its environment has
                // already been torn down. Its parent may reap it after capture.
                if status
                    .lines()
                    .find_map(|line| line.strip_prefix("State:"))
                    .and_then(|value| value.trim().chars().next())
                    .is_some_and(|state| matches!(state, 'Z' | 'X'))
                {
                    continue;
                }
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
    let tgid = status
        .lines()
        .find_map(|line| line.strip_prefix("Tgid:"))
        .ok_or(Error::UnsafeBoundary)?
        .trim()
        .parse()
        .map_err(|_| Error::UnsafeBoundary)?;
    Ok(Some(Task { tid, tgid, tracer, state, start, uids }))
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
    #[test]
    fn executable_pin_refuses_symlinks_and_non_root_owned_images() {
        use std::os::unix::fs::{MetadataExt, symlink};
        let directory =
            std::env::temp_dir().join(format!("ryoku-executable-pin-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let link = directory.join("linked-executable");
        symlink(std::env::current_exe().unwrap(), &link).unwrap();
        assert!(ExecutablePin::open(&link).is_err());
        // Identity is read from the same opened descriptor, never a separate
        // pathname stat. The actor exec regression below covers replacement.
        let executable = std::env::current_exe().unwrap();
        if executable.metadata().unwrap().uid() == 0 {
            let pin = ExecutablePin::open(&executable).unwrap();
            assert!(pin.matches(std::process::id() as i32).unwrap());
            assert!(ExecutablePin::open(&directory).is_err());
        } else {
            assert!(ExecutablePin::open(&executable).is_err());
        }
        std::fs::remove_file(link).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

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
    fn fork_writer(path: PathBuf) {
        let writer_path = path.clone();
        let worker = thread::spawn(move || writer(writer_path));
        while !path.join("fork-now").exists() {
            thread::sleep(Duration::from_millis(2));
        }
        let mut child = Command::new("/usr/bin/sleep").arg("2").spawn().unwrap();
        std::fs::write(path.join("fork-child"), child.id().to_string()).unwrap();
        let _ = child.wait();
        worker.join().unwrap();
    }

    fn exec_writer(path: PathBuf) {
        use std::os::unix::process::CommandExt;
        let worker_path = path.clone();
        thread::spawn(move || writer(worker_path));
        while !path.join("exec-now").exists() {
            thread::sleep(Duration::from_millis(2));
        }
        panic!(
            "exec fixture failed: {}",
            Command::new("/usr/bin/touch").arg(path.join("forbidden-exec-write")).exec()
        );
    }

    #[test]
    fn approved_actor_keeps_sibling_stopped_refreezes_and_rejects_exec_replacement() {
        const ACTOR_TEST: &str = "writer_freeze::tests::approved_actor_keeps_sibling_stopped_refreezes_and_rejects_exec_replacement";
        if std::env::var(ROLE).as_deref() == Ok("actor-scenario") {
            let mut actor = Fixture::new();
            let actor_pid = actor.spawn("writer");
            let mut sibling = Fixture::new();
            let sibling_pid = sibling.spawn("writer");
            for index in 0..3 {
                actor.wait(&format!("counter-{index}"));
                sibling.wait(&format!("counter-{index}"));
            }
            let target = || Target::Children {
                parent: std::process::id() as i32,
                uid: rustix::process::geteuid().as_raw(),
                home: std::env::var("HOME").unwrap(),
                excluded: vec![],
            };
            let mut guard = WriterFreeze::start(target()).unwrap();
            let actor_before = actor.counters();
            let sibling_before = sibling.counters();
            guard.run_actor(ActorExecution { actor_pid, executables: vec![] }).unwrap();
            actor.resumed(&actor_before);
            assert_eq!(sibling.counters(), sibling_before);
            assert_eq!(read_task(sibling_pid).unwrap().unwrap().state, 't');
            guard.refreeze().unwrap();
            guard.check().unwrap();
            let stopped = actor.counters();
            thread::sleep(Duration::from_millis(30));
            assert_eq!(actor.counters(), stopped);
            assert_eq!(sibling.counters(), sibling_before);
            guard.release().unwrap();
            sibling.resumed(&sibling_before);

            let mut forking = Fixture::new();
            let forking_pid = forking.spawn("fork-writer");
            for index in 0..3 {
                forking.wait(&format!("counter-{index}"));
            }
            let mut guard = WriterFreeze::start(target()).unwrap();
            let sibling_before = sibling.counters();
            std::fs::write(forking.path.join("fork-now"), "1").unwrap();
            guard
                .run_actor(ActorExecution {
                    actor_pid: forking_pid,
                    executables: vec![
                        ExecutablePin::open(std::path::Path::new("/usr/bin/sleep")).unwrap(),
                    ],
                })
                .unwrap();
            forking.wait("fork-child");
            let child_pid = std::fs::read_to_string(forking.path.join("fork-child"))
                .unwrap()
                .parse::<i32>()
                .unwrap();
            let deadline = Instant::now() + TIMEOUT;
            while read_task(child_pid).unwrap().is_none_or(|task| task.state == 't') {
                assert!(Instant::now() < deadline, "approved child did not resume");
                thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(sibling.counters(), sibling_before);
            assert_eq!(read_task(sibling_pid).unwrap().unwrap().state, 't');
            guard.refreeze().unwrap();
            guard.check().unwrap();
            assert_eq!(read_task(child_pid).unwrap().unwrap().state, 't');
            assert_eq!(sibling.counters(), sibling_before);
            guard.release().unwrap();
            sibling.resumed(&sibling_before);
            std::fs::write(forking.path.join("stop"), "1").unwrap();
            forking.wait("writer-ended");

            let mut replacement = Fixture::new();
            let replacement_pid = replacement.spawn("exec-writer");
            for index in 0..3 {
                replacement.wait(&format!("counter-{index}"));
            }
            let mut guard = WriterFreeze::start(target()).unwrap();
            let sibling_before = sibling.counters();
            std::fs::write(replacement.path.join("exec-now"), "1").unwrap();
            guard
                .run_actor(ActorExecution { actor_pid: replacement_pid, executables: vec![] })
                .unwrap();
            let deadline = Instant::now() + TIMEOUT;
            while read_task(replacement_pid).unwrap().is_some_and(|task| task.state != 'Z') {
                assert!(Instant::now() < deadline, "actor exec did not fail closed");
                thread::sleep(Duration::from_millis(2));
            }
            assert!(
                !replacement.path.join("forbidden-exec-write").exists(),
                "rejected image ran user code"
            );
            assert!(guard.refreeze().is_err());
            assert!(guard.check().is_err());
            assert_eq!(sibling.counters(), sibling_before);
            assert_eq!(read_task(sibling_pid).unwrap().unwrap().state, 't');
            guard.release().unwrap();
            assert!(
                read_task(replacement_pid)
                    .unwrap()
                    .is_none_or(|task| matches!(task.state, 'Z' | 'X')),
                "rejected actor survived detach"
            );
            assert!(!replacement.path.join("forbidden-exec-write").exists());
            sibling.resumed(&sibling_before);
            return;
        }
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", ACTOR_TEST, "--nocapture", "--test-threads=1"])
            .env(ROLE, "actor-scenario")
            .status()
            .unwrap();
        assert!(status.success());
    }
    #[test]
    fn kernel_guard_resumes_after_release_and_helper_sigkill() {
        if let Ok(role) = std::env::var(ROLE) {
            let path = PathBuf::from(std::env::var_os(PATH).unwrap());
            match role.as_str() {
                "writer" => writer(path),
                "exec-writer" => exec_writer(path),
                "fork-writer" => fork_writer(path),
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
