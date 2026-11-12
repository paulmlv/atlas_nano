//! Accounting: which sessions a run covers and what each one cost.

use std::collections::HashMap;
use std::ops::AddAssign;

use chrono::{DateTime, Duration, Utc};
use rust_decimal::Decimal;

use crate::model::{Kind, Session, SessionRef};
use crate::prices::PriceTable;

/// Token counts by billed category; `cache_write` holds both five-minute and one-hour writes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    pub input: u64,
    pub cached: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl Tokens {
    pub fn total(&self) -> u64 {
        self.input + self.cached + self.cache_write + self.output
    }
}

impl AddAssign for Tokens {
    fn add_assign(&mut self, other: Tokens) {
        self.input += other.input;
        self.cached += other.cached;
        self.cache_write += other.cache_write;
        self.output += other.output;
    }
}

/// One session's tokens and API-equivalent spend. Unpriced requests cost nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cost {
    pub tokens: Tokens,
    pub spend: Decimal,
    pub unpriced_requests: u64,
}

/// Price a session's own model requests.
pub fn cost(session: &Session, prices: &PriceTable) -> Cost {
    let mut cost = Cost::default();
    for usage in &session.usage {
        cost.tokens += Tokens {
            input: usage.input,
            cached: usage.cached,
            cache_write: usage.cache_write + usage.cache_write_1h,
            output: usage.output,
        };
        match prices.cost(session.client, usage) {
            Some(price) => cost.spend += price,
            None => cost.unpriced_requests += 1,
        }
    }
    cost
}

/// Sums over a group of sessions. Conversations are the chats the user started.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub conversations: u64,
    pub subagents: u64,
    pub reviews: u64,
    pub turns: u64,
    pub unknown_turns: u64,
    pub working: Duration,
    pub wall: Duration,
    pub tokens: Tokens,
    pub spend: Decimal,
    pub unpriced_requests: u64,
}

impl Totals {
    pub fn add(&mut self, session: &Session, cost: &Cost) {
        match session.kind {
            Kind::Primary => self.conversations += 1,
            Kind::Subagent => self.subagents += 1,
            Kind::Review => self.reviews += 1,
        }
        self.turns += session.turns.len() as u64;
        self.unknown_turns += session
            .turns
            .iter()
            .filter(|turn| turn.working.is_none())
            .count() as u64;
        self.working += session.working();
        self.wall += session.wall().unwrap_or_default();
        self.tokens += cost.tokens;
        self.spend += cost.spend;
        self.unpriced_requests += cost.unpriced_requests;
    }
}

/// Map each session to the chat the user started that it descends from, itself if it is one.
///
/// A session whose parent was not loaded is its own root. A loop of parents, which valid records
/// never form, is rooted at the session where the walk first met it. Each session is walked once.
pub fn roots(sessions: &[Session]) -> Vec<usize> {
    let index: HashMap<SessionRef, usize> = sessions
        .iter()
        .enumerate()
        .map(|(n, session)| (session.reference(), n))
        .collect();
    let parent = |n: usize| {
        let session = &sessions[n];
        let parent = session.parent.as_ref()?;
        (session.kind != Kind::Primary).then(|| index.get(parent).copied())?
    };
    let mut roots: Vec<Option<usize>> = vec![None; sessions.len()];
    let mut walking = vec![false; sessions.len()];
    for start in 0..sessions.len() {
        let mut path = Vec::new();
        let mut n = start;
        let root = loop {
            if let Some(root) = roots[n] {
                break root;
            }
            if walking[n] {
                break n;
            }
            walking[n] = true;
            path.push(n);
            match parent(n) {
                Some(next) => n = next,
                None => break n,
            }
        };
        for n in path {
            roots[n] = Some(root);
        }
    }
    roots
        .into_iter()
        .map(|root| root.expect("every session was walked"))
        .collect()
}

/// Whether each session is in the period: its root chat started at or after `cutoff`.
pub fn in_period(
    sessions: &[Session],
    roots: &[usize],
    cutoff: Option<DateTime<Utc>>,
) -> Vec<bool> {
    roots
        .iter()
        .map(|&root| match (cutoff, sessions[root].started) {
            (None, _) => true,
            (Some(cutoff), Some(started)) => started >= cutoff,
            (Some(_), None) => false,
        })
        .collect()
}

