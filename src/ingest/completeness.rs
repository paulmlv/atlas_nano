//! Evidence that a model replied but its token counts could not be read.

use std::collections::{HashMap, HashSet};

use crate::model::{Client, Session};
use time::UtcDateTime;

#[derive(Debug, Hash, Eq, PartialEq)]
enum Key {
    Response(String),
    Turn(String),
    RunningTotal(String),
}

struct Gap {
    key: Option<Key>,
    owner: Option<String>,
    line: usize,
}

/// A legacy turn may also appear in a newer copy with per-response measurements.
struct LegacyTurn {
    responses: Vec<(String, usize)>,
    totals: Vec<usize>,
}

#[derive(Default)]
pub(super) struct Evidence {
    gaps: Vec<Gap>,
    pub usage_thread: Option<String>,
    pub measured_turns: HashSet<String>,
    legacy: Vec<LegacyTurn>,
}

impl Evidence {
    pub fn legacy_turn(&mut self, mut replies: Vec<(&str, usize)>, totals: Vec<usize>) {
        replies.sort_unstable();
        replies.dedup_by_key(|r| r.0);
        self.legacy.push(LegacyTurn {
            responses: replies
                .into_iter()
                .map(|(id, line)| (id.to_owned(), line))
                .collect(),
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

    pub fn missing_total(&mut self, id: Option<String>, line: usize) {
        self.gaps.push(Gap {
            key: id.map(Key::RunningTotal),
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

/// Turn names and cumulative counts belong to a history. Only response IDs are global.
fn qualify_codex_ids(sessions: &mut [Session], evidence: &mut [Evidence]) {
    let index: HashMap<_, _> = sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| s.client == Client::Codex)
        .map(|(n, s)| (s.id.as_str(), n))
        .collect();
    let roots = crate::account::ancestor_roots(evidence.len(), |n| {
        let parent = evidence[n].usage_thread.as_deref()?;
        index.get(parent).copied().filter(|&p| p != n)
    });
    let scopes: Vec<_> = roots
        .into_iter()
        .map(|root| evidence[root].usage_thread.clone())
        .collect();
    for ((session, source), scope) in sessions.iter_mut().zip(evidence).zip(scopes) {
        let Some(scope) = scope else { continue };
        let qualify = |id: &str| format!("{scope}:{id}");
        source.measured_turns = source
            .measured_turns
            .drain()
            .map(|id| qualify(&id))
            .collect();
        for gap in &mut source.gaps {
            if let Some(Key::Turn(id) | Key::RunningTotal(id)) = &mut gap.key {
                *id = qualify(id);
            }
        }
        for turn in &source.legacy {
            for &n in &turn.totals {
                let id = &mut session.usage[n].id;
                *id = qualify(id);
            }
        }
    }
}

/// Keep each model request once: in the session recorded with it, when that session holds a copy,
/// or else in the earliest-started session that does.
///
/// A fork's copied history can start at its parent's time; on a tie the session with no parent
/// keeps the request. A request repeated within one session also counts once.
pub(super) fn count_requests_once(
    sessions: &mut [Session],
    evidence: &mut [Evidence],
) -> Vec<Option<usize>> {
    qualify_codex_ids(sessions, evidence);
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
fn deduplicate_running_totals(
    sessions: &[Session],
    evidence: &mut [Evidence],
    requests: &mut Owners,
) {
    let Some(readings) = requests.get_mut(&Client::Codex) else {
        return;
    };
    let mut replaced = HashSet::new();
    for (session, source) in sessions.iter().zip(evidence.iter()) {
        for legacy in &source.legacy {
            let mut counts = HashMap::new();
            for &n in &legacy.totals {
                let u = &session.usage[n];
                counts
                    .entry((u.prompt(), u.output))
                    .or_insert_with(Vec::new)
                    .push(u);
            }
            for (id, _) in &legacy.responses {
                let Some(&(s, r)) = readings.get(id) else {
                    continue;
                };
                let response = &sessions[s].usage[r];
                if let Some(candidates) = counts.get_mut(&(response.prompt(), response.output))
                    && !candidates.is_empty()
                {
                    let index = candidates
                        .iter()
                        .position(|u| {
                            u.cached == response.cached && u.cache_write == response.cache_write
                        })
                        .unwrap_or(0);
                    replaced.insert(candidates.swap_remove(index).id.as_str());
                }
            }
        }
    }
    for id in replaced {
        readings.remove(id);
    }
    for (session, source) in sessions.iter().zip(evidence) {
        for legacy in std::mem::take(&mut source.legacy) {
            let remaining = legacy
                .totals
                .iter()
                .filter(|&&n| readings.contains_key(&session.usage[n].id))
                .count();
            let missing: Vec<_> = legacy
                .responses
                .into_iter()
                .filter(|(id, _)| !readings.contains_key(id))
                .collect();
            if missing.len() > remaining {
                for (id, line) in missing {
                    source.missing_response(Some(&id), None, line);
                }
            }
        }
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
                Key::Response(id) | Key::RunningTotal(id) => {
                    requests[&session.client].contains_key(id)
                }
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
