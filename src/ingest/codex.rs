//! Parse one Codex rollout, following the record rules in `docs/LABELING.md`.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;

use super::timing::{self, Event, Signal};
use super::{Span, known, stem, time};
use crate::model::{Client, HistoryBase, Item, Kind, Role, Session, SessionRef, Usage};

const TOOL_CALLS: [&str; 6] = [
    "function_call",
    "custom_tool_call",
    "local_shell_call",
    "tool_search_call",
    "web_search_call",
    "image_generation_call",
];
const WORK: [&str; 3] = [
    "reasoning",
    "function_call_output",
    "custom_tool_call_output",
];
/// The record types `parse` reads besides the tool calls and work above, named as `label` names
/// them.
const READ: &[&str] = &[
    "session_meta",
    "turn_context",
    "event_msg/task_started",
    "event_msg/task_complete",
    "event_msg/turn_aborted",
    "event_msg/token_count",
    "event_msg/thread_goal_updated",
    "token_usage_record",
    "response_item/message",
    "response_item/agent_message",
];
/// The record types left out on purpose: none holds the conversation, its spend, or its timing.
const DROPPED: &[&str] = &[
    // A summary of items already read as response items.
    "event_msg/item_completed",
    // Settings and tool lists.
    "event_msg/thread_settings_applied",
    "response_item/additional_tools",
    "response_item/configuration_update",
    // Compacted history.
    "compacted",
    "response_item/compaction",
    // Internal state.
    "world_state",
    "inter_agent_communication_metadata",
];

/// A record's type, with an event's or a response item's own type.
fn label(record: &Value) -> String {
    let kind = record["type"].as_str().unwrap_or("(no type)");
    match (kind, record["payload"]["type"].as_str()) {
        ("event_msg" | "response_item", Some(detail)) => format!("{kind}/{detail}"),
        _ => kind.to_string(),
    }
}

/// The type of a record `parse` neither reads nor leaves out on purpose.
pub fn unknown(record: &Value) -> Option<String> {
    let label = label(record);
    let read = READ.contains(&label.as_str())
        || label
            .strip_prefix("response_item/")
            .is_some_and(|item| TOOL_CALLS.contains(&item) || WORK.contains(&item));
    (!read && !DROPPED.contains(&label.as_str())).then_some(label)
}

const QUESTION_REPLY: &str = "<send_user_message_question_reply>";
const INTERNAL_CONTEXT: &str = "<codex_internal_context";
const GOAL_CONTEXT: &str = "<codex_internal_context source=\"goal\"";

static SHELL_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)\A<user_shell_command>\s*<command>\s*(.*?)\s*</command>").unwrap()
});
static IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<image\b[^>]*>.*?</image>").unwrap());

/// Return a rollout file's thread ID and its own rollout ID.
///
/// Rollouts are named `rollout-<time>-<thread ID>[_<rollout ID>]`; a thread's first rollout shares
/// the thread's ID.
pub fn ids(path: &Path) -> (String, String) {
    let stem = stem(path);
    let (thread, rollout) = stem.split_once('_').unwrap_or((&stem, ""));
    // The last 36 bytes are the ID; a name that is not a rollout's may not split there.
    let tail = |text: &str| {
        let start = text.len().saturating_sub(36);
        text.get(start..).unwrap_or(text).to_string()
    };
    (
        tail(thread),
        tail(if rollout.is_empty() { thread } else { rollout }),
    )
}

/// Join a message's text blocks; images become `[image]`.
fn text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| match block["type"].as_str()? {
                "input_text" | "output_text" | "text" => block["text"].as_str().map(str::to_string),
                "input_image" | "image" => Some("[image]".to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    }
}

/// Read an answer to the model's question, and remove the IDE context before a request.
fn request(text: &str) -> String {
    if let Some(body) = text.strip_prefix(QUESTION_REPLY) {
        let body = body
            .strip_suffix("</send_user_message_question_reply>")
            .unwrap_or(body);
        let replies: Vec<Value> = serde_json::from_str(body).unwrap_or_default();
        let answers = replies.iter().filter_map(|reply| reply["answer"].as_str());
        return answers
            .map(|answer| format!("Answer: {answer}"))
            .collect::<Vec<_>>()
            .join("\n");
    }
    let text = text
        .split_once("## My request:")
        .map_or(text, |(_, request)| request);
    IMAGE.replace_all(text, "[image]").trim().to_string()
}

