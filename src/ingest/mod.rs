//! Find and read the local Claude Code and Codex sessions.

mod claude;
mod codex;
mod timing;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde_json::Value;

use crate::model::{Client, Item, Kind, Role, Session, SessionRef, Usage};

/// The folders each client keeps its sessions in, relative to the user's home.
pub const FOLDERS: [(Client, &str); 3] = [
    (Client::Claude, ".claude/projects"),
    (Client::Codex, ".codex/sessions"),
    (Client::Codex, ".codex/archived_sessions"),
];

/// The earliest and latest record times seen.
#[derive(Default)]
struct Span(Option<(DateTime<Utc>, DateTime<Utc>)>);

impl Span {
    fn add(&mut self, at: Option<DateTime<Utc>>) {
        if let Some(at) = at {
            self.0 = Some(
                self.0
                    .map_or((at, at), |(first, last)| (first.min(at), last.max(at))),
            );
        }
    }

    fn bounds(&self) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
        (self.0.map(|span| span.0), self.0.map(|span| span.1))
    }
}

/// Parse a record's ISO 8601 time into UTC.
fn time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// A recorded string, or `None` when it is missing or empty.
fn known(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Return a transcript's file name without `.jsonl` and an optional `.zst`.
fn stem(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name.trim_end_matches(".zst")
        .trim_end_matches(".jsonl")
        .to_string()
}

/// Decode a JSON Lines file, which may be zstd-compressed. A line that is not JSON becomes `null`,
/// so every record keeps its line's position, which fork cutoffs count by.
fn read_jsonl(path: &Path) -> io::Result<Vec<Value>> {
    let mut data = fs::read(path)?;
    if path.extension().is_some_and(|extension| extension == "zst") {
        data = zstd::decode_all(data.as_slice())?;
    }
    Ok(data
        .split(|byte| *byte == b'\n')
        .map(|line| serde_json::from_slice(line).unwrap_or(Value::Null))
        .collect())
}

/// List every transcript under `folder`, in path order.
///
/// A folder that does not exist holds nothing; one that cannot be read is reported in `unreadable`.
fn transcripts(folder: &Path, found: &mut Vec<PathBuf>, unreadable: &mut Vec<(PathBuf, String)>) {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            unreadable.push((folder.to_path_buf(), error.to_string()));
            return;
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    paths.sort();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if path.is_dir() {
            transcripts(&path, found, unreadable);
        } else if (name.ends_with(".jsonl") || name.ends_with(".jsonl.zst"))
            && !name.contains(".orphaned-")
        {
            found.push(path);
        }
    }
}

/// What a load found.
pub struct Loaded {
    pub sessions: Vec<Session>,
    /// Files and folders that could not be read, with the reason.
    pub unreadable: Vec<(PathBuf, String)>,
}

/// Read every session of the covered clients beneath `home`.
///
/// A session found in several files is read from the copy with the most valid records. A fork's
/// text starts with the history it inherited, and each model request stays only in the session
/// recorded with it, or else the one that started first, among those holding a copy of it.
pub fn load(home: &Path, clients: &[Client]) -> Loaded {
    let mut files = Vec::new();
    let mut unreadable = Vec::new();
    for (client, folder) in FOLDERS {
        if clients.contains(&client) {
            let mut found = Vec::new();
            transcripts(&home.join(folder), &mut found, &mut unreadable);
            files.extend(found.into_iter().map(|path| (client, path)));
        }
    }
    let parsed: Vec<_> = files
        .par_iter()
        .map(|(client, path)| {
            read_jsonl(path).map(|records| {
                let parsed = match client {
                    Client::Claude => (claude::parse(path, &records), None),
                    Client::Codex => codex::parse(path, &records),
                };
                // Malformed lines keep their place for fork cutoffs but do not make a copy longer.
                let valid = records.iter().filter(|record| !record.is_null()).count();
                (valid, parsed)
            })
        })
        .collect();
    let mut longest = HashMap::new();
    let mut order = Vec::new();
    for ((_, path), result) in files.iter().zip(parsed) {
        match result {
            Err(error) => unreadable.push((path.clone(), error.to_string())),
            Ok((length, parsed)) => {
                let key = parsed.0.key();
                match longest.get(&key) {
                    Some((best, _)) if *best >= length => {}
                    Some(_) => drop(longest.insert(key, (length, parsed))),
                    None => {
                        order.push(key.clone());
                        longest.insert(key, (length, parsed));
                    }
                }
            }
        }
    }
    let (mut sessions, versions): (Vec<_>, Vec<_>) = order
        .iter()
        .filter_map(|key| Some(longest.remove(key)?.1))
        .unzip();
    codex::link_parents(&mut sessions, &versions);
    // Flag before requests are kept once and forks inherit history: a Claude fork's copied usage
    // still counts as its own, and a Codex fork's inherited replies do not.
    for session in &mut sessions {
        session.usage_missing = session.usage.is_empty()
            && session
                .items
                .iter()
                .any(|item| matches!(item.role, Role::Assistant | Role::Tool));
    }
    count_requests_once(&mut sessions);
    inherit_history(&mut sessions);
    link_scratchpads(&mut sessions);
    Loaded {
        sessions,
        unreadable,
    }
}

