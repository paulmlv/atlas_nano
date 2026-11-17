//! Evidence that a model replied but its token counts could not be read.

use std::collections::{HashMap, HashSet};

use crate::model::{Client, Session};
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
    responses: Vec<String>,
    totals: Vec<usize>,
}

#[derive(Default)]
pub(super) struct Evidence {
    gaps: Vec<Gap>,
    pub measured_turns: HashSet<String>,
    legacy: Vec<LegacyTurn>,
}

impl Evidence {
    pub fn legacy_turn(&mut self, mut replies: Vec<&str>, totals: Vec<usize>) {
        if replies.is_empty() {
            return;
        }
        replies.sort_unstable();
        replies.dedup();
        self.legacy.push(LegacyTurn {
            responses: replies.into_iter().map(str::to_owned).collect(),
            totals,
        });
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
    evidence: &[Evidence],
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
    deduplicate_running_totals(sessions, evidence, &mut owners);
    let lines = resolve_gaps(sessions, evidence, &owners);
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

/// A copied response can have two IDs: a cumulative token count and an explicit response ID.
/// When only part of the turn has per-response readings, remove one matching count per response.
fn deduplicate_running_totals(sessions: &[Session], evidence: &[Evidence], requests: &mut Owners) {
    let Some(readings) = requests.get_mut(&Client::Codex) else {
        return;
    };
    let mut replaced = HashSet::new();
    for (session, source) in sessions.iter().zip(evidence) {
        for legacy in &source.legacy {
            let measured: Vec<_> = legacy
                .responses
                .iter()
                .filter_map(|id| readings.get(id))
                .map(|&(s, r)| &sessions[s].usage[r])
                .collect();
            if measured.is_empty() {
                continue;
            }
            let mut remaining: Vec<_> = legacy.totals.iter().map(|&n| &session.usage[n]).collect();
            if measured.len() == legacy.responses.len() && measured.len() >= remaining.len() {
                replaced.extend(remaining.iter().map(|u| u.id.as_str()));
            } else {
                for response in measured {
                    if let Some(index) = remaining.iter().position(|u| {
                        u.prompt() == response.prompt() && u.output == response.output
                    }) {
                        replaced.insert(remaining.remove(index).id.as_str());
                    }
                }
            }
        }
    }
    for id in replaced {
        readings.remove(id);
    }
}

/// Return the first unresolved usage gap in each session. Streaming blocks, resumed sessions,
/// and forks can supply a measurement, or repeat the same gap, in another record.
fn resolve_gaps(
    sessions: &[Session],
    evidence: &[Evidence],
    requests: &Owners,
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
            let measured = match key {
                Key::Turn(id) => turns.contains(id),
                Key::Response(id) => requests[&session.client].contains_key(id),
            };
            if measured {
                continue;
            }
            let held = owners.entry((session.client, key)).or_insert((n, gap));
            if ownership_rank(session, gap.owner.as_deref())
                < ownership_rank(&sessions[held.0], held.1.owner.as_deref())
            {
                *held = (n, gap);
            }
        }
    }
    for (n, gap) in owners.into_values() {
        flag(n, gap.line);
    }
    lines
}
