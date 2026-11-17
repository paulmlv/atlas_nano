//! Bounded I/O and ownership of one labeling client's process group (macOS and Linux).

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use rustix::fs::{Mode, OFlags, fcntl_getfl, fcntl_setfl, open};
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

use super::Failure;

/// Each output stream and the Codex answer file has its own byte limit.
pub(super) const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Both deadlines start at call launch, not after spawning or receiving the response.
pub(super) struct Budget {
    response: Instant,
    total: Instant,
}

impl Budget {
    pub(super) fn new() -> Self {
        Self::with_limits(RESPONSE_TIMEOUT, TOTAL_TIMEOUT)
    }

    pub(super) fn with_limits(response: Duration, total: Duration) -> Self {
        let started = Instant::now();
        Self {
            response: started + response,
            total: started + total,
        }
    }

    pub(super) fn check_response(&self) -> Result<(), Failure> {
        self.check_total()?;
        if Instant::now() >= self.response {
            return Err(Failure::Rejected(
                "the complete labeler response exceeded the 10-minute limit".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn check_total(&self) -> Result<(), Failure> {
        if Instant::now() >= self.total {
            return Err(Failure::Rejected(
                "the labeling call exceeded the 15-minute total limit".into(),
            ));
        }
        Ok(())
    }
}

type Groups = Mutex<HashSet<Pid>>;

/// One signal listener for the CLI, rather than a thread per call. Clients have separate groups,
/// so interrupting Atlas must stop those groups before preserving the signal's normal exit status.
fn groups() -> io::Result<&'static Groups> {
    static GROUPS: OnceLock<Result<Arc<Groups>, String>> = OnceLock::new();
    GROUPS
        .get_or_init(|| {
            let groups = Arc::new(Mutex::new(HashSet::new()));
            let active = Arc::clone(&groups);
            let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP]).map_err(|e| e.to_string())?;
            thread::Builder::new()
                .name("labeler-signals".into())
                .spawn(move || {
                    for signal in signals.forever() {
                        let groups = active.lock().unwrap_or_else(|e| e.into_inner());
                        for &pid in groups.iter() {
                            let _ = kill_process_group(pid, Signal::KILL);
                        }
                        // Keep the lock until termination so another call cannot start in between.
                        let _ = signal_hook::low_level::emulate_default_handler(signal);
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(groups)
        })
        .as_ref()
        .map(Arc::as_ref)
        .map_err(|error| io::Error::other(error.clone()))
}

/// The leader is not reaped until its group is stopped. Keeping its PID reserved also prevents
/// cleanup or the signal listener from ever targeting a reused process-group ID.
struct Client {
    child: Child,
    pid: Pid,
    groups: &'static Groups,
}

impl Client {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        let groups = groups()?;
        let mut active = groups.lock().unwrap_or_else(|e| e.into_inner());
        let child = command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let pid = Pid::from_child(&child);
        active.insert(pid);
        Ok(Self { child, pid, groups })
    }

    fn exited(&self) -> io::Result<Option<bool>> {
        // WNOWAIT leaves the exited leader waitable until Drop has stopped its helpers.
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        match waitid(WaitId::Pid(self.pid), options) {
            Ok(status) => Ok(status.map(|status| status.exit_status() == Some(0))),
            Err(rustix::io::Errno::INTR) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let mut active = self.groups.lock().unwrap_or_else(|e| e.into_inner());
        let _ = kill_process_group(self.pid, Signal::KILL);
        active.remove(&self.pid);
        drop(active);
        let _ = self.child.wait();
    }
}

fn nonblocking(pipe: &impl AsFd) -> io::Result<()> {
    fcntl_setfl(pipe, fcntl_getfl(pipe)? | OFlags::NONBLOCK)?;
    Ok(())
}

/// Read one chunk per pass, so a busy stream cannot starve the other pipe or deadline checks.
fn read(
    pipe: &mut Option<impl Read>,
    bytes: &mut Vec<u8>,
    limit: usize,
    name: &str,
) -> io::Result<bool> {
    let Some(source) = pipe else { return Ok(false) };
    let mut buffer = [0; 8192];
    match source.read(&mut buffer) {
        Ok(0) => {
            *pipe = None;
            Ok(true)
        }
        Ok(count) => {
            if count > limit - bytes.len() {
                return Err(io::Error::other(format!("{name} exceeded {limit} bytes")));
            }
            bytes.extend_from_slice(&buffer[..count]);
            Ok(true)
        }
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

/// Send stdin and collect both output streams under the same deadline, with no blocking pipe
/// operations or reader threads to leave behind. Drop stops the group and reaps the direct child.
pub(super) fn run(
    mut command: Command,
    input: &str,
    budget: &Budget,
) -> Result<(bool, String, String), Failure> {
    let program = command.get_program().to_string_lossy().into_owned();
    let failed = |e| Failure::Rejected(format!("`{program}`: {e}"));
    budget.check_response()?;
    let mut client = Client::spawn(&mut command).map_err(failed)?;
    let mut stdin = client.child.stdin.take();
    let mut stdout = client.child.stdout.take();
    let mut stderr = client.child.stderr.take();
    nonblocking(stdin.as_ref().expect("stdin is piped")).map_err(failed)?;
    nonblocking(stdout.as_ref().expect("stdout is piped")).map_err(failed)?;
    nonblocking(stderr.as_ref().expect("stderr is piped")).map_err(failed)?;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut remaining = input.as_bytes();
    let mut status = None;
    loop {
        budget.check_response()?;
        if status.is_none() {
            status = client.exited().map_err(failed)?;
        }
        let mut progressed = false;
        if remaining.is_empty() || status.is_some() {
            stdin = None;
        } else if let Some(pipe) = &mut stdin {
            match pipe.write(&remaining[..remaining.len().min(8192)]) {
                Ok(0) => return Err(failed(io::Error::other("could not write the prompt"))),
                Ok(count) => {
                    remaining = &remaining[count..];
                    progressed = true;
                }
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => stdin = None,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => return Err(failed(error)),
            }
        }
        progressed |= read(&mut stdout, &mut out, OUTPUT_LIMIT, "stdout").map_err(failed)?;
        progressed |= read(&mut stderr, &mut err, OUTPUT_LIMIT, "stderr").map_err(failed)?;
        if let Some(succeeded) = status
            && stdout.is_none()
            && stderr.is_none()
        {
            budget.check_response()?;
            if succeeded && !remaining.is_empty() {
                return Err(failed(io::Error::other(
                    "exited before receiving the whole prompt",
                )));
            }
            return Ok((
                succeeded,
                String::from_utf8_lossy(&out).into_owned(),
                String::from_utf8_lossy(&err).into_owned(),
            ));
        }
        if !progressed {
            thread::sleep(
                POLL_INTERVAL.min(budget.response.saturating_duration_since(Instant::now())),
            );
        }
    }
}

/// Codex's answer bypasses stdout. Bound it too, and refuse special files such as a FIFO so an
/// unexpected file cannot create another blocking pipe outside the runner.
pub(super) fn read_answer(path: &Path, budget: &Budget) -> Result<String, Failure> {
    let failed = |e| Failure::Invalid(format!("could not read codex's answer: {e}"));
    budget.check_response()?;
    let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    let file = File::from(
        open(path, flags, Mode::empty())
            .map_err(io::Error::from)
            .map_err(failed)?,
    );
    if !file.metadata().map_err(failed)?.is_file() {
        return Err(failed(io::Error::other("expected a regular file")));
    }
    let mut file = Some(file);
    let mut bytes = Vec::new();
    while file.is_some() {
        budget.check_response()?;
        read(&mut file, &mut bytes, OUTPUT_LIMIT, "answer file").map_err(failed)?;
    }
    budget.check_response()?;
    String::from_utf8(bytes).map_err(|error| failed(io::Error::other(error)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Folder(PathBuf);

    impl Folder {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "atlas-nano-process-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    fn short_budget() -> Budget {
        Budget::with_limits(Duration::from_millis(150), Duration::from_secs(2))
    }

    fn assert_stopped(path: &Path) {
        let pid = fs::read_to_string(path).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let state = Command::new("ps")
                .args(["-p", pid.trim(), "-o", "stat="])
                .output()
                .unwrap();
            // A killed orphan can briefly be a zombie until the system reaps it.
            let state = String::from_utf8_lossy(&state.stdout);
            if state.trim().is_empty() || state.trim().starts_with('Z') {
                return;
            }
            assert!(
                Instant::now() < until,
                "helper {pid} is still running: {state}"
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    #[test]
    fn input_and_output_larger_than_pipe_capacity_do_not_deadlock() {
        let input = "prompt ".repeat(50_000);
        let command =
            shell("dd if=/dev/zero bs=1024 count=128 2>/dev/null; cat; printf warning >&2");
        let (ok, stdout, stderr) = run(command, &input, &Budget::new()).unwrap();
        assert!(ok);
        assert_eq!(stdout.len(), 128 * 1024 + input.len());
        assert!(stdout.ends_with(&input));
        assert_eq!(stderr, "warning");
    }

    #[test]
    fn a_running_client_and_its_helper_share_the_response_deadline() {
        let folder = Folder::new();
        let pid = folder.0.join("helper");
        let mut command = shell("sleep 5 & echo $! > \"$HELPER_PID\"; wait");
        command.env("HELPER_PID", &pid);
        let start = Instant::now();
        let failure = run(command, "", &short_budget()).unwrap_err();
        assert!(failure.message().contains("10-minute"));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_stopped(&pid);
    }

    #[test]
    fn a_helper_holding_pipes_after_its_parent_exits_cannot_extend_the_deadline() {
        let folder = Folder::new();
        let pid = folder.0.join("helper");
        let mut command = shell("sleep 5 & echo $! > \"$HELPER_PID\"; printf done");
        command.env("HELPER_PID", &pid);
        let start = Instant::now();
        let failure = run(command, "", &short_budget()).unwrap_err();
        assert!(failure.message().contains("10-minute"));
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_stopped(&pid);
    }

    #[test]
    fn a_client_not_reading_the_prompt_cannot_block_the_writer() {
        let failure = run(shell("sleep 5"), &"x".repeat(1024 * 1024), &short_budget()).unwrap_err();
        assert!(failure.message().contains("10-minute"));
    }

    #[test]
    fn output_limits_stop_the_group_on_either_stream() {
        for (stream, redirect) in [("stdout", "2>/dev/null"), ("stderr", ">&2 2>/dev/null")] {
            let folder = Folder::new();
            let pid = folder.0.join("helper");
            let mut command = shell(&format!(
                "sleep 5 & echo $! > \"$HELPER_PID\"; dd if=/dev/zero bs=1048576 count=9 {redirect}; wait"
            ));
            command.env("HELPER_PID", &pid);
            let failure = run(command, "", &Budget::new()).unwrap_err();
            assert!(
                failure
                    .message()
                    .contains(&format!("{stream} exceeded {OUTPUT_LIMIT} bytes"))
            );
            assert_stopped(&pid);
        }
    }

    #[test]
    fn successful_calls_also_stop_leftover_helpers_without_pipes() {
        let folder = Folder::new();
        let pid = folder.0.join("helper");
        let mut command =
            shell("sleep 5 </dev/null >/dev/null 2>&1 & echo $! > \"$HELPER_PID\"; printf done");
        command.env("HELPER_PID", &pid);
        assert_eq!(run(command, "", &Budget::new()).unwrap().1, "done");
        assert_stopped(&pid);
    }

    #[test]
    fn early_exit_keeps_errors_but_never_reports_an_unread_prompt_as_success() {
        let input = "x".repeat(1024 * 1024);
        let (ok, _, stderr) =
            run(shell("printf denied >&2; exit 1"), &input, &Budget::new()).unwrap();
        assert!(!ok);
        assert_eq!(stderr, "denied");
        let failure = run(shell("exit 0"), &input, &Budget::new()).unwrap_err();
        assert!(failure.message().contains("whole prompt"));
        assert!(!run(shell("kill -TERM $$"), "", &Budget::new()).unwrap().0);
    }

    #[test]
    fn a_spawn_failure_is_returned_and_an_expired_budget_never_starts_a_client() {
        let failure = run(Command::new("/does/not/exist"), "", &Budget::new()).unwrap_err();
        assert!(failure.message().contains("/does/not/exist"));
        let budget = Budget::with_limits(Duration::ZERO, TOTAL_TIMEOUT);
        let failure = run(Command::new("/does/not/exist"), "", &budget).unwrap_err();
        assert!(failure.message().contains("10-minute"));
    }

    #[test]
    fn codex_answer_files_are_bounded_and_must_be_regular_files() {
        let folder = Folder::new();
        let path = folder.0.join("answer");
        fs::write(&path, "{\"work\":\"Debugging\"}").unwrap();
        assert_eq!(
            read_answer(&path, &Budget::new()).unwrap(),
            "{\"work\":\"Debugging\"}"
        );
        File::create(&path)
            .unwrap()
            .set_len(OUTPUT_LIMIT as u64 + 1)
            .unwrap();
        assert!(
            read_answer(&path, &Budget::new())
                .unwrap_err()
                .message()
                .contains("answer file exceeded")
        );
        fs::remove_file(&path).unwrap();
        assert!(
            Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            read_answer(&path, &short_budget())
                .unwrap_err()
                .message()
                .contains("regular file")
        );
        fs::remove_file(&path).unwrap();
        assert!(read_answer(&path, &Budget::new()).is_err());
    }

    #[test]
    fn codex_answer_reading_uses_the_original_response_deadline() {
        let folder = Folder::new();
        let path = folder.0.join("answer");
        fs::write(&path, "{}").unwrap();
        let budget = Budget::with_limits(Duration::ZERO, TOTAL_TIMEOUT);
        assert!(
            read_answer(&path, &budget)
                .unwrap_err()
                .message()
                .contains("10-minute")
        );
    }
}