/// The Claude Code session whose scratchpad holds `cwd`, if it is in one.
///
/// Claude Code gives each session `/tmp/claude-<uid>/<project>/<session id>/scratchpad`, where it
/// makes worktrees and folders that other conversations, such as Codex reviews, then start in.
fn scratchpad_owner(cwd: &str) -> Option<&str> {
    let rest = cwd.strip_prefix("/private").unwrap_or(cwd);
    let mut parts = rest.strip_prefix("/tmp/claude-")?.split('/').skip(2);
    let session = parts.next()?;
    (parts.next() == Some("scratchpad")).then_some(session)
}

/// Make each chat started in a loaded Claude Code session's scratchpad a subagent of that session,
/// which the scratchpad's path names.
fn link_scratchpads(sessions: &mut [Session]) {
    let claude: HashMap<&str, usize> = (0..sessions.len())
        .filter(|&n| sessions[n].client == Client::Claude && sessions[n].kind == Kind::Primary)
        .map(|n| (sessions[n].id.as_str(), n))
        .collect();
    let links: Vec<(usize, String)> = (0..sessions.len())
        .filter(|&n| sessions[n].kind == Kind::Primary && sessions[n].parent.is_none())
        .filter_map(|n| {
            let owner = *claude.get(scratchpad_owner(sessions[n].cwd.as_deref()?)?)?;
            (owner != n).then(|| (n, sessions[owner].id.clone()))
        })
        .collect();
    for (n, owner) in links {
        let chat = &mut sessions[n];
        chat.parent = Some(SessionRef {
            client: Client::Claude,
            id: owner,
        });
        chat.kind = Kind::Subagent;
    }
}

/// Keep each model request once: in the session recorded with it, when that session holds a copy,
/// or else in the earliest-started session that does.
///
/// A fork's copied history can start at its parent's time; on a tie the session with no parent
/// keeps the request. A request repeated within one session also counts once.
fn count_requests_once(sessions: &mut [Session]) {
    // Every copy of a request records the same owner, so one usage ranks all its holders.
    let rank = |session: &Session, usage: &Usage| {
        (
            usage.owner.as_deref() != Some(session.id.as_str()),
            session.started.is_none(),
            session.started,
            session.parent.is_some(),
            session.key(),
        )
    };
    let mut owner: HashMap<String, usize> = HashMap::new();
    for (index, session) in sessions.iter().enumerate() {
        for usage in &session.usage {
            let held = owner.entry(usage.id.clone()).or_insert(index);
            if rank(session, usage) < rank(&sessions[*held], usage) {
                *held = index;
            }
        }
    }
    for (index, session) in sessions.iter_mut().enumerate() {
        let mut seen = HashSet::new();
        session
            .usage
            .retain(|usage| owner[&usage.id] == index && seen.insert(usage.id.clone()));
    }
}

