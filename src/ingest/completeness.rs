//! Evidence that a model replied but its token counts could not be read.

use std::collections::{HashMap, HashSet};

use super::ownership_rank;
use crate::model::{Client, Session, Usage};

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

#[derive(Default)]
pub(super) struct Evidence {
    gaps: Vec<Gap>,
    pub measured_turns: HashSet<String>,
}

impl Evidence {
    /// Use a shorter copy only for responses that appear in the chosen transcript.
    pub fn recover_from_copy(&self, session: &mut Session, other: Vec<Usage>) {
        if self.gaps.is_empty() || other.is_empty() {
            return;
        }
        let mut wanted: HashSet<&str> = self
            .gaps
            .iter()
            .filter_map(|gap| match &gap.key {
                Some(Key::Response(id)) => Some(id.as_str()),
                _ => None,
            })
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

/// Return the first unresolved usage gap in each session. Streaming blocks, resumed sessions,
/// and forks can supply a measurement, or repeat the same gap, in another record.
pub(super) fn resolve(sessions: &mut [Session], evidence: &[Evidence]) -> Vec<Option<usize>> {
    let mut lines = vec![None; sessions.len()];
    if evidence.iter().all(|source| source.gaps.is_empty()) {
        return lines;
    }
    let mut measured = HashMap::new();
    for (n, session) in sessions.iter().enumerate() {
        for usage in &session.usage {
            let holder = measured
                .entry((session.client, usage.id.as_str()))
                .or_insert((n, usage));
            if ownership_rank(session, usage.owner.as_deref())
                < ownership_rank(&sessions[holder.0], holder.1.owner.as_deref())
            {
                *holder = (n, usage);
            }
        }
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
            Key::Response(id)
                if let Some(&(source, usage)) = measured.get(&(client, id.as_str())) =>
            {
                // A fork can have the only readable copy of the original session's request.
                // Restore it to the better-ranked holder before request deduplication.
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
        sessions[n].usage.push(usage);
    }
    lines
}
