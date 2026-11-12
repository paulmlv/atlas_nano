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

use crate::model::{Client, Item, Kind, Session};

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
/// A session found in several files is read from the copy with the most records. A fork's text
/// starts with the history it inherited, and each model request stays only in the session that
/// started first among those holding a copy of it.
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
                let session = match client {
                    Client::Claude => claude::parse(path, &records),
                    Client::Codex => codex::parse(path, &records),
                };
                (records.len(), session)
            })
        })
        .collect();
    let mut longest: HashMap<String, (usize, Session)> = HashMap::new();
    let mut order = Vec::new();
    for ((_, path), result) in files.iter().zip(parsed) {
        match result {
            Err(error) => unreadable.push((path.clone(), error.to_string())),
            Ok((length, session)) => {
                let key = session.key();
                match longest.get(&key) {
                    Some((best, _)) if *best >= length => {}
                    Some(_) => drop(longest.insert(key, (length, session))),
                    None => {
                        order.push(key.clone());
                        longest.insert(key, (length, session));
                    }
                }
            }
        }
    }
    let mut sessions: Vec<Session> = order
        .iter()
        .filter_map(|key| Some(longest.remove(key)?.1))
        .collect();
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
            let owner = *claude.get(scratchpad_owner(&sessions[n].cwd)?)?;
            (owner != n).then(|| (n, sessions[owner].id.clone()))
        })
        .collect();
    for (n, owner) in links {
        let chat = &mut sessions[n];
        chat.parent = Some(owner);
        chat.parent_client = Client::Claude;
        chat.kind = Kind::Subagent;
    }
}

/// Keep each model request once, in the earliest-started session that holds a copy of it.
///
/// A fork's copied history can start at its parent's time; on a tie the session with no parent
/// keeps the request. A request repeated within one session also counts once.
fn count_requests_once(sessions: &mut [Session]) {
    let rank = |session: &Session| {
        (
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
            if rank(session) < rank(&sessions[*held]) {
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
    fn complete(
        n: usize,
        sessions: &[Session],
        index: &HashMap<String, usize>,
        depth: u8,
    ) -> Vec<Item> {
        let session = &sessions[n];
        let mut items = match &session.history_base {
            Some(base) if depth < 32 => index.get(&base.rollout).map_or_else(Vec::new, |&parent| {
                let mut inherited = complete(parent, sessions, index, depth + 1);
                inherited.retain(|item| item.position < base.end);
                inherited
            }),
            _ => Vec::new(),
        };
        items.extend(session.items.iter().cloned());
        items
    }
    let forks: Vec<usize> = (0..sessions.len())
        .filter(|&n| sessions[n].history_base.is_some())
        .collect();
    let completed: Vec<Vec<Item>> = forks
        .iter()
        .map(|&n| complete(n, sessions, &index, 0))
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
            (chat.client, chat.parent_client) = (client, client);
            chat.cwd = cwd.to_string();
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
                session.parent.as_deref(),
                session.parent_client,
            )
        };
        assert_eq!(link(0), (Kind::Primary, None, Client::Claude));
        assert_eq!(link(1), (Kind::Subagent, Some("owner"), Client::Claude));
        assert_eq!(link(2), (Kind::Subagent, Some("owner"), Client::Claude));
        assert_eq!(link(3), (Kind::Primary, None, Client::Codex));
        assert_eq!(link(4), (Kind::Primary, None, Client::Codex));
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
            (agent.kind, agent.parent.as_deref()),
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