/// Prepend to each Codex fork the items of the rollout it references, before its cutoff.
fn inherit_history(sessions: &mut [Session]) {
    let index: HashMap<String, usize> = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| session.client == Client::Codex)
        .map(|(n, session)| (session.id.clone(), n))
        .collect();
    // Follow the bases up to the first one not loaded, stopping if one repeats, then add each
    // level's own items on top of what it inherits before its cutoff.
    fn complete(n: usize, sessions: &[Session], index: &HashMap<String, usize>) -> Vec<Item> {
        let mut chain = vec![n];
        let mut seen = HashSet::from([n]);
        let mut cutoffs = Vec::new();
        while let Some(base) = &sessions[*chain.last().expect("starts with n")].history_base {
            match index.get(&base.rollout) {
                Some(&parent) if seen.insert(parent) => {
                    chain.push(parent);
                    cutoffs.push(base.end);
                }
                _ => break,
            }
        }
        let top = chain.pop().expect("starts with n");
        let mut items = sessions[top].items.clone();
        for (level, end) in chain.into_iter().zip(cutoffs).rev() {
            items.retain(|item| item.position < end);
            items.extend(sessions[level].items.iter().cloned());
        }
        items
    }
    let forks: Vec<usize> = (0..sessions.len())
        .filter(|&n| sessions[n].history_base.is_some())
        .collect();
    let completed: Vec<Vec<Item>> = forks
        .iter()
        .map(|&n| complete(n, sessions, &index))
        .collect();
    for (n, items) in forks.into_iter().zip(completed) {
        sessions[n].items = items;
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde_json::json;

    use super::*;
    use crate::model::Kind;

    fn write(path: &Path, records: &[Value], compress: bool) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let lines: String = records.iter().map(|record| format!("{record}\n")).collect();
        let data = if compress {
            zstd::encode_all(lines.as_bytes(), 0).unwrap()
        } else {
            lines.into_bytes()
        };
        fs::File::create(path).unwrap().write_all(&data).unwrap();
    }

    fn reply(id: &str, second: u32) -> Value {
        json!({"type": "assistant", "uuid": format!("{id}-{second}"),
            "timestamp": format!("2026-01-01T00:00:{second:02}Z"),
            "message": {"id": id, "model": "claude-haiku-5-5", "content": [],
                "usage": {"input_tokens": 1, "output_tokens": 1}}})
    }

    #[test]
    fn finds_the_session_that_owns_a_scratchpad() {
        let owner = |cwd| scratchpad_owner(cwd);
        assert_eq!(
            owner("/private/tmp/claude-501/-Code-app/s1/scratchpad/wt"),
            Some("s1")
        );
        assert_eq!(owner("/tmp/claude-501/-Code-app/s1/scratchpad"), Some("s1"));
        assert_eq!(owner("/tmp/claude-501/-Code-app/s1/other"), None);
        assert_eq!(owner("/private/tmp/review"), None);
    }

    #[test]
    fn links_a_chat_in_a_scratchpad_to_the_session_owning_it() {
        use crate::account::tests::session;
        let in_folder = |id: &str, client: Client, cwd: &str| {
            let mut chat = session(id, Kind::Primary, None, 2);
            chat.client = client;
            chat.cwd = Some(cwd.to_string());
            chat
        };
        let scratchpad = "/private/tmp/claude-501/-Code-app/owner/scratchpad";
        let mut sessions = vec![
            in_folder("owner", Client::Claude, scratchpad),
            in_folder("review", Client::Codex, &format!("{scratchpad}/wt")),
            in_folder("spike", Client::Claude, scratchpad),
            // A worktree elsewhere names no session, and an unloaded owner links nothing.
            in_folder("other", Client::Codex, "/x/app/.claude/worktrees/review-a"),
            in_folder(
                "orphan",
                Client::Codex,
                "/tmp/claude-501/-Code-app/gone/scratchpad",
            ),
        ];
        link_scratchpads(&mut sessions);
        let link = |n: usize| {
            let session = &sessions[n];
            (
                session.kind,
                session.parent.as_ref().map(|parent| parent.id.as_str()),
                session.parent.as_ref().map(|parent| parent.client),
            )
        };
        let owner = (Kind::Subagent, Some("owner"), Some(Client::Claude));
        assert_eq!(link(0), (Kind::Primary, None, None));
        assert_eq!(link(1), owner);
        assert_eq!(link(2), owner);
        assert_eq!(link(3), (Kind::Primary, None, None));
        assert_eq!(link(4), (Kind::Primary, None, None));
    }

    fn home(name: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("atlas-nano-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        home
    }

    #[test]
    fn a_request_counts_once_and_a_fork_loses_ties_to_its_parent() {
        use crate::account::tests::session;
        let mut parent = session("parent", Kind::Primary, None, 1);
        let mut fork = session("a-fork", Kind::Primary, Some("parent"), 1);
        fork.usage = parent.usage.clone();
        // A replayed request within one session.
        parent.usage.push(parent.usage[0].clone());
        let mut sessions = vec![fork, parent];
        count_requests_once(&mut sessions);
        assert!(sessions[0].usage.is_empty());
        let ids: Vec<&str> = sessions[1]
            .usage
            .iter()
            .map(|usage| usage.id.as_str())
            .collect();
        let mut unique = ids.clone();
        unique.dedup();
        assert_eq!(ids, unique);
    }

    #[test]
    fn loads_both_clients_and_counts_a_copied_request_once() {
        let home = home("load");
        let projects = home.join(".claude/projects/p");
        write(
            &projects.join("first.jsonl"),
            &[reply("m1", 1), reply("m2", 2)],
            false,
        );
        // A fork copies its parent's records, then adds its own.
        write(
            &projects.join("fork.jsonl.zst"),
            &[reply("m1", 1), reply("m3", 9)],
            true,
        );
        write(
            &projects.join("first/subagents/agent-1.jsonl"),
            &[reply("m4", 3)],
            false,
        );
        write(
            &projects.join("gone.orphaned-1.jsonl"),
            &[reply("m5", 1)],
            false,
        );
        fs::write(projects.join("notes.txt"), "not a transcript").unwrap();
        let rollout = "rollout-2026-01-01T00-00-00-00000037-0205-7016-8024-000000000001.jsonl";
        let meta = json!({"type": "session_meta", "payload": {"thread_source": "subagent"}});
        write(
            &home.join(".codex/sessions/2026").join(rollout),
            std::slice::from_ref(&meta),
            false,
        );
        write(
            &home.join(".codex/archived_sessions").join(rollout),
            &[meta.clone(), meta],
            false,
        );

        let loaded = load(&home, &[Client::Claude, Client::Codex]);
        let usage = |id: &str| -> Vec<String> {
            let session = loaded
                .sessions
                .iter()
                .find(|session| session.id == id)
                .unwrap();
            session.usage.iter().map(|usage| usage.id.clone()).collect()
        };
        assert_eq!(loaded.sessions.len(), 4);
        assert_eq!(usage("first"), ["m1", "m2"]);
        assert_eq!(usage("fork"), ["m3"]);
        let agent = loaded
            .sessions
            .iter()
            .find(|session| session.id == "agent-1")
            .unwrap();
        assert_eq!(
            (
                agent.kind,
                agent.parent.as_ref().map(|parent| parent.id.as_str())
            ),
            (Kind::Subagent, Some("first"))
        );
        assert!(loaded.unreadable.is_empty());
        assert_eq!(load(&home, &[Client::Codex]).sessions.len(), 1);
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_claude_fork_enters_the_period_at_its_fork_point() {
        use crate::account::{in_period, roots};

        let home = home("claude-fork");
        let projects = home.join(".claude/projects/p");
        let mut original = reply("old", 1);
        original["sessionId"] = json!("original");
        write(
            &projects.join("original.jsonl"),
            std::slice::from_ref(&original),
            false,
        );

        let mut marked = original.clone();
        marked["uuid"] = json!("copied-answer");
        marked["sessionId"] = json!("fork-marked");
        marked["forkedFrom"] = json!({"sessionId": "original", "messageUuid": original["uuid"]});
        let fork_time = "2026-01-01T00:00:08Z";
        for (id, copied) in [("fork-copy", original), ("fork-marked", marked)] {
            let started = json!({
                "type": "custom-title", "sessionId": id, "timestamp": fork_time,
                "customTitle": "Fork"
            });
            write(
                &projects.join(format!("{id}.jsonl")),
                &[copied, started, reply(id, 9)],
                false,
            );
        }

        let sessions = load(&home, &[Client::Claude]).sessions;
        fs::remove_dir_all(&home).unwrap();
        let fork_time = time(fork_time).unwrap();
        let covered = in_period(&sessions, &roots(&sessions), Some(fork_time));
        let included: Vec<_> = sessions
            .iter()
            .zip(covered)
            .filter_map(|(session, included)| included.then_some(session.id.as_str()))
            .collect();
        assert_eq!(included, ["fork-copy", "fork-marked"]);
        for session in sessions {
            let own_request = if session.id == "original" {
                "old"
            } else {
                assert_eq!(session.started, Some(fork_time));
                session.id.as_str()
            };
            assert_eq!(session.usage.len(), 1);
            assert_eq!(session.usage[0].id, own_request);
        }
    }

    #[test]
    fn a_copy_with_more_valid_records_wins_over_a_malformed_one() {
        let home = home("malformed-copy");
        let projects = home.join(".claude/projects");
        write(
            &projects.join("p/chat.jsonl"),
            &[reply("m1", 1), reply("m2", 2)],
            false,
        );
        let malformed = format!("{}\nnot json\nnot json\nnot json\n", reply("m1", 1));
        fs::create_dir_all(projects.join("q")).unwrap();
        fs::write(projects.join("q/chat.jsonl"), malformed).unwrap();

        let sessions = load(&home, &[Client::Claude]).sessions;
        fs::remove_dir_all(&home).unwrap();
        let ids: Vec<_> = sessions[0].usage.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, ["m1", "m2"]);
    }

    #[test]
    fn a_claude_copy_belongs_to_the_session_recorded_with_it() {
        let home = home("claude-recorded-owner");
        let projects = home.join(".claude/projects/p");
        let recorded = |id: &str, second, session: &str| {
            let mut record = reply(id, second);
            record["sessionId"] = json!(session);
            record
        };
        // The fork starts at the same time as the original and sorts first by name.
        write(
            &projects.join("z-original.jsonl"),
            &[recorded("old", 1, "z-original")],
            false,
        );
        write(
            &projects.join("a-fork.jsonl"),
            &[
                recorded("old", 1, "z-original"),
                recorded("new", 1, "a-fork"),
            ],
            false,
        );
        // Another fork relabels its copy with its own ID and records where it came from.
        let mut relabelled = recorded("old", 1, "zz-fork");
        relabelled["forkedFrom"] = json!({"sessionId": "z-original"});
        write(
            &projects.join("zz-fork.jsonl"),
            &[relabelled, recorded("newer", 1, "zz-fork")],
            false,
        );
        // A subagent copied under the fork still belongs to the original.
        write(
            &projects.join("a-fork/subagents/agent-1.jsonl"),
            &[recorded("sub", 2, "z-original")],
            false,
        );

        let sessions = load(&home, &[Client::Claude]).sessions;
        fs::remove_dir_all(&home).unwrap();
        let find = |id: &str| sessions.iter().find(|session| session.id == id).unwrap();
        let ids =
            |id: &str| -> Vec<&str> { find(id).usage.iter().map(|u| u.id.as_str()).collect() };
        assert_eq!(ids("z-original"), ["old"]);
        assert_eq!(ids("a-fork"), ["new"]);
        assert_eq!(ids("zz-fork"), ["newer"]);
        let parent = find("agent-1").parent.as_ref().map(|p| p.id.as_str());
        assert_eq!(parent, Some("z-original"));
    }

    #[test]
    fn a_codex_fork_keeps_a_copied_request_with_its_parent() {
        let home = home("codex-fork-copies");
        let folder = home.join(".codex/sessions");
        let at = |second: u32| format!("2026-01-01T00:00:{second:02}Z");
        let started = |second| json!({"type": "event_msg", "timestamp": at(second), "payload": {"type": "task_started", "turn_id": "t1"}});
        let total = |second, output: u64| {
            json!({"type": "event_msg", "timestamp": at(second), "payload": {"type": "token_count",
                "turn_id": "t1", "info": {"last_token_usage": {"input_tokens": 10, "output_tokens": 1},
                "total_token_usage": {"input_tokens": 10, "output_tokens": output}}}})
        };
        let response = |second, id: &str| {
            json!({"type": "token_usage_record", "timestamp": at(second), "payload": {
                "turn_id": "t1", "response_id": id, "usage": {"input_tokens": 10, "output_tokens": 1}}})
        };
        let file = |id: &str| folder.join(format!("rollout-2026-01-01T00-00-00-{id}.jsonl"));
        let [old, new, old_fork, new_fork] = ["a", "b", "c", "d"].map(|id| id.repeat(36));
        // Each fork copies its parent's records with new timestamps, then adds its own.
        write(&file(&old), &[started(1), total(2, 1)], false);
        write(
            &file(&format!("{old}_{old_fork}")),
            &[started(30), total(30, 1), total(31, 2)],
            false,
        );
        write(&file(&new), &[started(1), response(2, "resp_old")], false);
        write(
            &file(&format!("{new}_{new_fork}")),
            &[
                started(30),
                response(30, "resp_old"),
                response(31, "resp_new"),
            ],
            false,
        );

        let sessions = load(&home, &[Client::Codex]).sessions;
        fs::remove_dir_all(&home).unwrap();
        let mut usage: Vec<_> = sessions
            .iter()
            .map(|session| {
                let ids: Vec<_> = session.usage.iter().map(|u| u.id.as_str()).collect();
                (session.parent.is_some(), ids.join(" "))
            })
            .collect();
        usage.sort();
        assert_eq!(
            usage,
            [
                (false, "resp_old".to_string()),
                (false, "t1:10:1".to_string()),
                (true, "resp_new".to_string()),
                (true, "t1:10:2".to_string()),
            ]
        );
    }

    #[test]
    fn a_codex_child_uses_the_parent_rollout_current_at_its_start() {
        use crate::account::{in_period, roots};

        let home = home("codex-parent-versions");
        let folder = home.join(".codex/sessions");
        let [thread, edit, future, before, after, review, exact] =
            ["a", "b", "c", "d", "e", "f", "g"].map(|id| id.repeat(36));
        let file = |day: u32, id: &str| {
            folder.join(format!("rollout-2026-01-{day:02}T00-00-00-{id}.jsonl"))
        };
        let meta = |day: u32, payload: Value| {
            json!({"type": "session_meta", "timestamp": format!("2026-01-{day:02}T00:00:00Z"),
                "payload": payload})
        };
        let copied = json!({"type": "turn_context", "timestamp": "2026-01-05T00:01:00Z"});
        write(&file(5, &thread), &[meta(5, json!({}))], false);
        write(
            &file(9, &format!("{thread}_{edit}")),
            &[meta(9, json!({}))],
            false,
        );
        write(
            &file(11, &format!("{thread}_{future}")),
            &[meta(11, json!({})), copied.clone()],
            false,
        );
        let spawn = json!({"thread_source": "subagent", "source": {
            "subagent": {"thread_spawn": {"parent_thread_id": thread}}
        }});
        let guardian = json!({"thread_source": "guardian_review", "parent_thread_id": thread});
        let mut explicit = guardian.clone();
        explicit["history_base"] = json!({"thread_id": thread, "end_ordinal_exclusive": 1});
        for (day, id, payload) in [
            (8, &before, spawn.clone()),
            (10, &after, spawn),
            (10, &review, guardian),
            (10, &exact, explicit),
        ] {
            write(&file(day, id), &[meta(day, payload), copied.clone()], false);
        }

        let sessions = load(&home, &[Client::Codex]).sessions;
        fs::remove_dir_all(&home).unwrap();
        let index = |id: &str| {
            sessions
                .iter()
                .position(|session| session.id == id)
                .unwrap()
        };
        let roots = roots(&sessions);
        let included = in_period(&sessions, &roots, time("2026-01-09T00:00:00Z"));
        for (child, parent, in_period) in [
            (&before, &thread, false),
            (&after, &edit, true),
            (&review, &edit, true),
            (&exact, &thread, false),
        ] {
            let child = index(child);
            assert_eq!(
                sessions[child].parent,
                Some(SessionRef {
                    client: Client::Codex,
                    id: parent.clone()
                })
            );
            assert_eq!(roots[child], index(parent));
            assert_eq!(included[child], in_period);
        }
    }

    #[test]
    fn history_is_inherited_at_any_depth_and_a_loop_ends() {
        use crate::account::tests::session;
        use crate::model::{HistoryBase, Role};
        let rollout = |id: &str, base: Option<&str>| {
            let mut fork = session(id, Kind::Primary, None, 1);
            fork.client = Client::Codex;
            fork.history_base = base.map(|base| HistoryBase {
                rollout: base.to_string(),
                end: 1,
            });
            // The item at position 5 is past every cutoff, so only the fork's own copy is kept.
            fork.items = [(0, id.to_string()), (5, format!("late-{id}"))]
                .map(|(position, text)| Item {
                    position,
                    role: Role::Assistant,
                    text,
                })
                .to_vec();
            fork
        };
        let mut sessions: Vec<Session> = (0..40)
            .map(|n| {
                let base = (n > 0).then(|| format!("s{}", n - 1));
                rollout(&format!("s{n}"), base.as_deref())
            })
            .collect();
        sessions.push(rollout("x", Some("y")));
        sessions.push(rollout("y", Some("x")));
        inherit_history(&mut sessions);
        let texts = |n: usize| -> Vec<&str> {
            sessions[n]
                .items
                .iter()
                .map(|item| item.text.as_str())
                .collect()
        };
        let mut deepest: Vec<String> = (0..40).map(|n| format!("s{n}")).collect();
        deepest.push("late-s39".to_string());
        assert_eq!(texts(39), deepest);
        assert_eq!(texts(40), ["y", "x", "late-x"]);
    }

    #[test]
    fn flags_the_sessions_with_replies_but_no_usage() {
        let home = home("usage-missing");
        let projects = home.join(".claude/projects/p");
        let mut answer = reply("m1", 1);
        answer["sessionId"] = json!("with_usage");
        answer["message"]["content"] = json!([{"type": "text", "text": "hi"}]);
        let mut no_usage = answer.clone();
        no_usage["message"].as_object_mut().unwrap().remove("usage");
        let mut renamed = answer.clone();
        renamed["message"]["usage"] = json!({"input": 1, "output": 1});
        write(&projects.join("with_usage.jsonl"), &[answer.clone()], false);
        write(&projects.join("no_usage.jsonl"), &[no_usage], false);
        write(&projects.join("renamed.jsonl"), &[renamed], false);
        write(
            &projects.join("asked.jsonl"),
            &[json!({"type": "user", "message": {"content": "hi"}})],
            false,
        );
        // A fork with no reply of its own: its copied request goes to the parent when each request
        // is kept once, but the copy still counts as its usage.
        write(&projects.join("fork.jsonl"), &[answer], false);

        let rollout = |thread: u8| {
            home.join(format!(
                ".codex/sessions/rollout-2026-01-01T00-00-00-00000037-0205-7016-8024-00000000000{thread}.jsonl"
            ))
        };
        let message = json!({"type": "response_item", "timestamp": "2026-01-01T00:00:01Z",
            "payload": {"type": "message", "role": "assistant",
                "content": [{"type": "output_text", "text": "hi"}]}});
        let record = |usage: Value| {
            json!({"type": "token_usage_record", "timestamp": "2026-01-01T00:00:01Z",
                "payload": {"response_id": "r1", "usage": usage}})
        };
        let counts = json!({"input_tokens": 1, "output_tokens": 1});
        write(&rollout(1), &[message.clone(), record(counts)], false);
        write(&rollout(2), std::slice::from_ref(&message), false);
        let renamed = json!({"input": 1, "output": 1});
        write(&rollout(3), &[message, record(renamed)], false);
        // A fork with no reply of its own inherits its base's replies but not their usage.
        let thread = "00000037-0205-7016-8024-000000000001";
        let forked = json!({"type": "session_meta", "payload": {
            "history_base": {"thread_id": thread, "end_ordinal_exclusive": 1}}});
        write(&rollout(4), &[forked], false);

        let loaded = load(&home, &[Client::Claude, Client::Codex]);
        fs::remove_dir_all(&home).unwrap();
        let mut flagged: Vec<String> = loaded
            .sessions
            .iter()
            .filter(|session| session.usage_missing)
            .map(Session::key)
            .collect();
        flagged.sort();
        assert_eq!(
            flagged,
            [
                "claude:no_usage",
                "claude:renamed",
                "codex:00000037-0205-7016-8024-000000000002",
                "codex:00000037-0205-7016-8024-000000000003"
            ]
        );
    }

    #[test]
    fn a_fork_inherits_the_referenced_history_before_its_cutoff() {
        let home = home("fork");
        let folder = home.join(".codex/sessions");
        let name = |id: &str| format!("rollout-2026-01-01T00-00-00-{id}.jsonl");
        let (base, fork) = ("a".repeat(36), "b".repeat(36));
        let said = |text: &str| {
            json!({"type": "response_item", "payload": {"type": "message", "role": "assistant",
                "content": [{"type": "output_text", "text": text}]}})
        };
        let meta = json!({"type": "session_meta", "payload": {}});
        write(
            &folder.join(name(&base)),
            &[meta, said("one"), said("two"), said("three")],
            false,
        );
        let forked = json!({"type": "session_meta", "payload": {
            "history_base": {"thread_id": base, "end_ordinal_exclusive": 3}}});
        write(&folder.join(name(&fork)), &[forked, said("four")], false);

        let loaded = load(&home, &[Client::Codex]);
        let session = loaded
            .sessions
            .iter()
            .find(|session| session.id == fork)
            .unwrap();
        let texts: Vec<&str> = session
            .items
            .iter()
            .map(|item| item.text.as_str())
            .collect();
        assert_eq!(texts, ["one", "two", "four"]);
        fs::remove_dir_all(&home).unwrap();
    }
}
