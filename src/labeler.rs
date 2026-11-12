//! One labeling call through the installed Claude Code or Codex client.
//!
//! The command lines are the required flags in `docs/LABELING.md`.

use std::fs;
use std::io::{Read, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::model::{Client, PullRequest};
use crate::prompt::Prompt;
use crate::taxonomy::Taxonomy;

const ROLES: [&str; 3] = ["Contributed to", "Reviewed", "Mentioned only"];
const CODEX_DISABLED: [&str; 18] = [
    "shell_tool",
    "unified_exec",
    "view_image",
    "image_generation",
    "multi_agent",
    "goals",
    "memories",
    "apps",
    "plugins",
    "browser_use",
    "computer_use",
    "skill_search",
    "tool_suggest",
    "sleep_tool",
    "in_app_browser",
    "hooks",
    "workspace_dependencies",
    "collaboration_modes",
];

/// Everything a call needs besides the conversation.
pub struct Labeler<'a> {
    pub client: Client,
    pub model: &'a str,
    pub prompt: &'a Prompt,
    pub schema: Value,
    /// An empty folder the client runs in, which also holds the files Codex reads.
    pub folder: &'a Path,
}

/// The labels one call returned.
#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub work: String,
    pub topic: String,
    pub prs: Vec<PullRequest>,
}

/// What one call used, as the client reported it, whether or not it returned a usable answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spent {
    pub input_tokens: u64,
    pub cached_tokens: u64,
    pub output_tokens: u64,
    /// The API-equivalent cost the client reported, if it reports one.
    pub cost: Option<Decimal>,
}

/// One call's answer, or why it gave none, and what it used either way.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub answer: Result<Answer, Failure>,
    pub spent: Spent,
}

/// Why a call produced no label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The client could not run, refused the call, or is not signed in.
    Rejected(String),
    /// The call ran but its answer is missing or does not match the schema.
    Invalid(String),
}

impl Failure {
    pub fn message(&self) -> &str {
        match self {
            Failure::Rejected(message) | Failure::Invalid(message) => message,
        }
    }
}

/// The JSON schema that restricts an answer to the listed labels.
pub fn schema(taxonomy: &Taxonomy, pull_requests: bool) -> Value {
    fn labels(entries: &[crate::taxonomy::Entry]) -> Vec<&str> {
        entries.iter().map(|entry| entry.label.as_str()).collect()
    }
    let mut schema = json!({
        "type": "object",
        "properties": {
            "work": {"type": "string", "enum": labels(&taxonomy.work)},
            "topic": {"type": "string", "enum": labels(&taxonomy.topics)},
        },
        "required": ["work", "topic"],
        "additionalProperties": false,
    });
    if pull_requests {
        schema["properties"]["prs"] = json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "url": {"type": "string"},
                    "role": {"type": "string", "enum": ROLES},
                },
                "required": ["url", "role"],
                "additionalProperties": false,
            },
        });
        schema["required"] = json!(["work", "topic", "prs"]);
    }
    schema
}

/// A GitHub pull request URL in one form, `https://github.com/<owner>/<repo>/pull/<number>`, or
/// `None` if `url` does not name a pull request. Anything after the number, such as `/files` or a
/// `#discussion_...` anchor, is cut, and the owner and repository are lowercased as GitHub ignores
/// their case.
fn pull_request_url(url: &str) -> Option<String> {
    let rest = url.trim();
    let rest = rest
        .strip_prefix("https://")
        .or_else(|| rest.strip_prefix("http://"))?;
    let mut parts = rest.split(['/', '?', '#']);
    let host = parts.next()?.to_lowercase();
    if host != "github.com" && host != "www.github.com" {
        return None;
    }
    let (owner, repo, pull) = (parts.next()?, parts.next()?, parts.next()?);
    let number: u64 = parts.next()?.parse().ok().filter(|number| *number > 0)?;
    if owner.is_empty() || repo.is_empty() || pull != "pull" {
        return None;
    }
    Some(format!(
        "https://github.com/{}/{}/pull/{number}",
        owner.to_lowercase(),
        repo.to_lowercase()
    ))
}

