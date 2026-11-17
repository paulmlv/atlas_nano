//! Bounded I/O and ownership of one labeling client's process group (macOS and Linux).

use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

use super::Failure;

/// Each output stream and the Codex answer file has its own byte limit.
pub(super) const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
pub(super) const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(super) fn check_deadline(deadline: Instant) -> Result<(), Failure> {
    if Instant::now() >= deadline {
        return Err(Failure::Rejected(
            "the complete labeler response exceeded the 10-minute limit".into(),
        ));
    }
    Ok(())
}

fn active_groups() -> MutexGuard<'static, Vec<Pid>> {
    static GROUPS: Mutex<Vec<Pid>> = Mutex::new(Vec::new());
    GROUPS.lock().unwrap_or_else(|e| e.into_inner())
}

fn listen_for_signals() -> io::Result<()> {
    let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])?;
    thread::Builder::new()
        .name("labeler-signals".into())
        .spawn(move || {
            for signal in signals.forever() {
                // Hold the lock through termination so another call cannot start in between.
                let groups = active_groups();
                for &pid in groups.iter() {
                    let _ = kill_process_group(pid, Signal::KILL);
                }
                let _ = signal_hook::low_level::emulate_default_handler(signal);
            }
        })?;
    Ok(())
}

/// The leader is not reaped until its group is stopped. Keeping its PID reserved also prevents
/// cleanup or the signal listener from ever targeting a reused process-group ID.
struct Client {
    child: Child,
    pid: Pid,
}

impl Client {
    fn spawn(command: &mut Command) -> io::Result<Self> {
        static LISTENER: OnceLock<io::Result<()>> = OnceLock::new();
        LISTENER
            .get_or_init(listen_for_signals)
            .as_ref()
            .map_err(io::Error::other)?;
        let mut active = active_groups();
        let child = command
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let pid = Pid::from_child(&child);
        active.push(pid);
        Ok(Self { child, pid })
    }

    fn stop_group(&self) {
        let _ = kill_process_group(self.pid, Signal::KILL);
    }