#[cfg(test)]
pub mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::model::{Client, SessionRef, Turn, Usage};

    pub fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 10, hour, 0, 0).unwrap()
    }

    pub fn session(id: &str, kind: Kind, parent: Option<&str>, started: u32) -> Session {
        Session {
            client: Client::Claude,
            id: id.to_string(),
            parent: parent.map(|id| SessionRef {
                client: Client::Claude,
                id: id.to_string(),
            }),
            kind,
            cwd: Some("/work/app".to_string()),
            title: None,
            repository: None,
            branch: None,
            started: Some(at(started)),
            ended: Some(at(started + 2)),
            items: Vec::new(),
            tool_calls: 3,
            turns: vec![
                Turn {
                    at: at(started),
                    working: Some(Duration::minutes(30)),
                },
                Turn {
                    at: at(started + 1),
                    working: None,
                },
            ],
            usage: vec![
                Usage {
                    id: format!("{id}-1"),
                    owner: None,
                    at: at(started),
                    model: "claude-haiku-5-5".to_string(),
                    fast: false,
                    input: 100_000,
                    cached: 0,
                    cache_write: 0,
                    cache_write_1h: 0,
                    output: 1_000_000,
                },
                Usage {
                    id: format!("{id}-2"),
                    owner: None,
                    at: at(started + 1),
                    model: "unpriced".to_string(),
                    fast: false,
                    input: 5,
                    cached: 0,
                    cache_write: 1,
                    cache_write_1h: 2,
                    output: 0,
                },
            ],
            history_base: None,
        }
    }

    #[test]
    fn a_subagent_is_in_the_period_exactly_when_its_chat_is() {
        let sessions = [
            session("old", Kind::Primary, None, 1),
            session("agent", Kind::Subagent, Some("old"), 9),
            session("nested", Kind::Subagent, Some("agent"), 10),
            session("new", Kind::Primary, None, 8),
            session("fork", Kind::Primary, Some("old"), 9),
            session("orphan", Kind::Subagent, Some("missing"), 9),
        ];
        let roots = roots(&sessions);
        assert_eq!(roots, [0, 0, 0, 3, 4, 5]);
        let period = in_period(&sessions, &roots, Some(at(8)));
        assert_eq!(period, [false, false, false, true, true, true]);
        assert!(
            in_period(&sessions, &roots, None)
                .iter()
                .all(|&covered| covered)
        );
    }

    #[test]
    fn a_long_chain_reaches_its_chat_and_a_loop_ends() {
        let mut sessions = vec![session("chat", Kind::Primary, None, 1)];
        for n in 1..100 {
            let parent = if n == 1 {
                "chat".to_string()
            } else {
                format!("agent-{}", n - 1)
            };
            sessions.push(session(
                &format!("agent-{n}"),
                Kind::Subagent,
                Some(&parent),
                1,
            ));
        }
        sessions.push(session("a", Kind::Subagent, Some("b"), 1));
        sessions.push(session("b", Kind::Subagent, Some("a"), 1));
        let roots = roots(&sessions);
        assert!(roots[..100].iter().all(|&root| root == 0));
        assert_eq!(roots[100..], [100, 100]);
    }

    #[test]
    fn totals_sum_priced_spend_and_count_what_is_unknown() {
        let prices = PriceTable::built_in();
        let mut totals = Totals::default();
        for session in [
            session("a", Kind::Primary, None, 1),
            session("b", Kind::Subagent, Some("a"), 2),
        ] {
            totals.add(&session, &cost(&session, &prices));
        }
        assert_eq!(
            (totals.conversations, totals.subagents, totals.turns),
            (1, 1, 4)
        );
        assert_eq!(totals.unknown_turns, 2);
        assert_eq!(totals.working, Duration::hours(1));
        assert_eq!(totals.wall, Duration::hours(4));
        // Haiku: $0.01 input + $0.50 output per session; the other request has no price.
        assert_eq!(totals.spend, Decimal::new(102, 2));
        assert_eq!(totals.unpriced_requests, 2);
        assert_eq!(
            totals.tokens,
            Tokens {
                input: 200_010,
                cached: 0,
                cache_write: 6,
                output: 2_000_000
            }
        );
    }
}