/// Check an answer against the schema's labels and read it.
fn answer(value: &Value, schema: &Value) -> Result<(String, String, Vec<PullRequest>), Failure> {
    let label = |name: &str| -> Result<String, Failure> {
        let allowed = schema["properties"][name]["enum"].as_array();
        match value[name].as_str() {
            Some(label) if allowed.is_some_and(|list| list.iter().any(|l| l == label)) => {
                Ok(label.to_string())
            }
            found => Err(Failure::Invalid(format!(
                "the answer's `{name}` is not a listed label: {found:?}"
            ))),
        }
    };
    // With pull request mapping on, the list is required and each entry must be whole.
    let prs = if schema["properties"].get("prs").is_some() {
        let invalid =
            || Failure::Invalid("the answer's `prs` is not a list of pull requests".into());
        let listed = value["prs"].as_array().ok_or_else(invalid)?;
        let entries = listed
            .iter()
            .map(|pr| {
                let role = pr["role"].as_str().filter(|role| ROLES.contains(role));
                match (pr["url"].as_str(), role) {
                    (Some(url), Some(role)) => Ok((url, role)),
                    _ => Err(invalid()),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        // An entry that names no pull request is dropped; the rest are kept once each.
        let mut prs: Vec<PullRequest> = Vec::new();
        for (url, role) in entries {
            if let Some(url) = pull_request_url(url)
                && !prs.iter().any(|seen| seen.url == url)
            {
                prs.push(PullRequest {
                    url,
                    role: role.to_string(),
                });
            }
        }
        prs
    } else {
        Vec::new()
    };
    Ok((label("work")?, label("topic")?, prs))
}

/// How long one labeling call may run before it is stopped and its session left unlabeled.
const TIMEOUT: Duration = Duration::from_secs(600);

/// Run a client with the conversation on standard input and return its output and whether it
/// succeeded. A call still running after `TIMEOUT` is stopped.
fn run(mut command: Command, input: &str) -> Result<(bool, String, String), Failure> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| Failure::Rejected(format!("could not run `{program}`: {error}")))?;
    let mut stdin = child.stdin.take().expect("standard input is piped");
    let text = input.to_string();
    // Pipes are written and read on their own threads, so a full pipe cannot block the call.
    let writer = thread::spawn(move || stdin.write_all(text.as_bytes()));
    let read = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = read(Box::new(
        child.stdout.take().expect("standard output is piped"),
    ));
    let stderr = read(Box::new(
        child.stderr.take().expect("standard error is piped"),
    ));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < TIMEOUT => thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                // The client stays in this process's group, so Ctrl-C still reaches it and the
                // processes it started.
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::Rejected(format!(
                    "`{program}` was stopped after {} minutes",
                    TIMEOUT.as_secs() / 60
                )));
            }
            Err(error) => return Err(Failure::Rejected(format!("`{program}` failed: {error}"))),
        }
    };
    let _ = writer.join();
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    Ok((status.success(), stdout, stderr))
}

/// The last non-empty line of a client's error output, for a short message.
fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("no error message")
        .trim()
}

