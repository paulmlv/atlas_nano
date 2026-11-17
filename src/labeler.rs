//! One labeling call through the installed Claude Code or Codex client.
//!
//! The command lines are the required flags in `docs/LABELING.md`.

mod process;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::model::{Client, PrRole, PullRequest};
use crate::prompt::Prompt;
use crate::taxonomy::Taxonomy;
use process::{Budget, read_answer, run};

/// The reasoning effort of every labeling call. It is part of the label cache key.
pub const EFFORT: &str = "medium";

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
                    "role": {"type": "string", "enum": PrRole::ALL.map(PrRole::name)},
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
fn answer(value: &Value, schema: &Value, budget: &Budget) -> Result<Answer, Failure> {
    budget.check_total()?;
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
        // An entry that names no pull request is dropped; the rest are kept once each.
        let mut prs = Vec::new();
        let mut seen = HashSet::new();
        for pr in listed {
            budget.check_total()?;
            let role = pr["role"].as_str().and_then(PrRole::from_name);
            let (url, role) = match (pr["url"].as_str(), role) {
                (Some(url), Some(role)) => (url, role),
                _ => return Err(invalid()),
            };
            if let Some(url) = pull_request_url(url)
                && seen.insert(url.clone())
            {
                prs.push(PullRequest { url, role });
            }
        }
        prs
    } else {
        Vec::new()
    };
    let answer = Answer {
        work: label("work")?,
        topic: label("topic")?,
        prs,
    };
    budget.check_total()?;
    Ok(answer)
}

/// Per-call files are removed on every return path, including a spawn failure or timeout.
struct CallFiles([PathBuf; 3]);

