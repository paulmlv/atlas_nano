//! Run the binary end to end on synthetic sessions, with a fake `claude` standing in for the client.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration as WallDuration, Instant};

use chrono::{Duration, Utc};
use serde_json::json;

/// A stand-in for Claude Code: it answers `--help`, logs each call, rejects a conversation that
/// says REJECT, rejects one that says FLAKY only the first time, and labels the rest by whether
/// they mention a bug; a bug comes with a pull request.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$*" in *--help*) echo "  --no-session-persistence"; exit 0 ;; esac
input=$(cat)
echo call >> "$FAKE_LOG"
prs=""
case "$input" in
  *REJECT*) echo '{"is_error": true, "result": "Not logged in"}'; exit 1 ;;
  *FLAKY*) if [ ! -e "$FAKE_LOG.flaky" ]; then
      touch "$FAKE_LOG.flaky"; echo '{"is_error": true, "result": "Overloaded"}'; exit 1
    fi
    work="Writing code"; topic="Frontend and UI" ;;
  *bug*) work="Debugging"; topic="Backend and APIs"
    prs='{"url": "https://github.com/owner/repo/pull/7", "role": "Contributed to"}' ;;
  *) work="Writing code"; topic="Frontend and UI" ;;
esac
echo "{\"is_error\": false, \"structured_output\": {\"work\": \"$work\", \"topic\": \"$topic\", \"prs\": [$prs]},
  \"usage\": {\"input_tokens\": 100, \"output_tokens\": 5}, \"total_cost_usd\": 0.01}"
"#;

struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Sandbox {
        let root =
            std::env::temp_dir().join(format!("atlas-nano-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("bin")).unwrap();
        let fake = root.join("bin/claude");
        fs::write(&fake, FAKE_CLAUDE).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        Sandbox { root }
    }

    /// Write a Claude Code session of one request and one reply, started `hours_ago`.
    fn session(&self, id: &str, request: &str, hours_ago: i64) {
        let at = |minutes: i64| {
            (Utc::now() - Duration::hours(hours_ago) + Duration::minutes(minutes)).to_rfc3339()
        };
        let records = [
            json!({"type": "user", "uuid": format!("{id}-1"), "timestamp": at(0), "cwd": "/work/app",
                "origin": {"kind": "human"}, "message": {"content": request}}),
            json!({"type": "assistant", "uuid": format!("{id}-2"), "timestamp": at(2),
                "message": {"id": format!("{id}-m"), "model": "claude-haiku-5-5", "stop_reason": "end_turn",
                    "usage": {"input_tokens": 1000, "output_tokens": 1_000_000},
                    "content": [{"type": "text", "text": "Done."}]}}),
        ];
        let folder = self.root.join("home/.claude/projects/p");
        fs::create_dir_all(&folder).unwrap();
        let lines: Vec<String> = records.iter().map(|record| record.to_string()).collect();
        fs::write(folder.join(format!("{id}.jsonl")), lines.join("\n")).unwrap();
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.answer(arguments, "")
    }

    /// Run the binary with `input` typed on its standard input, then closed.
    fn answer(&self, arguments: &[&str], input: &str) -> Output {
        let mut child = self
            .command(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_atlas-nano"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("FAKE_LOG", self.root.join("calls.log"));
        command
    }

    fn calls(&self) -> usize {
        fs::read_to_string(self.root.join("calls.log")).map_or(0, |log| log.lines().count())
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn csv_column(path: &Path, column: &str) -> Vec<String> {
    let file = fs::read_to_string(path).unwrap();
    let mut lines = file.lines();
    let at = lines
        .next()
        .unwrap()
        .split(',')
        .position(|name| name == column)
        .unwrap();
    lines
        .map(|line| line.split(',').nth(at).unwrap().to_string())
        .collect()
}

#[test]
fn run_needs_a_configuration() {
    let sandbox = Sandbox::new("unconfigured");
    let output = sandbox.run(&["run"]);
    assert!(!output.status.success());
    assert!(text(&output.stderr).contains("run `atlas-nano configure` first"));
}

#[test]
fn interrupting_atlas_stops_all_active_labelers_and_their_helpers() {
    use rustix::process::{Pid, Signal, kill_process};

    for signal in [Signal::INT, Signal::TERM] {
        let sandbox = Sandbox::new(&format!("interrupted-{}", signal.as_raw()));
        sandbox.session("first", "First request", 1);
        sandbox.session("second", "Second request", 1);
        let configured = sandbox.answer(&["configure"], "\n\n2\n\n\n\n");
        assert!(
            configured.status.success(),
            "{}\n{}",
            text(&configured.stdout),
            text(&configured.stderr)
        );
        fs::write(
            sandbox.root.join("bin/claude"),
            r#"#!/bin/sh
cat >/dev/null
echo $$ >> "$FAKE_LOG.active"
sleep 30 &
echo $! >> "$FAKE_LOG.helpers"
wait
"#,
        )
        .unwrap();
        let mut child = sandbox
            .command(&["run", "--no-cache"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let ready = Instant::now() + WallDuration::from_secs(5);
        while fs::read_to_string(sandbox.root.join("calls.log.helpers"))
            .map_or(0, |text| text.lines().count())
            < 2
        {
            if Instant::now() >= ready || child.try_wait().unwrap().is_some() {
                let _ = kill_process(Pid::from_child(&child), Signal::TERM);
                let _ = child.wait();
                panic!("both fake labelers did not start");
            }
            thread::sleep(WallDuration::from_millis(10));
        }
        kill_process(Pid::from_child(&child), signal).unwrap();
        let until = Instant::now() + WallDuration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= until {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Atlas did not terminate on {signal:?}");
            }
            thread::sleep(WallDuration::from_millis(10));
        };
        assert_eq!(status.signal(), Some(signal.as_raw()));
        for file in ["calls.log.active", "calls.log.helpers"] {
            for pid in fs::read_to_string(sandbox.root.join(file)).unwrap().lines() {
                let until = Instant::now() + WallDuration::from_secs(2);
                loop {
                    let state = Command::new("ps")
                        .args(["-p", pid, "-o", "stat="])
                        .output()
                        .unwrap();
                    let state = text(&state.stdout);
                    if state.trim().is_empty() || state.trim().starts_with('Z') {
                        break;
                    }
                    assert!(
                        Instant::now() < until,
                        "fake process {pid} survived {signal:?}: {state}"
                    );
                    thread::sleep(WallDuration::from_millis(10));
                }
            }
        }
    }
}

#[test]
fn codex_collects_its_answer_and_rejects_oversized_or_special_files() {
    let sandbox = Sandbox::new("codex-answer");
    sandbox.session("plain", "A plain request", 1);
    assert!(
        sandbox
            .answer(&["configure"], "\n\n\n\n\n\n")
            .status
            .success()
    );
    let config = sandbox.root.join("config/atlas-nano/config.toml");
    let mut saved =
        atlas_nano::config::Config::decode(&fs::read_to_string(&config).unwrap()).unwrap();
    saved.labeler.client = atlas_nano::model::Client::Codex;
    saved.labeler.model = "gpt-6-luna".into();
    fs::write(config, saved.encode()).unwrap();
    let fake = sandbox.root.join("bin/codex");
    fs::write(
        &fake,
        r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then shift; answer=$1; fi
  shift
done
cat >/dev/null
case "$FAKE_MODE" in
  oversized) dd if=/dev/zero of="$answer" bs=1048576 count=9 2>/dev/null ;;
  fifo) mkfifo "$answer" ;;
  *) echo '{"work":"Debugging","topic":"Security","prs":[]}' > "$answer" ;;
esac
echo '{"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":5}}'
"#,
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    for (mode, expected) in [
        ("ok", "1 labeled, 0 from cache, 0 unlabeled"),
        ("oversized", "0 labeled, 0 from cache, 1 unlabeled"),
        ("fifo", "0 labeled, 0 from cache, 1 unlabeled"),
    ] {
        let output = sandbox
            .command(&["run", "--no-cache", "--retries", "0"])
            .env("FAKE_MODE", mode)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert!(
            text(&output.stdout).contains(expected),
            "{}",
            text(&output.stdout)
        );
        if mode == "oversized" {
            assert!(text(&output.stderr).contains("answer file exceeded"));
        } else if mode == "fifo" {
            assert!(text(&output.stderr).contains("regular file"));
        }
    }
}

#[test]
fn a_failed_call_is_retried_unless_retries_are_off() {
    let sandbox = Sandbox::new("retried");
    sandbox.session("flaky", "A FLAKY request", 1);
    assert!(
        sandbox
            .answer(&["configure"], "\n\n\n\n\n\n")
            .status
            .success()
    );
    assert_eq!(sandbox.calls(), 1);

    // With retries off, the first failure leaves the session unlabeled.
    let off = sandbox.run(&["run", "--retries", "0"]);
    let (report, errors) = (text(&off.stdout), text(&off.stderr));
    assert!(
        report.contains("0 labeled, 0 from cache, 1 unlabeled"),
        "{report}"
    );
    assert!(!errors.contains("Retrying"), "{errors}");
    assert_eq!(sandbox.calls(), 2);

    // The saved retry tries it again after a failure, and the second try labels it.
    fs::remove_file(sandbox.root.join("calls.log.flaky")).unwrap();
    let on = sandbox.run(&["run"]);
    let (report, errors) = (text(&on.stdout), text(&on.stderr));
    assert!(
        report.contains("1 labeled, 0 from cache, 0 unlabeled"),
        "{report}"
    );
    assert!(errors.contains("Retrying 1 failed calls"), "{errors}");
    assert_eq!(sandbox.calls(), 4);
}

#[test]
fn more_calls_at_once_than_the_computer_runs_are_lowered() {
    let sandbox = Sandbox::new("lowered");
    sandbox.session("plain", "A plain request", 1);
    assert!(
        sandbox
            .answer(&["configure"], "\n\n\n\n\n\n")
            .status
            .success()
    );
    let path = sandbox.root.join("config/atlas-nano/config.toml");
    let saved = fs::read_to_string(&path).unwrap();
    let most = atlas_nano::label::jobs_limit();
    fs::write(
        &path,
        saved.replace(&format!("jobs = {most}"), "jobs = 100000"),
    )
    .unwrap();
    let errors = text(&sandbox.run(&["run"]).stderr);
    assert!(
        errors.contains(&format!("jobs = 100000 is more calls at once than this computer can run without slowing down, so {most} run at once")),
        "{errors}"
    );
}

#[test]
fn configure_then_run_labels_reports_and_reuses_the_cache() {
    let sandbox = Sandbox::new("labeled");
    sandbox.session("bug", "Please fix the bug in the server", 1);
    sandbox.session("button", "Add a button to the page", 2);
    sandbox.session("refused", "REJECT this one", 3);
    sandbox.session("old", "An old request about a bug", 24 * 30);

    // Ending the input before every question is answered saves nothing.
    let cancelled = sandbox.answer(&["configure"], "\n");
    assert!(!cancelled.status.success());
    assert!(text(&cancelled.stderr).contains("nothing was saved"));
    assert!(!sandbox.root.join("config/atlas-nano/config.toml").exists());

    // Blank answers take the defaults: tool calls kept, pull requests mapped, once 0 is refused the
    // most calls at once this computer runs, 10 rows in the ranked tables, the interactive view, and
    // the minimal theme.
    let configured = sandbox.answer(&["configure"], "\n\n0\n\n\n\n\n");
    let said = text(&configured.stdout);
    assert!(
        configured.status.success(),
        "{said}{}",
        text(&configured.stderr)
    );
    assert!(said.contains("No Codex/ChatGPT installation detected, skipped"));
    assert!(said.contains("reading Claude Code sessions, the only ones on this computer"));
    assert!(
        said.contains("Include tool calls for labeling (more accurate, costs more)? (y / n) [y]")
    );
    assert!(said.contains("please answer a whole number of at least 1"));
    assert!(
        said.contains(
            "Show the report in an interactive view (tui) or print it? (tui / print) [tui]"
        )
    );
    assert!(said.contains("We label conversations with your signed-in account"));
    assert!(said.contains("anthropic/claude-haiku-5-5 labels, through the only client installed"));
    assert!(said.contains("test call with claude-haiku-5-5: Writing code / Frontend and UI"));
    assert!(
        !said.contains("warning:"),
        "one provider, so no warning: {said}"
    );
    let saved = fs::read_to_string(sandbox.root.join("config/atlas-nano/config.toml")).unwrap();
    assert!(saved.contains("coverage = [\"claude\"]"));
    assert!(saved.contains("theme = \"minimal\""));
    assert!(saved.contains("top = \"10\""));
    assert!(saved.contains("pull_requests = true"));
    assert!(saved.contains(&format!("jobs = {}", atlas_nano::label::jobs_limit())));
    assert!(saved.contains("retries = 1"));
    assert!(saved.contains("output = \"tui\""));
    assert_eq!(sandbox.calls(), 1);

    // The view needs a terminal: asked for here, it stops before labeling; saved, it prints.
    let piped = sandbox.run(&["run", "--output", "tui"]);
    assert!(!piped.status.success());
    assert!(text(&piped.stderr).contains("--output tui needs a terminal"));
    assert_eq!(sandbox.calls(), 1);

    let csv = sandbox.root.join("out.csv");
    let json = sandbox.root.join("out.jsonl");
    let first = sandbox.run(&[
        "run",
        "--csv",
        csv.to_str().unwrap(),
        "--json",
        json.to_str().unwrap(),
    ]);
    let (report, errors) = (text(&first.stdout), text(&first.stderr));
    assert!(first.status.success(), "{errors}");
    assert!(
        report.starts_with("atlas-nano · last 7 days · 3 conversations · 0 subagents"),
        "{report}"
    );
    for expected in [
        "By type of work",
        "    Debugging",
        "    Writing code",
        "  Unlabeled",
        "By topic",
        "    Backend and APIs",
        // The sessions ran in `/work/app`, which no longer exists.
        "By project",
        "  (unknown)",
        "By repo",
        "Labeling run: 2 labeled, 0 from cache, 1 unlabeled · claude-haiku-5-5",
        "in 200 · out 10 tokens · API-equivalent $0.02",
        // Three sessions of 1,000 input and 1,000,000 output Haiku tokens each.
        "Spend (API-equivalent)  $1.50",
        "By project (top 10)",
        "Top 10 conversations by spend",
        "Top 10 pull requests by spend (~ marks an amount split across several)",
        "https://github.com/owner/repo/pull/7",
    ] {
        assert!(
            report.contains(expected),
            "missing `{expected}` in:\n{report}"
        );
    }
    assert!(
        errors.contains("Labeling 3 sessions in 3 calls"),
        "{errors}"
    );
    assert!(
        errors.contains("We label conversations with your signed-in account"),
        "{errors}"
    );
    assert!(
        errors.contains("claude:refused is unlabeled: claude: Not logged in"),
        "{errors}"
    );
    // The refused call is tried once more.
    assert!(errors.contains("Retrying 1 failed calls"), "{errors}");
    assert_eq!(sandbox.calls(), 5);
    assert_eq!(csv_column(&csv, "work"), ["Debugging", "Writing code", ""]);
    assert_eq!(csv_column(&csv, "label_from_cache"), ["false", "false", ""]);
    assert_eq!(csv_column(&csv, "turns"), ["1", "1", "1"]);
    assert_eq!(csv_column(&csv, "repo"), ["(unknown)"; 3]);
    assert_eq!(csv_column(&csv, "working_seconds"), ["120", "120", "120"]);
    // The JSON holds the same sessions in the same order, with the label nested.
    let lines: Vec<serde_json::Value> = fs::read_to_string(&json)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let work: Vec<_> = lines.iter().map(|line| &line["label"]["work"]).collect();
    assert_eq!(
        work,
        [
            &"Debugging".into(),
            &"Writing code".into(),
            &serde_json::Value::Null
        ]
    );
    assert_eq!(lines[0]["turns"], 1);
    assert_eq!(lines[0]["working_seconds"], 120);
    assert_eq!(lines[0]["repo"], "(unknown)");
    assert_eq!(lines[0]["parent"], serde_json::Value::Null);
    assert_eq!(lines[0]["tokens"]["output"], 1_000_000);
    assert_eq!(lines[0]["label"]["from_cache"], false);

    // A second run reuses the two stored labels and tries the failed session again.
    let second = text(&sandbox.run(&["run"]).stdout);
    assert!(
        second.contains("Labeling run: 0 labeled, 2 from cache, 1 unlabeled"),
        "{second}"
    );
    assert_eq!(sandbox.calls(), 7);

    let relabeled = text(
        &sandbox
            .run(&["run", "--no-cache", "--period", "all"])
            .stdout,
    );
    assert!(
        relabeled.contains("all time · 4 conversations"),
        "{relabeled}"
    );
    assert!(
        relabeled.contains("Labeling run: 3 labeled, 0 from cache, 1 unlabeled"),
        "{relabeled}"
    );
    assert_eq!(sandbox.calls(), 12);

    let unlabeled = text(&sandbox.run(&["run", "--no-label"]).stdout);
    assert!(!unlabeled.contains("By type of work") && !unlabeled.contains("Labeling run"));
    assert!(unlabeled.contains("By project"));
    assert_eq!(sandbox.calls(), 12);

    // A reply with no usage warns, but only in a period that covers it.
    sandbox.session(
        "no-usage",
        "A request whose usage was not recorded",
        24 * 30,
    );
    let file = sandbox.root.join("home/.claude/projects/p/no-usage.jsonl");
    let lines = fs::read_to_string(&file)
        .unwrap()
        .replace("\"usage\"", "\"moved\"");
    fs::write(&file, lines).unwrap();
    let recent = text(&sandbox.run(&["run", "--no-label"]).stderr);
    assert!(!recent.contains("have replies but no usage"), "{recent}");
    let all = text(
        &sandbox
            .run(&["run", "--no-label", "--period", "all"])
            .stderr,
    );
    let missing = "1 Claude Code session(s) have replies but no usage";
    assert!(all.contains(missing), "{all}");
}
