use std::collections::{HashMap, HashSet};

use crate::model::{Client, Item, Session};

/// Each session's link to the Codex rollout it inherits from, in the loaded sessions' order.
pub(super) struct History {
    bases: Vec<Option<(usize, u64)>>,
}

impl History {
    pub(super) fn new(sessions: &[Session]) -> Self {
        let rollouts: HashMap<&str, usize> = sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| session.client == Client::Codex)
            .map(|(n, session)| (session.id.as_str(), n))
            .collect();
        let bases = sessions
            .iter()
            .map(|session| {
                let base = session.history_base.as_ref()?;
                Some((*rollouts.get(base.rollout.as_str())?, base.end))
            })
            .collect();
        Self { bases }
    }

    /// Borrow the items visible to session `n`, oldest ancestor first. Each inherited item must
    /// precede every cutoff between its own rollout and `n`, even when positions are unordered.
    pub(super) fn items<'a>(
        &self,
        sessions: &'a [Session],
        n: usize,
    ) -> impl Iterator<Item = &'a Item> + use<'a> {
        let mut ancestors = Vec::new();
        let mut seen = HashSet::new();
        let mut current = n;
        let mut end = u64::MAX;
        while let Some((parent, cutoff)) = self.bases[current] {
            if cutoff == 0 || parent == n || !seen.insert(parent) {
                break;
            }
            end = end.min(cutoff);
            ancestors.push((&sessions[parent].items, end));
            current = parent;
        }
        ancestors
            .into_iter()
            .rev()
            .flat_map(|(items, end)| items.iter().filter(move |item| item.position < end))
            .chain(&sessions[n].items)
    }
}