/// One model response's usage, from Codex's token counts and an ID that a fork's copy keeps.
fn usage(id: String, counts: &Value, timestamp: &str, model: &str) -> Option<Usage> {
    let count = |name: &str| counts[name].as_u64().unwrap_or(0);
    let (cached, cache_write) = (
        count("cached_input_tokens"),
        count("cache_write_input_tokens"),
    );
    Some(Usage {
        id,
        owner: None,
        at: time(timestamp)?,
        model: model.to_string(),
        fast: false,
        input: count("input_tokens").saturating_sub(cached + cache_write),
        cached,
        cache_write,
        cache_write_1h: 0,
        output: count("output_tokens"),
    })
}

fn kind(thread_source: Option<&str>) -> Kind {
    match thread_source {
        Some("guardian_review") => Kind::Review,
        Some("subagent" | "agent_created_thread") => Kind::Subagent,
        _ => Kind::Primary,
    }
}

/// The rollout's own creation time, independent of the dates in its history.
pub(super) struct Version {
    thread: String,
    created: DateTime<Utc>,
    parent_is_thread: bool,
}

/// Apply the parent rollout assumption in `docs/ASSUMPTIONS.md` after loading all versions.
pub(super) fn link_parents(sessions: &mut [Session], versions: &[Option<Version>]) {
    let mut by_thread: HashMap<&str, Vec<_>> = HashMap::new();
    for (session, version) in sessions.iter().zip(versions) {
        if let Some(version) = version {
            by_thread
                .entry(&version.thread)
                .or_default()
                .push((version.created, session.id.as_str()));
        }
    }
    let links: Vec<_> = sessions
        .iter()
        .zip(versions)
        .enumerate()
        .filter_map(|(n, (session, version))| {
            let version = version
                .as_ref()
                .filter(|version| version.parent_is_thread)?;
            let parent = session.parent.as_ref()?.id.as_str();
            if parent == version.thread {
                return None;
            }
            let candidates = by_thread.get(parent)?;
            let &(created, rollout) = candidates
                .iter()
                .filter(|(created, _)| *created <= version.created)
                .max()?;
            let tied = candidates
                .iter()
                .any(|&(at, id)| at == created && id != rollout);
            (rollout != parent && !tied).then(|| (n, rollout.to_string()))
        })
        .collect();
    for (n, id) in links {
        sessions[n].parent = Some(SessionRef {
            client: Client::Codex,
            id,
        });
    }
}

