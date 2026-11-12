//! Parse one Claude Code transcript, following the record rules in `docs/LABELING.md`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::timing::{self, Event, Signal};
use super::{Span, known, stem, time};
use crate::model::{Client, Item, Kind, Role, Session, SessionRef, Usage};

const INTERRUPTIONS: [&str; 2] = [
    "[Request interrupted by user]",
    "[Request interrupted by user for tool use]",
];
/// The typed commands Claude Code sends to the model for a reply, which are turns.
const TURN_COMMANDS: [&str; 2] = ["/goal", "!"];

static COMMAND_MESSAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<command-message>.*?</command-message>").unwrap());
static COMMAND_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<command-name>(.*?)</command-name>\s*(?:<command-args>(.*?)</command-args>)?")
        .unwrap()
});
static BASH_INPUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\A<bash-input>(.*?)</bash-input>").unwrap());
static LEADING_REMINDERS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)\A(?:\s*<system-reminder>.*?</system-reminder>)+\s*").unwrap()
});

/// Join a message's text blocks without leading system reminders; images become `[image]`.
fn text(content: &Value) -> String {
    let clean = |text: &str| LEADING_REMINDERS.replace(text, "").into_owned();
    match content {
        Value::String(text) => clean(text).trim().to_string(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| match block["type"].as_str()? {
                "text" => Some(clean(block["text"].as_str()?)),
                "image" => Some("[image]".to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// Return a tagged slash or `!` shell command as typed, such as `/goal Ship it` or `! ls`.
fn typed_command(text: &str) -> Option<String> {
    if text.starts_with("<command-name>") || text.starts_with("<command-message>") {
        let text = COMMAND_MESSAGE.replace_all(text, "");
        let typed = COMMAND_NAME.replace_all(&text, |found: &regex::Captures| {
            let arguments = found.get(2).map_or("", |m| m.as_str()).trim();
            format!("{} {arguments}", found[1].trim())
                .trim()
                .to_string()
        });
        return Some(typed.trim().to_string());
    }
    BASH_INPUT
        .captures(text)
        .map(|shell| format!("! {}", shell[1].trim()))
}

/// What a `user` or `queued_command` record contributes: its role and text, if it is kept.
fn message(origin: &Value, text: String, record: &Value) -> Option<(Role, String)> {
    let kind = origin["kind"].as_str();
    if let Some(kind) = kind.filter(|&kind| kind != "human") {
        let body = origin["body"].as_str().filter(|_| kind == "peer");
        return Some((
            Role::Agent,
            body.map_or(text, |body| body.trim().to_string()),
        ));
    }
    if let Some(typed) = typed_command(&text) {
        return Some((Role::User, typed));
    }
    if record["isMeta"].as_bool() == Some(true)
        || record["isVisibleInTranscriptOnly"].as_bool() == Some(true)
    {
        return None;
    }
    Some((
        if kind == Some("human") {
            Role::User
        } else {
            Role::Agent
        },
        text,
    ))
}

/// Whether a user item is a turn: a typed prompt, or a command the model replies to.
fn is_turn(text: &str) -> bool {
    let command = |word: &str| {
        word == "!"
            || word.strip_prefix('/').is_some_and(|name| {
                !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_alphanumeric() || "_:-".contains(c))
            })
    };
    match text.split_whitespace().next() {
        Some(word) if command(word) => TURN_COMMANDS.contains(&word),
        _ => true,
    }
}

fn usage(reply: &Value, at: chrono::DateTime<chrono::Utc>) -> Option<Usage> {
    let reported = reply.get("usage")?;
    let count = |name: &str| reported[name].as_u64().unwrap_or(0);
    let hour = reported["cache_creation"]["ephemeral_1h_input_tokens"]
        .as_u64()
        .unwrap_or(0);
    Some(Usage {
        id: reply["id"].as_str()?.to_string(),
        at,
        model: reply["model"].as_str().unwrap_or_default().to_string(),
        fast: reported["speed"].as_str() == Some("fast"),
        input: count("input_tokens"),
        cached: count("cache_read_input_tokens"),
        cache_write: count("cache_creation_input_tokens").saturating_sub(hour),
        cache_write_1h: hour,
        output: count("output_tokens"),
    })
}

/// Describe one transcript. A file under a `subagents` directory is a subagent of the session
/// whose directory owns it. A fork is dated from its own records after the copied prefix.
pub fn parse(path: &Path, records: &[Value]) -> Session {
    let owner = path
        .ancestors()
        .find(|folder| folder.file_name().is_some_and(|name| name == "subagents"))
        .and_then(|folder| folder.parent()?.file_name()?.to_str())
        .map(|id| SessionRef {
            client: Client::Claude,
            id: id.to_string(),
        });
    let mut session = Session {
        client: Client::Claude,
        id: stem(path),
        kind: if owner.is_some() {
            Kind::Subagent
        } else {
            Kind::Primary
        },
        parent: owner,
        cwd: String::new(),
        title: None,
        repository: None,
        branch: None,
        started: None,
        ended: None,
        items: Vec::new(),
        tool_calls: 0,
        turns: Vec::new(),
        usage: Vec::new(),
        history_base: None,
    };
    let mut span = Span::default();
    let mut seen: HashSet<&str> = HashSet::new();
    let mut tools: HashSet<&str> = HashSet::new();
    let mut usage_at: HashMap<String, usize> = HashMap::new();
    let mut events: Vec<Event> = Vec::new();
    let mut last_complete: Option<&str> = None;
    let (mut custom_title, mut ai_title) = ("", "");
    for (position, record) in (0u64..).zip(records) {
        if let Some(uuid) = record["uuid"].as_str()
            && !seen.insert(uuid)
        {
            continue;
        }
        let at = record["timestamp"].as_str().and_then(time);
        if session.kind == Kind::Primary
            && (record["forkedFrom"]["sessionId"].is_string()
                || record["sessionId"]
                    .as_str()
                    .is_some_and(|id| id != session.id))
        {
            // Discard timestamps from the copied prefix, including its unmarked records.
            span = Span::default();
        } else {
            span.add(at);
        }
        if session.cwd.is_empty() {
            session.cwd = record["cwd"].as_str().unwrap_or_default().to_string();
        }
        if session.branch.is_none() {
            session.branch = known(&record["gitBranch"]);
        }
        let mut keep = |role: Role, text: String| {
            if !text.is_empty() {
                session.items.push(Item {
                    position,
                    role,
                    text,
                });
            }
        };
        let event = |signal: Signal| Event {
            signal,
            at,
            run: None,
        };
        let record_type = record["type"].as_str().unwrap_or_default();
        if record_type != "assistant" {
            last_complete = None;
        }
        match record_type {
            "custom-title" => custom_title = record["customTitle"].as_str().unwrap_or_default(),
            "ai-title" => ai_title = record["aiTitle"].as_str().unwrap_or_default(),
            "system" if record["subtype"].as_str() == Some("api_error") => {
                events.push(event(Signal::Interrupted));
            }
            "user" if record["isCompactSummary"].as_bool() != Some(true) => {
                let content = &record["message"]["content"];
                let is_result = content
                    .as_array()
                    .is_some_and(|blocks| blocks.iter().any(|b| b["type"] == "tool_result"));
                if content
                    .as_str()
                    .is_some_and(|text| INTERRUPTIONS.contains(&text.trim()))
                {
                    events.push(event(Signal::Interrupted));
                } else if is_result {
                    events.push(event(Signal::Activity));
                }
                if let Some((role, text)) = message(&record["origin"], text(content), record) {
                    if role == Role::User && is_turn(&text) && !text.is_empty() {
                        events.push(event(Signal::Request { steer: false }));
                    }
                    keep(role, text);
                }
            }
            "attachment" if record["attachment"]["type"].as_str() == Some("queued_command") => {
                let queued = &record["attachment"];
                let human = queued["origin"]["kind"].as_str() == Some("human");
                let text = text(&queued["prompt"]);
                if human && !text.is_empty() {
                    events.push(event(Signal::Request { steer: true }));
                }
                keep(if human { Role::User } else { Role::Agent }, text);
            }
            "assistant" => {
                let reply = &record["message"];
                for block in reply["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str() {
                        Some("text") => {
                            let text = block["text"].as_str().unwrap_or_default().trim();
                            keep(Role::Assistant, text.to_string());
                        }
                        Some("tool_use") => {
                            if block["id"].as_str().is_none_or(|id| tools.insert(id)) {
                                session.tool_calls += 1;
                            }
                            let name = block["name"].as_str().unwrap_or_default();
                            keep(Role::Tool, format!("{name} {}", block["input"]));
                        }
                        _ => {}
                    }
                }
                if let Some(at) = at
                    && let Some(reported) = usage(reply, at)
                {
                    // Streaming writes a record per content block, each with the usage so far.
                    match usage_at.get(&reported.id) {
                        Some(&index) => session.usage[index] = reported,
                        None => {
                            usage_at.insert(reported.id.clone(), session.usage.len());
                            session.usage.push(reported);
                        }
                    }
                }
                let done = matches!(
                    reply["stop_reason"].as_str(),
                    Some("end_turn" | "stop_sequence")
                );
                let id = reply["id"].as_str();
                // Each block's record repeats its response's stop reason: one completion, at the last.
                if done && id.is_some() && last_complete == id {
                    events.pop();
                }
                events.push(event(if done {
                    Signal::Complete
                } else {
                    Signal::Activity
                }));
                last_complete = id.filter(|_| done);
            }
            _ => {}
        }
    }
    (session.started, session.ended) = span.bounds();
    session.turns = timing::turns(&events);
    session.title = [custom_title, ai_title]
        .into_iter()
        .find(|t| !t.is_empty())
        .map(str::to_string);
    session
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn parse_main(records: &[Value]) -> Session {
        parse(Path::new("/home/.claude/projects/p/abc.jsonl"), records)
    }

    fn rendered(session: &Session) -> Vec<String> {
        session
            .items
            .iter()
            .map(|i| format!("{} {}", i.role.marker(), i.text))
            .collect()
    }

    #[test]
    fn renders_messages_and_tool_calls_and_drops_the_rest() {
        let session = parse_main(&[
            json!({"type": "user", "uuid": "1", "timestamp": "2026-01-01T00:00:00Z",
                "cwd": "/work/app", "origin": {"kind": "human"},
                "message": {"content": [
                    {"type": "text", "text": "<system-reminder>x</system-reminder>\nFix the test"},
                    {"type": "image"}]}}),
            json!({"type": "assistant", "uuid": "2", "timestamp": "2026-01-01T00:00:05Z",
                "message": {"id": "m1", "model": "claude-haiku-5-5", "stop_reason": "tool_use",
                    "usage": {"input_tokens": 10, "output_tokens": 2},
                    "content": [{"type": "thinking", "thinking": "hm"},
                        {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}}]}}),
            json!({"type": "user", "uuid": "3", "timestamp": "2026-01-01T00:00:06Z",
                "toolUseResult": "big", "message": {"content": [
                    {"type": "tool_result", "content": "file list"}]}}),
            json!({"type": "assistant", "uuid": "4", "timestamp": "2026-01-01T00:00:09Z",
                "message": {"id": "m2", "model": "claude-haiku-5-5", "stop_reason": "end_turn",
                    "usage": {"input_tokens": 20, "output_tokens": 3,
                        "cache_creation_input_tokens": 7,
                        "cache_creation": {"ephemeral_1h_input_tokens": 5}},
                    "content": [{"type": "text", "text": "Done."}]}}),
            json!({"type": "user", "uuid": "5", "isMeta": true, "message": {"content": "context"}}),
            json!({"type": "system", "subtype": "informational", "content": "noise"}),
        ]);
        assert_eq!(
            rendered(&session),
            [
                "[user] Fix the test\n\n[image]",
                r#"[tool] Bash {"command":"ls"}"#,
                "[assistant] Done.",
            ]
        );
        assert_eq!(session.tool_calls, 1);
        assert_eq!(session.cwd, "/work/app");
        assert_eq!(session.turns.len(), 1);
        assert_eq!(session.turns[0].working, Some(chrono::Duration::seconds(9)));
        assert_eq!(session.wall(), Some(chrono::Duration::seconds(9)));
        let last = &session.usage[1];
        assert_eq!(
            (
                last.input,
                last.cache_write,
                last.cache_write_1h,
                last.output
            ),
            (20, 2, 5, 3)
        );
    }

    #[test]
    fn attributes_agents_commands_and_subagents() {
        let records = [
            json!({"type": "user", "uuid": "1", "message": {"content": "Review the diff"}}),
            json!({"type": "user", "uuid": "2", "origin": {"kind": "peer", "body": " report "},
                "message": {"content": "<wrapped>report</wrapped>"}}),
            json!({"type": "user", "uuid": "3", "origin": {"kind": "human"}, "message": {"content":
                "<command-name>/goal</command-name><command-args>Ship it</command-args>"}}),
            json!({"type": "user", "uuid": "4", "origin": {"kind": "human"}, "message": {"content":
                "<command-message>x</command-message><command-name>/clear</command-name>"}}),
            json!({"type": "user", "uuid": "4", "origin": {"kind": "human"},
                "message": {"content": "a repeated record"}}),
            json!({"type": "user", "uuid": "5", "isCompactSummary": true,
                "message": {"content": "summary"}}),
            json!({"type": "attachment", "uuid": "6", "attachment": {"type": "queued_command",
                "origin": {"kind": "human"}, "prompt": "also this"}}),
        ];
        let path = Path::new("/home/.claude/projects/p/parent-id/subagents/agent-1.jsonl");
        let session = parse(path, &records);
        assert_eq!(
            rendered(&session),
            [
                "[agent] Review the diff",
                "[agent] report",
                "[user] /goal Ship it",
                "[user] /clear",
                "[user] also this",
            ]
        );
        assert_eq!(session.kind, Kind::Subagent);
        assert_eq!(
            session.parent.map(|parent| parent.id).as_deref(),
            Some("parent-id")
        );
    }

    #[test]
    fn keeps_the_last_usage_of_a_streamed_response() {
        let block = |uuid: &str, output: u64| {
            json!({"type": "assistant", "uuid": uuid, "timestamp": "2026-01-01T00:00:00Z",
                "message": {"id": "m1", "model": "m", "content": [],
                    "usage": {"input_tokens": 1, "output_tokens": output}}})
        };
        let session = parse_main(&[block("1", 1), block("2", 9)]);
        assert_eq!(session.usage.len(), 1);
        assert_eq!(session.usage[0].output, 9);
    }
}
