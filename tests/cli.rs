//! Run the binary end to end on synthetic sessions, with a fake `claude` standing in for the client.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use chrono::{Duration, Utc};
use serde_json::json;

/// A stand-in for Claude Code: it answers `--help`, logs each call, rejects a conversation that
/// says REJECT, and labels the rest by whether they mention a bug.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$*" in *--help*) echo "  --no-session-persistence"; exit 0 ;; esac
input=$(cat)
echo call >> "$FAKE_LOG"
case "$input" in
  *REJECT*) echo '{"is_error": true, "result": "Not logged in"}'; exit 1 ;;
  *bug*) work="Debugging"; topic="Backend and APIs" ;;
  *) work="Writing code"; topic="Frontend and UI" ;;
esac
echo "{\"is_error\": false, \"structured_output\": {\"work\": \"$work\", \"topic\": \"$topic\"},
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
        let mut child = Command::new(env!("CARGO_BIN_EXE_atlas-nano"))
            .args(arguments)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("FAKE_LOG", self.root.join("calls.log"))
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

    // Blank answers take the defaults: tool calls kept, no pull requests, 32 calls at once once 0
    // is refused, and the minimal theme.
    let configured = sandbox.answer(&["configure"], "\n\n0\n\n\n");
    let said = text(&configured.stdout);
    assert!(
        configured.status.success(),
        "{said}{}",
        text(&configured.stderr)
    );
    assert!(said.contains("codex (Codex/ChatGPT): not installed, skipped"));
    assert!(said.contains("reading Claude Code sessions, the only ones on this computer"));
    assert!(said.contains("(y / n) [n]"));
    assert!(said.contains("please answer a whole number of at least 1"));
    assert!(said.contains("signed-in Claude Code client") && said.contains("not a charge"));
    assert!(said.contains("anthropic/claude-haiku-5-5 labels, through the only client installed"));
    assert!(said.contains("test call with claude-haiku-5-5: Writing code / Frontend and UI"));
    assert!(
        !said.contains("warning:"),
        "one provider, so no warning: {said}"
    );
    let saved = fs::read_to_string(sandbox.root.join("config/atlas-nano/config.toml")).unwrap();
    assert!(saved.contains("coverage = [\"claude\"]"));
    assert!(saved.contains("theme = \"minimal\""));
    assert!(saved.contains("jobs = 32"));
    assert_eq!(sandbox.calls(), 1);

    let csv = sandbox.root.join("out.csv");
    let first = sandbox.run(&["run", "--csv", csv.to_str().unwrap()]);
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
        errors.contains("go through your signed-in Claude Code client"),
        "{errors}"
    );
    assert!(
        errors.contains("claude:refused is unlabeled: claude: Not logged in"),
        "{errors}"
    );
    assert_eq!(sandbox.calls(), 4);
    assert_eq!(csv_column(&csv, "work"), ["Debugging", "Writing code", ""]);
    assert_eq!(csv_column(&csv, "label_from_cache"), ["false", "false", ""]);
    assert_eq!(csv_column(&csv, "turns"), ["1", "1", "1"]);
    assert_eq!(csv_column(&csv, "repo"), ["(unknown)"; 3]);
    assert_eq!(csv_column(&csv, "working_seconds"), ["120", "120", "120"]);

    // A second run reuses the two stored labels and tries the failed session again.
    let second = text(&sandbox.run(&["run"]).stdout);
    assert!(
        second.contains("Labeling run: 0 labeled, 2 from cache, 1 unlabeled"),
        "{second}"
    );
    assert_eq!(sandbox.calls(), 5);

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
    assert_eq!(sandbox.calls(), 9);

    let unlabeled = text(&sandbox.run(&["run", "--no-label"]).stdout);
    assert!(!unlabeled.contains("By type of work") && !unlabeled.contains("Labeling run"));
    assert!(unlabeled.contains("By project"));
    assert_eq!(sandbox.calls(), 9);
}