/// Describe one rollout from its own records. Each rollout is its own session, so an edit of a
/// thread is a separate conversation whose parent is the thread's first rollout.
pub(super) fn parse(path: &Path, records: &[Value]) -> (Session, Option<Version>) {
    let (thread, rollout) = ids(path);
    let null = Value::Null;
    let header = records
        .iter()
        .find(|record| record["type"] == "session_meta");
    let meta = header.map_or(&null, |record| &record["payload"]);
    let history_base = meta["history_base"]["thread_id"].as_str().and_then(|base| {
        let end = meta["history_base"]["end_ordinal_exclusive"].as_u64()?;
        (end > 0).then(|| HistoryBase {
            rollout: base.to_string(),
            end,
        })
    });
    let linked = [
        &meta["parent_thread_id"],
        &meta["forked_from_id"],
        &meta["source"]["subagent"]["thread_spawn"]["parent_thread_id"],
    ];
    let exact_parent = history_base
        .as_ref()
        .map(|base| base.rollout.clone())
        .or_else(|| (rollout != thread).then(|| thread.clone()));
    let kind = kind(meta["thread_source"].as_str());
    let parent_is_thread = kind != Kind::Primary && exact_parent.is_none();
    let parent = exact_parent
        .or_else(|| linked.iter().find_map(|id| id.as_str()).map(str::to_string))
        .map(|id| SessionRef {
            client: Client::Codex,
            id,
        });
    let mut session = Session {
        client: Client::Codex,
        id: rollout,
        parent,
        kind,
        cwd: known(&meta["cwd"]),
        title: None,
        repository: known(&meta["git"]["repository_url"]),
        branch: known(&meta["git"]["branch"]),
        started: None,
        ended: None,
        items: Vec::new(),
        tool_calls: 0,
        turns: Vec::new(),
        usage: Vec::new(),
        history_base,
    };
    let mut span = Span::default();
    let mut events: Vec<Event> = Vec::new();
    let mut run: Option<String> = None; // the run later records belong to
    let mut turn: Option<String> = None; // the recorded run, which a fork's copy keeps
    let mut run_closed = false;
    let mut running = false; // whether a turn has started and not ended
    let mut turn_requests = 0u32; // a user message in a turn that already has a request is a steer
    let mut model = String::new();
    let mut turn_models: HashMap<&str, &str> = HashMap::new();
    let mut unknown_model: Vec<(usize, &str)> = Vec::new(); // responses read before their turn's model
    let mut finished: HashSet<&str> = HashSet::new();
    let mut totals = &null;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut tools: HashSet<&str> = HashSet::new();
    let mut goals: HashSet<i64> = HashSet::new();
    let mut typed_goals: Vec<String> = Vec::new();
    // Newer clients record each response's usage; older ones only a running total. A rollout
    // resumed by a newer client holds both kinds, so the choice is made per turn.
    let per_response: HashSet<&str> = records
        .iter()
        .filter(|record| record["type"] == "token_usage_record")
        .filter_map(|record| record["payload"]["turn_id"].as_str())
        .collect();
    for (line, record) in (0u64..).zip(records) {
        let position = record["ordinal"].as_u64().unwrap_or(line);
        let timestamp = record["timestamp"].as_str().unwrap_or_default();
        let at = time(timestamp);
        span.add(at);
        let (record_type, payload) = (
            record["type"].as_str().unwrap_or_default(),
            &record["payload"],
        );
        let subtype = payload["type"].as_str().unwrap_or_default();
        let role = payload["role"].as_str().unwrap_or_default();
        let is_message = record_type == "response_item" && subtype == "message";
        match (record_type, subtype) {
            ("event_msg", "task_started") => {
                turn = payload["turn_id"].as_str().map(str::to_string);
                run = turn.clone().or_else(|| Some(format!("record {position}")));
                run_closed = false;
            }
            ("turn_context", _) if payload["turn_id"].is_string() => {
                turn = payload["turn_id"].as_str().map(str::to_string);
                run.clone_from(&turn);
                run_closed = false;
            }
            // Older histories can omit a new task_started: do not reuse the completed run.
            _ if run_closed && is_message && role == "user" => run = None,
            _ => {}
        }
        let event_run = payload["turn_id"]
            .as_str()
            .map(str::to_string)
            .or_else(|| run.clone());
        let mut signal = |signal: Signal, run: Option<String>| {
            events.push(Event { signal, at, run });
        };
        let mut keep = |role: Role, text: String| {
            if !text.is_empty() {
                session.items.push(Item {
                    position,
                    role,
                    text,
                });
            }
        };
        match (record_type, subtype) {
            ("turn_context", _) => {
                if let Some(name) = payload["model"].as_str() {
                    model = name.to_string();
                    if let Some(id) = payload["turn_id"].as_str() {
                        turn_models.insert(id, name);
                    }
                }
            }
            ("event_msg", "task_started" | "task_complete" | "turn_aborted") => {
                if subtype != "task_started" {
                    finished.extend(payload["turn_id"].as_str());
                }
                let done = subtype == "task_complete" && payload["error"].is_null();
                let active = event_run == run;
                if subtype == "task_started" {
                    signal(Signal::Activity, event_run);
                } else {
                    run_closed |= active;
                    signal(
                        if done {
                            Signal::Complete
                        } else {
                            Signal::Interrupted
                        },
                        event_run,
                    );
                }
                // A late end of an earlier run does not stop the current one.
                if subtype == "task_started" || active {
                    running = subtype == "task_started";
                    turn_requests = 0;
                }
            }
            ("token_usage_record", _) => {
                // Each record is one response; its ID survives a fork's copy.
                if let Some(response) = payload["response_id"].as_str()
                    && let Some(reported) =
                        usage(response.to_string(), &payload["usage"], timestamp, &model)
                {
                    // A turn's first response can be recorded before the turn's model.
                    if let Some(id) = payload["turn_id"].as_str()
                        && turn_models.get(id) != Some(&model.as_str())
                    {
                        unknown_model.push((session.usage.len(), id));
                    }
                    session.usage.push(reported);
                }
            }
            ("event_msg", "token_count")
                if turn.as_deref().is_none_or(|id| !per_response.contains(id)) =>
            {
                // A changed running total within a turn marks a new request. The turn and total
                // identify it; a fork's copy keeps both but not the timestamp.
                let info = &payload["info"];
                let total = &info["total_token_usage"];
                if total.is_object() && total != totals {
                    let id = format!(
                        "{}:{}:{}",
                        turn.as_deref().unwrap_or(&thread),
                        total["input_tokens"],
                        total["output_tokens"]
                    );
                    if running
                        && let Some(reported) =
                            usage(id, &info["last_token_usage"], timestamp, &model)
                    {
                        session.usage.push(reported);
                    }
                    totals = total;
                }
            }
            ("event_msg", "thread_goal_updated") => {
                // The event repeats as a goal progresses; its creation time identifies the goal. A
                // goal the agent sets is reported from within its turn and is not the user's.
                let goal = &payload["goal"];
                let objective = goal["objective"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let new = goal["createdAt"]
                    .as_i64()
                    .is_some_and(|created| goals.insert(created));
                if new && payload.get("turnId").is_none() {
                    // An older client also records a typed /goal as a message: that is one goal.
                    if let Some(typed) = typed_goals.iter().position(|goal| *goal == objective) {
                        typed_goals.swap_remove(typed);
                    } else {
                        signal(Signal::Request { steer: false }, None);
                        keep(Role::User, format!("/goal {objective}"));
                    }
                }
            }
            ("response_item", "agent_message") => {
                signal(Signal::Activity, event_run);
                if session.kind != Kind::Review {
                    keep(Role::Agent, text(&payload["content"]).trim().to_string());
                }
                turn_requests += 1;
            }
            ("response_item", _) if TOOL_CALLS.contains(&subtype) => {
                signal(Signal::Activity, event_run);
                let id = payload["call_id"]
                    .as_str()
                    .or_else(|| payload["id"].as_str());
                if id.is_none_or(|id| tools.insert(id)) {
                    session.tool_calls += 1;
                }
                let input = ["arguments", "input", "action"]
                    .iter()
                    .find_map(|name| payload.get(*name))
                    .map(|input| {
                        input
                            .as_str()
                            .map_or_else(|| input.to_string(), str::to_string)
                    });
                let name = payload["name"].as_str().unwrap_or(subtype);
                keep(
                    Role::Tool,
                    format!("{name} {}", input.unwrap_or_default())
                        .trim()
                        .to_string(),
                );
            }
            ("response_item", _) if WORK.contains(&subtype) => signal(Signal::Activity, event_run),
            ("response_item", "message") if role == "assistant" => {
                signal(Signal::Activity, event_run);
                if record["metadata"]["compaction_output"].is_null() {
                    keep(
                        Role::Assistant,
                        text(&payload["content"]).trim().to_string(),
                    );
                }
            }
            ("response_item", "message") if role == "user" => {
                let raw = text(&payload["content"]);
                let raw = raw.trim();
                let id = payload["id"].as_str();
                if let Some(shell) = SHELL_COMMAND.captures(raw) {
                    keep(Role::User, format!("! {}", &shell[1]));
                } else if raw.starts_with(INTERNAL_CONTEXT) {
                    // A goal's injected objective is the request of the turn it starts.
                    turn_requests += u32::from(raw.starts_with(GOAL_CONTEXT));
                } else if !record["metadata"]["user_input_order"].is_null()
                    && id.is_none_or(|id| !seen.contains(id))
                {
                    let cleaned = request(raw);
                    if !cleaned.is_empty() {
                        // An answer to the model's question arrives in the turn but is no steer.
                        let steer =
                            running && turn_requests > 0 && !raw.starts_with(QUESTION_REPLY);
                        signal(Signal::Request { steer }, event_run);
                        turn_requests += 1;
                        if let Some(objective) = raw.strip_prefix("/goal ") {
                            typed_goals.push(objective.trim().to_string());
                        }
                    }
                    seen.extend(id);
                    keep(Role::User, cleaned);
                }
            }
            _ => {}
        }
    }
    for (index, turn) in unknown_model {
        match turn_models.get(turn) {
            Some(name) => session.usage[index].model = name.to_string(),
            // A finished turn with no context of its own, such as a compaction, ran on the model
            // then current.
            None if finished.contains(turn) => {}
            // An unfinished one may yet record another model: leave it unpriced, not stale.
            None => session.usage[index].model.clear(),
        }
    }
    (session.started, session.ended) = span.bounds();
    session.turns = timing::turns(&events);
    let version = header
        .and_then(|record| time(record["timestamp"].as_str()?))
        .map(|created| Version {
            thread,
            created,
            parent_is_thread,
        });
    (session, version)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const THREAD: &str = "00000037-0205-7016-8024-000000000001";

    #[test]
    fn flags_only_the_records_it_neither_reads_nor_drops() {
        let record = |kind: &str, detail: &str| json!({"type": kind, "payload": {"type": detail}});
        let records = [
            json!({"type": "session_meta", "payload": {}}),
            record("event_msg", "token_count"),
            record("response_item", "function_call"),
            record("response_item", "reasoning"),
            record("event_msg", "item_completed"),
            json!({"type": "token_usage_record", "payload": {}}),
            record("event_msg", "new_event"),
            record("response_item", "new_item"),
            json!({"type": "new_record", "payload": {"type": "x"}}),
        ];
        let unknown: Vec<String> = records.iter().filter_map(unknown).collect();
        assert_eq!(
            unknown,
            [
                "event_msg/new_event",
                "response_item/new_item",
                "new_record"
            ]
        );
    }

    fn path() -> std::path::PathBuf {
        format!("/home/.codex/sessions/2026/01/01/rollout-2026-01-01T00-00-00-{THREAD}.jsonl")
            .into()
    }

    fn user(id: &str, second: u32, text: &str, marked: bool) -> Value {
        json!({"type": "response_item", "timestamp": format!("2026-01-01T00:00:{second:02}Z"),
            "metadata": {"user_input_order": if marked { json!(1) } else { Value::Null }},
            "payload": {"type": "message", "role": "user", "id": id,
                "content": [{"type": "input_text", "text": text}]}})
    }

    fn event(subtype: &str, second: u32, extra: Value) -> Value {
        let mut payload = json!({"type": subtype, "turn_id": "t1"});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        json!({"type": "event_msg", "timestamp": format!("2026-01-01T00:00:{second:02}Z"),
            "payload": payload})
    }

    fn tokens(second: u32, input: u64, output: u64) -> Value {
        event(
            "token_count",
            second,
            json!({"info": {
                "last_token_usage": {"input_tokens": input, "cached_input_tokens": 4,
                    "output_tokens": output},
                "total_token_usage": {"input_tokens": input, "output_tokens": output}}}),
        )
    }

    #[test]
    fn reads_the_ids_in_a_rollout_name() {
        assert_eq!(ids(&path()), (THREAD.to_string(), THREAD.to_string()));
        let edit = format!(
            "rollout-2026-01-01T00-00-00-{THREAD}_{}.jsonl.zst",
            "b".repeat(36)
        );
        assert_eq!(ids(Path::new(&edit)), (THREAD.to_string(), "b".repeat(36)));
        // A name that is not a rollout's, with the cut inside a character, does not panic.
        let name = format!("{}a", "é".repeat(20));
        assert_eq!(ids(Path::new(&format!("{name}.jsonl"))).0, name);
    }

    #[test]
    fn renders_a_turn_with_its_usage_and_working_time() {
        let records = [
            json!({"type": "session_meta", "timestamp": "2026-01-01T00:00:00Z", "payload": {
                "thread_source": "user", "cwd": "/work/app",
                "git": {"repository_url": "https://github.com/o/r", "branch": "main"}}}),
            json!({"type": "turn_context", "payload": {"turn_id": "t1", "model": "gpt-6-luna"}}),
            event("task_started", 1, json!({})),
            user(
                "u0",
                1,
                "<environment_context>ide</environment_context>",
                false,
            ),
            user(
                "u1",
                2,
                "## Context\nfile\n## My request:\nFix it <image name=a>x</image>",
                true,
            ),
            user("u1", 2, "a repeated copy", true),
            json!({"type": "response_item", "timestamp": "2026-01-01T00:00:03Z", "payload": {
                "type": "function_call", "call_id": "c1", "name": "shell",
                "arguments": "{\"cmd\":\"ls\"}"}}),
            json!({"type": "response_item", "timestamp": "2026-01-01T00:00:04Z", "payload": {
                "type": "function_call_output", "call_id": "c1", "output": "files"}}),
            tokens(5, 100, 7),
            tokens(5, 100, 7),
            json!({"type": "response_item", "timestamp": "2026-01-01T00:00:06Z", "payload": {
                "type": "message", "role": "assistant",
                "content": [{"type": "output_text", "text": "Fixed."}]}}),
            event("task_complete", 8, json!({})),
        ];
        let (session, _) = parse(&path(), &records);
        let rendered: Vec<String> = session
            .items
            .iter()
            .map(|i| format!("{} {}", i.role.marker(), i.text))
            .collect();
        assert_eq!(
            rendered,
            [
                "[user] Fix it [image]",
                r#"[tool] shell {"cmd":"ls"}"#,
                "[assistant] Fixed."
            ]
        );
        assert_eq!(session.kind, Kind::Primary);
        assert_eq!(session.cwd.as_deref(), Some("/work/app"));
        assert_eq!(session.usage.len(), 1);
        let usage = &session.usage[0];
        assert_eq!((usage.input, usage.cached, usage.output), (96, 4, 7));
        assert_eq!(usage.model, "gpt-6-luna");
        assert_eq!(session.turns.len(), 1);
        assert_eq!(session.turns[0].working, Some(chrono::Duration::seconds(6)));
    }

    #[test]
    fn per_response_usage_replaces_the_running_total() {
        let response = |second: u32, id: &str, input: u64, output: u64| {
            json!({"type": "token_usage_record",
                "timestamp": format!("2026-01-01T00:00:{second:02}Z"),
                "payload": {"turn_id": "t1", "response_id": id, "usage": {
                    "input_tokens": input, "cached_input_tokens": 10, "output_tokens": output}}})
        };
        // The running total lags: it reports only the first response.
        let records = [
            json!({"type": "turn_context", "payload": {"turn_id": "t1", "model": "gpt-6-luna"}}),
            event("task_started", 1, json!({})),
            user("u1", 2, "Fix it", true),
            response(3, "resp_a", 100, 7),
            tokens(3, 100, 7),
            response(4, "resp_b", 200, 9),
        ];
        let (session, _) = parse(&path(), &records);
        let usage: Vec<_> = session
            .usage
            .iter()
            .map(|u| (u.id.as_str(), u.input, u.cached, u.output, u.model.as_str()))
            .collect();
        assert_eq!(
            usage,
            [
                ("resp_a", 90, 10, 7, "gpt-6-luna"),
                ("resp_b", 190, 10, 9, "gpt-6-luna")
            ]
        );
    }

    #[test]
    fn a_response_recorded_before_its_turn_context_takes_the_turns_model() {
        let records = [
            json!({"type": "turn_context", "payload": {"turn_id": "t0", "model": "gpt-6-luna"}}),
            event("task_started", 1, json!({})),
            json!({"type": "token_usage_record", "timestamp": "2026-01-01T00:00:02Z",
                "payload": {"turn_id": "t1", "response_id": "resp_a", "usage": {}}}),
            json!({"type": "turn_context", "payload": {"turn_id": "t1", "model": "gpt-6.1-sol"}}),
        ];
        let (session, _) = parse(&path(), &records);
        assert_eq!(session.usage[0].model, "gpt-6.1-sol");
    }

    #[test]
    fn a_resumed_rollout_counts_its_older_turns_from_the_running_total() {
        let records = [
            json!({"type": "turn_context", "payload": {"turn_id": "t1", "model": "gpt-6-luna"}}),
            event("task_started", 1, json!({})),
            user("u1", 2, "Old turn", true),
            tokens(3, 100, 7),
            event("task_complete", 4, json!({})),
            json!({"type": "turn_context", "payload": {"turn_id": "t2", "model": "gpt-6-luna"}}),
            event("task_started", 5, json!({"turn_id": "t2"})),
            user("u2", 6, "New turn", true),
            json!({"type": "token_usage_record", "timestamp": "2026-01-01T00:00:07Z",
                "payload": {"turn_id": "t2", "response_id": "resp_b", "usage": {}}}),
            event(
                "token_count",
                7,
                json!({"turn_id": "t2", "info": {
                "last_token_usage": {"input_tokens": 1}, "total_token_usage": {"input_tokens": 101}}}),
            ),
        ];
        let (session, _) = parse(&path(), &records);
        let ids: Vec<_> = session.usage.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, ["t1:100:7", "resp_b"]);
    }

    #[test]
    fn a_turn_without_a_model_keeps_the_current_one_only_once_finished() {
        let response = |turn: &str, id: &str| {
            json!({"type": "token_usage_record", "timestamp": "2026-01-01T00:00:02Z",
                "payload": {"turn_id": turn, "response_id": id, "usage": {}}})
        };
        // A compaction turn finishes without a turn_context; the last turn has not finished.
        let records = [
            json!({"type": "turn_context", "payload": {"turn_id": "t0", "model": "gpt-6-luna"}}),
            response("compact", "resp_a"),
            event("task_complete", 3, json!({"turn_id": "compact"})),
            response("t1", "resp_b"),
        ];
        let (session, _) = parse(&path(), &records);
        let models: Vec<_> = session.usage.iter().map(|u| u.model.as_str()).collect();
        assert_eq!(models, ["gpt-6-luna", ""]);
    }

    #[test]
    fn a_running_total_is_identified_without_its_timestamp() {
        let records = [
            event("task_started", 1, json!({})),
            user("u1", 2, "Fix it", true),
            tokens(3, 100, 7),
        ];
        let (session, _) = parse(&path(), &records);
        assert_eq!(session.usage[0].id, "t1:100:7");
    }

    #[test]
    fn manual_compaction_does_not_extend_the_previous_turn() {
        let records = [
            event("task_started", 0, json!({})),
            user("u1", 1, "First request", true),
            event("task_complete", 11, json!({})),
            event("task_started", 50, json!({"turn_id": "compact"})),
            event("task_complete", 52, json!({"turn_id": "compact"})),
        ];
        let (session, _) = parse(&path(), &records);
        assert_eq!(session.turns.len(), 1);
        assert_eq!(
            session.turns[0].working,
            Some(chrono::Duration::seconds(10))
        );
    }

    #[test]
    fn names_kinds_parents_goals_and_shell_commands() {
        let records = [
            json!({"type": "session_meta", "payload": {"thread_source": "guardian_review",
                "history_base": {"thread_id": "base", "end_ordinal_exclusive": 3}}}),
            json!({"type": "event_msg", "ordinal": 9, "timestamp": "2026-01-01T00:00:01Z",
                "payload": {"type": "thread_goal_updated",
                    "goal": {"createdAt": 1, "objective": " Ship it "}}}),
            json!({"type": "event_msg", "timestamp": "2026-01-01T00:00:02Z",
                "payload": {"type": "thread_goal_updated",
                    "goal": {"createdAt": 1, "objective": "Ship it"}}}),
            user(
                "u1",
                3,
                "<user_shell_command><command>ls -l</command></user_shell_command>",
                true,
            ),
            json!({"type": "response_item", "payload": {"type": "agent_message",
                "content": [{"type": "text", "text": "reviewed text"}]}}),
        ];
        let (session, _) = parse(&path(), &records);
        assert_eq!(session.kind, Kind::Review);
        assert_eq!(
            session.parent.map(|parent| parent.id).as_deref(),
            Some("base")
        );
        assert_eq!(
            session.history_base,
            Some(HistoryBase {
                rollout: "base".into(),
                end: 3
            })
        );
        let texts: Vec<&str> = session.items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, ["/goal Ship it", "! ls -l"]);
        assert_eq!(session.items[0].position, 9);
        assert_eq!(session.turns.len(), 1);
    }
}