    fn finish_if_exited(&self) -> io::Result<Option<bool>> {
        // WNOWAIT leaves the exited leader waitable until Drop has stopped its helpers.
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        match waitid(WaitId::Pid(self.pid), options) {
            Ok(Some(status)) => {
                // Close inherited writers now. The pipes still hold everything the client wrote.
                self.stop_group();
                Ok(Some(status.exit_status() == Some(0)))
            }
            Ok(None) | Err(rustix::io::Errno::INTR) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop_group();
        active_groups().retain(|&pid| pid != self.pid);
        let _ = self.child.wait();
    }
}

fn nonblocking(pipe: &impl AsFd) -> io::Result<()> {
    fcntl_setfl(pipe, fcntl_getfl(pipe)? | OFlags::NONBLOCK)?;
    Ok(())
}

fn retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

/// Read one chunk per pass, so a busy stream cannot starve the other pipe or deadline checks.
fn read(pipe: &mut Option<impl Read>, bytes: &mut Vec<u8>, name: &str) -> io::Result<bool> {
    let Some(source) = pipe else { return Ok(false) };
    let mut buffer = [0; 8192];
    match source.read(&mut buffer) {
        Ok(0) => {
            *pipe = None;
            Ok(true)
        }
        Ok(count) => {
            if bytes.len() + count > OUTPUT_LIMIT {
                return Err(io::Error::other(format!(
                    "{name} exceeded {OUTPUT_LIMIT} bytes"
                )));
            }
            bytes.extend_from_slice(&buffer[..count]);
            Ok(true)
        }
        Err(e) if retryable(&e) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Send stdin and collect both output streams under the same deadline, with no blocking pipe
/// operations or reader threads to leave behind. Drop stops the group and reaps the direct child.
pub(super) fn run(
    mut command: Command,
    input: &str,
    deadline: Instant,
) -> Result<(bool, String, String), Failure> {
    let program = command.get_program().to_string_lossy().into_owned();
    let failed = |e| Failure::Rejected(format!("`{program}`: {e}"));
    check_deadline(deadline)?;
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
        check_deadline(deadline)?;
        if status.is_none() {
            status = client.finish_if_exited().map_err(failed)?;
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
                Err(e) if retryable(&e) => {}
                Err(error) => return Err(failed(error)),
            }
        }
        progressed |= read(&mut stdout, &mut out, "stdout").map_err(failed)?;
        progressed |= read(&mut stderr, &mut err, "stderr").map_err(failed)?;
        if let Some(succeeded) = status
            && stdout.is_none()
            && stderr.is_none()
        {
            check_deadline(deadline)?;
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
            thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(3)
    }

    #[test]
    fn input_and_output_larger_than_pipe_capacity_do_not_deadlock() {
        let input = "prompt ".repeat(50_000);
        let command =
            shell("dd if=/dev/zero bs=1024 count=128 2>/dev/null; cat; printf warning >&2");
        let (ok, stdout, stderr) = run(command, &input, deadline()).unwrap();
        assert!(ok);
        assert_eq!(stdout.len(), 128 * 1024 + input.len());
        assert!(stdout.ends_with(&input));
        assert_eq!(stderr, "warning");
    }

    #[test]
    fn client_exit_preserves_output_even_when_a_helper_inherits_both_pipes() {
        let command = shell("sleep 30 & printf done; printf warning >&2");
        let (ok, stdout, stderr) = run(command, "", deadline()).unwrap();
        assert!(ok);
        assert_eq!(stdout, "done");
        assert_eq!(stderr, "warning");
    }

    #[test]
    fn timing_out_a_client_blocked_on_stdin_stops_its_group() {
        let path = std::env::temp_dir().join(format!("atlas-nano-timeout-{}", std::process::id()));
        let mut command = shell("sleep 30 & echo $! > \"$HELPER_PID\"; wait");
        command.env("HELPER_PID", &path);
        let deadline = Instant::now() + Duration::from_millis(500);
        let failure = run(command, &"x".repeat(1024 * 1024), deadline).unwrap_err();
        assert!(failure.message().contains("10-minute"));

        let pid = std::fs::read_to_string(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let state = Command::new("ps")
                .args(["-p", pid.trim(), "-o", "stat="])
                .output()
                .unwrap();
            // A killed orphan may be a zombie until the system reaps it.
            let state = String::from_utf8_lossy(&state.stdout);
            if state.trim().is_empty() || state.trim().starts_with('Z') {
                break;
            }
            assert!(
                Instant::now() < until,
                "helper {pid} is still running: {state}"
            );
            thread::sleep(POLL_INTERVAL);
        }
    }

    #[test]
    fn either_output_stream_can_exceed_the_limit() {
        for (stream, redirect) in [("stdout", "2>/dev/null"), ("stderr", ">&2 2>/dev/null")] {
            let command = shell(&format!("dd if=/dev/zero bs=1048576 count=9 {redirect}"));
            let failure = run(command, "", Instant::now() + RESPONSE_TIMEOUT).unwrap_err();
            assert!(
                failure
                    .message()
                    .contains(&format!("{stream} exceeded {OUTPUT_LIMIT} bytes"))
            );
        }
    }

    #[test]
    fn early_exit_keeps_errors_but_never_reports_an_unread_prompt_as_success() {
        let input = "x".repeat(1024 * 1024);
        let (ok, _, stderr) = run(shell("printf denied >&2; exit 1"), &input, deadline()).unwrap();
        assert!(!ok);
        assert_eq!(stderr, "denied");
        let failure = run(shell("exit 0"), &input, deadline()).unwrap_err();
        assert!(failure.message().contains("whole prompt"));
        assert!(!run(shell("kill -TERM $$"), "", deadline()).unwrap().0);
    }
}