impl Labeler<'_> {
    /// Label one conversation, already rendered and wrapped as the prompt.
    pub fn call(&self, prompt: &str) -> Call {
        let mut spent = Spent::default();
        let answer = match self.client {
            Client::Claude => self.claude(prompt, &mut spent),
            Client::Codex => self.codex(prompt, &mut spent),
        };
        Call { answer, spent }
    }

    fn claude(&self, prompt: &str, spent: &mut Spent) -> Result<Answer, Failure> {
        let mut command = Command::new("claude");
        // Without this, an `@path` in the conversation makes Claude Code attach that file.
        command.env("CLAUDE_CODE_DISABLE_ATTACHMENTS", "1");
        command.current_dir(self.folder).args([
            "-p",
            "--model",
            self.model,
            "--effort",
            "medium",
            "--no-session-persistence",
            "--system-prompt",
            &self.prompt.instructions,
            "--tools",
            "",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--setting-sources",
            "",
            "--output-format",
            "json",
            "--json-schema",
            &self.schema.to_string(),
        ]);
        let (succeeded, stdout, stderr) = run(command, prompt)?;
        let result: Value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
        let usage = &result["usage"];
        let count = |name: &str| usage[name].as_u64().unwrap_or(0);
        let cached = count("cache_read_input_tokens");
        *spent = Spent {
            input_tokens: count("input_tokens") + count("cache_creation_input_tokens") + cached,
            cached_tokens: cached,
            output_tokens: count("output_tokens"),
            cost: result["total_cost_usd"]
                .as_f64()
                .and_then(|cost| Decimal::from_str(&cost.to_string()).ok()),
        };
        if !succeeded || result["is_error"].as_bool() == Some(true) {
            let reason = result["result"]
                .as_str()
                .unwrap_or_else(|| last_line(&stderr));
            return Err(Failure::Rejected(format!("claude: {reason}")));
        }
        let (work, topic, prs) = answer(&result["structured_output"], &self.schema)?;
        Ok(Answer { work, topic, prs })
    }

    fn codex(&self, prompt: &str, spent: &mut Spent) -> Result<Answer, Failure> {
        // Each call has its own files, so parallel calls never read a file another is writing.
        let unique = format!("{:?}-{}", thread::current().id(), std::process::id())
            .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-', "");
        let answer_path = self.folder.join(format!("answer-{unique}.json"));
        let schema_path = self.folder.join(format!("schema-{unique}.json"));
        let instructions_path = self.folder.join(format!("instructions-{unique}.md"));
        let write = |path: &Path, text: &str| {
            fs::write(path, text)
                .map_err(|error| Failure::Rejected(format!("{}: {error}", path.display())))
        };
        write(&schema_path, &self.schema.to_string())?;
        write(&instructions_path, &self.prompt.codex_system)?;
        let _ = fs::remove_file(&answer_path);
        let mut command = Command::new("codex");
        command
            .current_dir(self.folder)
            .args(["exec", "-m", self.model]);
        command.args(["-c", "model_reasoning_effort=\"medium\"", "--ephemeral"]);
        command.args(["--skip-git-repo-check", "--sandbox", "read-only"]);
        command.args([
            "--ignore-user-config",
            "--ignore-rules",
            "-c",
            "project_doc_max_bytes=0",
        ]);
        // A TOML string, escaped, since the path may hold backslashes or quotes.
        let path = toml::Value::String(instructions_path.to_string_lossy().into_owned());
        command
            .arg("-c")
            .arg(format!("model_instructions_file={path}"));
        command.args([
            "-c",
            "web_search=\"disabled\"",
            "-c",
            "include_environment_context=false",
        ]);
        for feature in CODEX_DISABLED {
            command.args(["--disable", feature]);
        }
        command
            .arg("--output-schema")
            .arg(&schema_path)
            .arg("-o")
            .arg(&answer_path);
        command.args(["--json", "-"]);
        let input = format!("{}\n{prompt}", self.prompt.instructions);
        let (succeeded, stdout, stderr) = run(command, &input)?;
        let events: Vec<Value> = stdout
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let event = |name: &str| events.iter().rev().find(|event| event["type"] == name);
        let usage = event("turn.completed").map_or(&Value::Null, |event| &event["usage"]);
        let count = |name: &str| usage[name].as_u64().unwrap_or(0);
        *spent = Spent {
            input_tokens: count("input_tokens"),
            cached_tokens: count("cached_input_tokens"),
            output_tokens: count("output_tokens"),
            cost: None,
        };
        let written = fs::read_to_string(&answer_path).ok();
        for path in [&answer_path, &schema_path, &instructions_path] {
            let _ = fs::remove_file(path);
        }
        // A `Reconnecting...` error event is a retry: only a failed turn or no answer is a failure.
        if let Some(failed) = event("turn.failed") {
            let reason = failed["error"]["message"]
                .as_str()
                .unwrap_or("the turn failed");
            return Err(Failure::Rejected(format!("codex: {reason}")));
        }
        if !succeeded {
            return Err(Failure::Rejected(format!("codex: {}", last_line(&stderr))));
        }
        let text = written.ok_or(Failure::Invalid("codex wrote no answer".to_string()))?;
        let value: Value = serde_json::from_str(text.trim())
            .map_err(|error| Failure::Invalid(format!("codex's answer is not JSON: {error}")))?;
        let (work, topic, prs) = answer(&value, &self.schema)?;
        Ok(Answer { work, topic, prs })
    }
}

