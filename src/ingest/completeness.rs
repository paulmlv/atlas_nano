//! Evidence that a model replied but its token counts could not be read.

use std::collections::{HashMap, HashSet};

use crate::model::{Client, Session, Usage};
use time::UtcDateTime;

#[derive(Debug, Hash, Eq, PartialEq)]
enum Key {
    Response(String),
    Turn(String),
}

struct Gap {
    key: Option<Key>,
    owner: Option<String>,
    line: usize,
}

/// A legacy turn may also appear in a newer copy with per-response measurements.
struct LegacyTurn {
    responses: Vec<(String, usize)>,
    totals: Vec<String>,
}

#[derive(Default)]
pub(super) struct Evidence {
    gaps: Vec<Gap>,
    pub measured_turns: HashSet<String>,
    legacy: Vec<LegacyTurn>,
}

impl Evidence {
    pub fn legacy_turn(&mut self, replies: Vec<(&str, usize)>, totals: Vec<String>) {
        self.legacy.push(LegacyTurn {
            responses: replies
                .into_iter()
                .map(|(id, line)| (id.to_owned(), line))
                .collect(),
            totals,
        });
    }

    /// Use a shorter copy only for responses that appear in the chosen transcript.
    pub fn recover_from_copy(&self, session: &mut Session, other: Vec<Usage>) {
        if (self.gaps.is_empty() && self.legacy.is_empty()) || other.is_empty() {
            return;
        }
        let mut wanted: HashSet<&str> = self
            .gaps
            .iter()
            .filter_map(|gap| match &gap.key {
                Some(Key::Response(id)) => Some(id.as_str()),
                _ => None,
            })
            .chain(
                self.legacy
                    .iter()
                    .flat_map(|turn| turn.responses.iter().map(|(id, _)| id.as_str())),
            )
            .collect();
        for usage in &session.usage {
            wanted.remove(usage.id.as_str());
        }
        session
            .usage
            .extend(other.into_iter().filter(|u| wanted.remove(u.id.as_str())));
    }

    pub fn missing_response(&mut self, id: Option<&str>, owner: Option<&str>, line: usize) {
        self.gaps.push(Gap {
            key: id.map(|id| Key::Response(id.to_owned())),
            owner: owner.map(str::to_owned),
            line,
        });
    }

    pub fn missing_turn(&mut self, id: Option<&str>, line: usize) {
        self.gaps.push(Gap {
            key: id.map(|id| Key::Turn(id.to_owned())),
            owner: None,
            line,
        });
    }
}

fn ownership_rank<'a>(
    session: &'a Session,
    owner: Option<&str>,
) -> (bool, bool, Option<UtcDateTime>, bool, &'a str) {
    (
        owner != Some(session.id.as_str()),
        session.started.is_none(),
        session.started,
        session.parent.is_some(),
        &session.id,
    )
}

type Owners = HashMap<Client, HashMap<String, (usize, usize)>>;

/// Keep each model request once: in the session recorded with it, when that session holds a copy,
/// or else in the earliest-started session that does.
///
/// A fork's copied history can start at its parent's time; on a tie the session with no parent
/// keeps the request. A request repeated within one session also counts once.
pub(super) fn count_requests_once(
    sessions: &mut [Session],
    evidence: &mut [Evidence],
) -> Vec<Option<usize>> {
    let mut owners = Owners::new();
    for (index, session) in sessions.iter().enumerate() {
        let owner = owners.entry(session.client).or_default();
        for (request, usage) in session.usage.iter().enumerate() {
            let held = owner.entry(usage.id.clone()).or_insert((index, request));
            let other = &sessions[held.0];
            if held.0 != index
                && ownership_rank(session, usage.owner.as_deref())
                    < ownership_rank(other, other.usage[held.1].owner.as_deref())
            {
                *held = (index, request);
            }
        }
    }
    prefer_response_records(evidence, &mut owners);
    let lines = resolve_gaps(sessions, evidence, &mut owners);
    for (index, session) in sessions.iter_mut().enumerate() {
        let owner = &owners[&session.client];
        let mut seen = HashSet::new();
        session.usage.retain(|usage| {
            owner.get(&usage.id).is_some_and(|held| held.0 == index)
                && seen.insert(usage.id.clone())
        });
    }
    lines
}

/// A response ID ties a recovered measurement to a legacy fallback. Turn names alone can repeat
/// in unrelated rollouts.
fn prefer_response_records(evidence: &mut [Evidence], requests: &mut Owners) {
    let Some(readings) = requests.get_mut(&Client::Codex) else {
        return;
    };
    for source in evidence {
        for legacy in std::mem::take(&mut source.legacy) {
            if legacy
                .responses
                .iter()
                .any(|(id, _)| readings.contains_key(id))
            {
                for id in legacy.totals {
                    readings.remove(&id);
                }
                for (id, line) in legacy.responses {
                    source.missing_response(Some(&id), None, line);
                }
            }
        }
    }
}

/// Return the first unresolved usage gap in each session. Streaming blocks, resumed sessions,
/// and forks can supply a measurement, or repeat the same gap, in another record.
fn resolve_gaps(
    sessions: &mut [Session],
    evidence: &[Evidence],
    requests: &mut Owners,
) -> Vec<Option<usize>> {
    let mut lines = vec![None; sessions.len()];
    if evidence.iter().all(|source| source.gaps.is_empty()) {
        return lines;
    }
    let turns: HashSet<_> = evidence.iter().flat_map(|e| &e.measured_turns).collect();
    let mut owners: HashMap<(Client, &Key), (usize, &Gap)> = HashMap::new();
    let mut flag = |n: usize, line: usize| {
        lines[n] = Some(lines[n].map_or(line, |earlier: usize| earlier.min(line)));
    };
    for (n, source) in evidence.iter().enumerate() {
        let session = &sessions[n];
        for gap in &source.gaps {
            let Some(key) = &gap.key else {
                flag(n, gap.line);
                continue;
            };
            let held = owners.entry((session.client, key)).or_insert((n, gap));
            if ownership_rank(session, gap.owner.as_deref())
                < ownership_rank(&sessions[held.0], held.1.owner.as_deref())
            {
                *held = (n, gap);
            }
        }
    }
    let mut recovered = Vec::new();
    for ((client, key), (n, gap)) in owners {
        match key {
            Key::Turn(id) if turns.contains(id) => {}
            Key::Response(id) if let Some(&(source, index)) = requests[&client].get(id) => {
                // A fork can have the only readable copy of the original session's request.
                // Restore it to the better-ranked holder before request deduplication.
                let usage = &sessions[source].usage[index];
                let recorded = gap.owner.as_deref().or(usage.owner.as_deref());
                if ownership_rank(&sessions[n], recorded)
                    < ownership_rank(&sessions[source], recorded)
                {
                    let mut copy = usage.clone();
                    copy.owner = recorded.map(str::to_owned);
                    recovered.push((n, gap.line, copy));
                }
            }
            _ => flag(n, gap.line),
        }
    }
    recovered.sort_unstable_by_key(|(n, line, _)| (*n, *line));
    for (n, _, usage) in recovered {
        let session = &mut sessions[n];
        requests
            .get_mut(&session.client)
            .expect("each client has owners")
            .insert(usage.id.clone(), (n, session.usage.len()));
        session.usage.push(usage);
    }
    lines
}
