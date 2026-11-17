//! Turn a session's kept parts into the text sent to the labeling model, in chunks that fit.

use crate::model::{Item, Role};

const TRUNCATED: &str = "\n[truncated]";

/// Cut text to at most `limit` bytes at a character boundary, marking the cut.
fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit.saturating_sub(TRUNCATED.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATED}", &text[..end])
}

/// Render each kept part as `[role] text`. Without `tools`, tool calls are left out.
pub fn render(items: &[Item], tools: bool) -> Vec<(Role, String)> {
    items
        .iter()
        .filter(|item| tools || item.role != Role::Tool)
        .map(|item| (item.role, format!("{} {}", item.role.marker(), item.text)))
        .collect()
}

/// Split rendered parts into chunks of at most `budget` bytes, joined by blank lines.
///
/// A chunk starts at a user message, so it holds whole turns; a session with no user message
/// starts its chunks at messages from other agents. A turn larger than the budget is cut between
/// parts, and a part larger than the budget is truncated. Every chunk after the first starts with
/// the session's first request, marked as context.
pub fn chunks(parts: &[(Role, String)], budget: usize) -> Vec<String> {
    let whole: usize = parts.iter().map(|(_, text)| text.len() + 2).sum();
    if whole <= budget + 2 {
        let texts: Vec<&str> = parts.iter().map(|(_, text)| text.as_str()).collect();
        return if texts.is_empty() {
            Vec::new()
        } else {
            vec![texts.join("\n\n")]
        };
    }
    let opener = [Role::User, Role::Agent]
        .into_iter()
        .find(|role| parts.iter().any(|(part, _)| part == role));
    let first = opener.and_then(|role| parts.iter().find(|(part, _)| *part == role));
    let prefix = first.map_or(String::new(), |(_, text)| {
        format!(
            "[context] The session's first request was:\n{}\n\n",
            truncate(text, budget / 4)
        )
    });
    // After the first chunk, the prefix takes part of the budget.
    let room = |chunk: usize| {
        if chunk == 0 {
            budget
        } else {
            budget.saturating_sub(prefix.len())
        }
    };

    // Group the parts into turns, then pack whole turns into chunks.
    let mut turns: Vec<Vec<&str>> = Vec::new();
    for (role, text) in parts {
        if turns.is_empty() || Some(*role) == opener || opener.is_none() {
            turns.push(Vec::new());
        }
        turns.last_mut().expect("a turn was just opened").push(text);
    }
    let mut chunks: Vec<Vec<String>> = vec![Vec::new()];
    let mut used = 0;
    let size = |texts: &[&str]| texts.iter().map(|text| text.len() + 2).sum::<usize>();
    for turn in turns {
        let limit = room(chunks.len() - 1);
        if used > 0 && used + size(&turn) > limit {
            chunks.push(Vec::new());
            used = 0;
        }
        for text in turn {
            let limit = room(chunks.len() - 1);
            if used > 0 && used + text.len() + 2 > limit {
                chunks.push(Vec::new());
                used = 0;
            }
            let text = truncate(text, room(chunks.len() - 1));
            used += text.len() + 2;
            chunks
                .last_mut()
                .expect("there is always a chunk")
                .push(text);
        }
    }
    chunks
        .into_iter()
        .enumerate()
        .map(|(n, texts)| {
            format!(
                "{}{}",
                if n == 0 { "" } else { &prefix },
                texts.join("\n\n")
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(texts: &[(Role, &str)]) -> Vec<(Role, String)> {
        texts
            .iter()
            .map(|(role, text)| (*role, format!("{} {text}", role.marker())))
            .collect()
    }

    #[test]
    fn renders_parts_and_leaves_tools_out_in_no_tools_mode() {
        let items = [
            Item {
                position: 0,
                role: Role::User,
                text: "Fix it".into(),
            },
            Item {
                position: 1,
                role: Role::Tool,
                text: "Bash {}".into(),
            },
        ];
        let texts = |tools| {
            render(&items, tools)
                .into_iter()
                .map(|(_, text)| text)
                .collect::<Vec<_>>()
        };
        assert_eq!(texts(true), ["[user] Fix it", "[tool] Bash {}"]);
        assert_eq!(texts(false), ["[user] Fix it"]);
    }

    #[test]
    fn a_session_that_fits_is_one_chunk() {
        let parts = parts(&[(Role::User, "a"), (Role::Assistant, "b")]);
        assert_eq!(chunks(&parts, 1000), ["[user] a\n\n[assistant] b"]);
        assert!(chunks(&[], 1000).is_empty());
    }

    #[test]
    fn chunks_hold_whole_turns_and_repeat_the_first_request() {
        let long = "x".repeat(60);
        let parts = parts(&[
            (Role::User, "first request"),
            (Role::Assistant, &long),
            (Role::User, "second"),
            (Role::Assistant, &long),
            (Role::User, "third"),
            (Role::Assistant, &long),
        ]);
        let chunks = chunks(&parts, 180);
        assert_eq!(chunks.len(), 3);
        assert!(chunks[0].starts_with("[user] first request\n\n[assistant] xxx"));
        let context = "[context] The session's first request was:\n[user] first request\n\n";
        assert!(chunks[1].starts_with(&format!("{context}[user] second\n\n[assistant] x")));
        assert!(chunks[2].starts_with(&format!("{context}[user] third")));
        assert!(chunks.iter().all(|chunk| chunk.len() <= 180));
    }

    #[test]
    fn an_oversized_turn_is_cut_between_parts_and_an_oversized_part_truncated() {
        let big = "é".repeat(400);
        let parts = parts(&[
            (Role::Agent, "task"),
            (Role::Tool, &"a".repeat(90)),
            (Role::Tool, &"b".repeat(90)),
            (Role::Assistant, &big),
        ]);
        let chunks = chunks(&parts, 160);
        assert!(chunks.len() >= 3);
        assert!(chunks[0].starts_with("[agent] task"));
        assert!(chunks[1].starts_with("[context] The session's first request was:\n[agent] task"));
        assert!(chunks.last().unwrap().ends_with("[truncated]"));
        assert!(chunks.iter().all(|chunk| chunk.len() <= 160));
    }

    #[test]
    fn a_session_with_no_request_gets_no_prefix() {
        let parts = parts(&[
            (Role::Assistant, &"a".repeat(80)),
            (Role::Assistant, &"b".repeat(80)),
        ]);
        let chunks = chunks(&parts, 100);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[1].starts_with("[assistant] b"));
    }
}