/// Whether a client is installed and its version has the flag that stops it saving the call.
pub fn no_save_support(client: Client) -> Result<bool, String> {
    let (program, arguments, flag): (&str, &[&str], &str) = match client {
        Client::Claude => ("claude", &["--help"], "--no-session-persistence"),
        Client::Codex => ("codex", &["exec", "--help"], "--ephemeral"),
    };
    let output = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run `{program}`: {error}"))?;
    let help = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(help.contains(flag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_lists_the_labels_and_adds_pull_requests_on_request() {
        let taxonomy = Taxonomy::built_in();
        let plain = schema(&taxonomy, false);
        assert_eq!(
            plain["properties"]["work"]["enum"]
                .as_array()
                .unwrap()
                .len(),
            taxonomy.work.len()
        );
        assert_eq!(plain["required"], json!(["work", "topic"]));
        assert!(plain["properties"].get("prs").is_none());
        let mapped = schema(&taxonomy, true);
        assert_eq!(mapped["required"], json!(["work", "topic", "prs"]));
        assert_eq!(
            mapped["properties"]["prs"]["items"]["properties"]["role"]["enum"],
            json!(ROLES)
        );
    }

    #[test]
    fn pull_request_urls_take_one_form() {
        let one = Some("https://github.com/o/r/pull/7".to_string());
        for url in [
            "https://github.com/o/r/pull/7",
            "https://github.com/O/R/pull/7#discussion_r123",
            "http://www.github.com/o/r/pull/7/files",
            "https://github.com/o/r/pull/7?tab=commits",
        ] {
            assert_eq!(pull_request_url(url), one, "{url}");
        }
        for url in [
            "7",
            "#7",
            "https://github.com/o/r/issues/7",
            "https://github.com/o/r/pull/",
            "https://github.com/o/r/pull/0",
            "https://gitlab.com/o/r/pull/7",
            "https://github.com//r/pull/7",
        ] {
            assert_eq!(pull_request_url(url), None, "{url}");
        }
    }

    #[test]
    fn an_answer_keeps_each_named_pull_request_once() {
        let schema = schema(&Taxonomy::built_in(), true);
        let value = json!({"work": "Debugging", "topic": "Security", "prs": [
            {"url": "7", "role": "Reviewed"},
            {"url": "https://github.com/o/r/pull/7", "role": "Reviewed"},
            {"url": "https://github.com/o/r/pull/7#discussion_r1", "role": "Reviewed"}]});
        let (_, _, prs) = answer(&value, &schema).unwrap();
        assert_eq!(
            prs,
            [PullRequest {
                url: "https://github.com/o/r/pull/7".into(),
                role: "Reviewed".into()
            }]
        );
    }

    #[test]
    fn an_answer_must_use_listed_labels() {
        let schema = schema(&Taxonomy::built_in(), true);
        let good = json!({"work": "Debugging", "topic": "Security", "prs": [
            {"url": "https://github.com/o/r/pull/1", "role": "Reviewed"}]});
        let (work, topic, prs) = answer(&good, &schema).unwrap();
        assert_eq!((work.as_str(), topic.as_str()), ("Debugging", "Security"));
        assert_eq!(
            prs,
            [PullRequest {
                url: "https://github.com/o/r/pull/1".into(),
                role: "Reviewed".into()
            }]
        );
        // With mapping on, a missing list or an unlisted role makes the whole answer invalid.
        let invented = json!({"work": "Debugging", "topic": "Security", "prs": [
            {"url": "https://github.com/o/r/pull/2", "role": "Invented"}]});
        assert!(matches!(
            answer(&invented, &schema),
            Err(Failure::Invalid(_))
        ));
        let missing = json!({"work": "Debugging", "topic": "Security"});
        assert!(matches!(
            answer(&missing, &schema),
            Err(Failure::Invalid(_))
        ));
        let without = super::schema(&Taxonomy::built_in(), false);
        assert_eq!(answer(&missing, &without).unwrap().2, []);
        let bad = json!({"work": "Debugging", "topic": "Astrology", "prs": []});
        assert!(matches!(answer(&bad, &schema), Err(Failure::Invalid(_))));
        assert!(matches!(
            answer(&Value::Null, &schema),
            Err(Failure::Invalid(_))
        ));
    }
}