impl Drop for CallFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = fs::remove_file(path);
        }
    }
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
        let budget = Budget::new();
        let mut spent = Spent::default();
        let answer = match self.client {
            Client::Claude => self.claude(prompt, &mut spent, &budget),
            Client::Codex => self.codex(prompt, &mut spent, &budget),
        };
        let answer = budget.check_total().and(answer);
        Call { answer, spent }
    }

    fn claude(&self, prompt: &str, spent: &mut Spent, budget: &Budget) -> Result<Answer, Failure> {
        let mut command = Command::new("claude");
        // Without this, an `@path` in the conversation makes Claude Code attach that file.
        command.env("CLAUDE_CODE_DISABLE_ATTACHMENTS", "1");
        command.current_dir(self.folder).args([
            "-p",
            "--model",
            self.model,
            "--effort",
            EFFORT,
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
        let (succeeded, stdout, stderr) = run(command, prompt, budget)?;
        budget.check_total()?;
        let result: Value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
        budget.check_total()?;
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
        answer(&result["structured_output"], &self.schema, budget)
    }

    fn codex(&self, prompt: &str, spent: &mut Spent, budget: &Budget) -> Result<Answer, Failure> {
        // Each call has its own files, so parallel calls never read a file another is writing.
        static CALLS: AtomicU64 = AtomicU64::new(0);
        let unique = CALLS.fetch_add(1, Ordering::Relaxed);
        let files = CallFiles([
            self.folder.join(format!("answer-{unique}.json")),
            self.folder.join(format!("schema-{unique}.json")),
            self.folder.join(format!("instructions-{unique}.md")),
        ]);
        let [answer_path, schema_path, instructions_path] = &files.0;
        let write = |path: &Path, text: &str| {
            fs::write(path, text)
                .map_err(|error| Failure::Rejected(format!("{}: {error}", path.display())))
        };
        write(schema_path, &self.schema.to_string())?;
        write(instructions_path, &self.prompt.codex_system)?;
        let mut command = Command::new("codex");
        command
            .current_dir(self.folder)
            .args(["exec", "-m", self.model]);
        let effort = format!("model_reasoning_effort=\"{EFFORT}\"");
        command.args(["-c", &effort, "--ephemeral"]);
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
            .arg(schema_path)
            .arg("-o")
            .arg(answer_path);
        command.args(["--json", "-"]);
        let input = format!("{}\n{prompt}", self.prompt.instructions);
        let (succeeded, stdout, stderr) = run(command, &input, budget)?;
        // Read the file before parsing events: it shares the response-collection deadline.
        let written = read_answer(answer_path, budget);
        let (mut completed, mut failed) = (Value::Null, Value::Null);
        for line in stdout.lines() {
            budget.check_total()?;
            if let Ok(event) = serde_json::from_str::<Value>(line) {
                match event["type"].as_str() {
                    Some("turn.completed") => completed = event,
                    Some("turn.failed") => failed = event,
                    _ => {}
                }
            }
        }
        budget.check_total()?;
        let usage = &completed["usage"];
        let count = |name: &str| usage[name].as_u64().unwrap_or(0);
        *spent = Spent {
            input_tokens: count("input_tokens"),
            cached_tokens: count("cached_input_tokens"),
            output_tokens: count("output_tokens"),
            cost: None,
        };
        // A `Reconnecting...` error event is a retry: only a failed turn or no answer is a failure.
        if !failed.is_null() {
            let reason = failed["error"]["message"]
                .as_str()
                .unwrap_or("the turn failed");
            return Err(Failure::Rejected(format!("codex: {reason}")));
        }
        if !succeeded {
            return Err(Failure::Rejected(format!("codex: {}", last_line(&stderr))));
        }
        let text = written?;
        let value: Value = serde_json::from_str(text.trim())
            .map_err(|error| Failure::Invalid(format!("codex's answer is not JSON: {error}")))?;
        budget.check_total()?;
        answer(&value, &self.schema, budget)
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
    use std::time::Duration;

    #[test]
    fn validation_uses_the_total_deadline_after_the_response_deadline() {
        let schema = schema(&Taxonomy::built_in(), false);
        let value = json!({"work": "Debugging", "topic": "Security"});
        let reading_done = Budget::with_limits(Duration::ZERO, Duration::from_secs(1));
        assert!(answer(&value, &schema, &reading_done).is_ok());
        let expired = Budget::with_limits(Duration::ZERO, Duration::ZERO);
        assert!(
            answer(&value, &schema, &expired)
                .unwrap_err()
                .message()
                .contains("15-minute")
        );
    }

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
            json!(PrRole::ALL.map(PrRole::name))
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
        assert_eq!(
            answer(&value, &schema, &Budget::new()).unwrap().prs,
            [PullRequest {
                url: "https://github.com/o/r/pull/7".into(),
                role: PrRole::Reviewed
            }]
        );
    }

    #[test]
    fn an_answer_must_use_listed_labels() {
        let schema = schema(&Taxonomy::built_in(), true);
        let good = json!({"work": "Debugging", "topic": "Security", "prs": [
            {"url": "https://github.com/o/r/pull/1", "role": "Reviewed"}]});
        assert_eq!(
            answer(&good, &schema, &Budget::new()).unwrap(),
            Answer {
                work: "Debugging".into(),
                topic: "Security".into(),
                prs: vec![PullRequest {
                    url: "https://github.com/o/r/pull/1".into(),
                    role: PrRole::Reviewed
                }]
            }
        );
        // With mapping on, a missing list or an unlisted role makes the whole answer invalid.
        let invented = json!({"work": "Debugging", "topic": "Security", "prs": [
            {"url": "https://github.com/o/r/pull/2", "role": "Invented"}]});
        assert!(matches!(
            answer(&invented, &schema, &Budget::new()),
            Err(Failure::Invalid(_))
        ));
        let missing = json!({"work": "Debugging", "topic": "Security"});
        assert!(matches!(
            answer(&missing, &schema, &Budget::new()),
            Err(Failure::Invalid(_))
        ));
        let without = super::schema(&Taxonomy::built_in(), false);
        assert_eq!(answer(&missing, &without, &Budget::new()).unwrap().prs, []);
        let bad = json!({"work": "Debugging", "topic": "Astrology", "prs": []});
        assert!(matches!(
            answer(&bad, &schema, &Budget::new()),
            Err(Failure::Invalid(_))
        ));
        assert!(matches!(
            answer(&Value::Null, &schema, &Budget::new()),
            Err(Failure::Invalid(_))
        ));
    }
}
