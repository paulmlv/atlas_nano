//! The printed report and the CSV, as `docs/REPORT.md` describes them.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use chrono::{DateTime, Duration, Local, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};

use crate::account::{Cost, Tokens, Totals};
use crate::label::Labeling;
use crate::model::{Kind, Label, Role, Session};
use crate::place::Place;
use crate::prices::PriceTable;
use crate::taxonomy::{Entry, Taxonomy};

const UNLABELED: &str = "Unlabeled";
const REVIEWS: &str = "Automatic reviews";
const NO_REPO: &str = "(no repo)";
/// The roles through which a conversation's cost goes to a pull request.
const WORKED_ON: [&str; 2] = ["Contributed to", "Reviewed"];

/// One session in the report, with its cost, its label, and the chat it descends from.
pub struct Row<'a> {
    pub session: &'a Session,
    pub cost: Cost,
    pub label: Option<&'a Label>,
    /// The index in the rows of the chat the user started.
    pub root: usize,
    /// Where that chat started, which its subagents and reviews share.
    pub place: &'a Place,
}

/// Everything the report prints.
pub struct Report<'a> {
    pub period: &'a str,
    pub rows: Vec<Row<'a>>,
    /// None with `--no-label`: the category tables are left out.
    pub taxonomy: Option<&'a Taxonomy>,
    pub labeling: Option<Labeling>,
    pub pull_requests: bool,
    pub prices: &'a PriceTable,
}

/// Halves round up, as people expect of a price: $0.125 is $0.13, not $0.12.
const HALF_UP: RoundingStrategy = RoundingStrategy::MidpointAwayFromZero;

fn money(amount: Decimal) -> String {
    let cents = amount.round_dp_with_strategy(2, HALF_UP).to_string();
    let (whole, fraction) = cents.split_once('.').unwrap_or((&cents, "00"));
    let mut grouped = String::new();
    for (n, digit) in whole.chars().enumerate() {
        if n > 0 && (whole.len() - n) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("${grouped}.{fraction:0<2}")
}

fn share(part: Decimal, whole: Decimal) -> String {
    if whole.is_zero() {
        return "-".to_string();
    }
    let percent = part * Decimal::from(100) / whole;
    format!("{}%", percent.round_dp_with_strategy(0, HALF_UP))
}

fn count(tokens: u64) -> String {
    let value = tokens as f64;
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{:.0}k", value / 1e3),
        1_000_000..10_000_000 => format!("{:.1}M", value / 1e6),
        10_000_000..1_000_000_000 => format!("{:.0}M", value / 1e6),
        _ => format!("{:.1}B", value / 1e9),
    }
}

fn span(duration: Duration) -> String {
    let seconds = duration.num_seconds();
    let hours = seconds as f64 / 3600.0;
    match seconds {
        0 => "-".to_string(),
        1..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..36_000 => format!("{hours:.1}h"),
        _ => format!("{hours:.0}h"),
    }
}

/// Text from a transcript or folder name, with control characters, such as terminal escape codes,
/// and text-direction marks replaced so they show as data and cannot act on the terminal or reorder
/// what it shows.
fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || reorders(c) {
                '\u{fffd}'
            } else {
                c
            }
        })
        .collect()
}

/// The marks that change the direction text shows in: embeddings, overrides, and isolates.
fn reorders(c: char) -> bool {
    matches!(
        c,
        '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
    )
}

/// Lay out rows of cells: the first column left-aligned, the others right-aligned.
fn table(out: &mut String, header: &[&str], rows: &[Vec<String>]) {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| row.iter().map(|cell| printable(cell)).collect())
        .collect();
    let rows = &rows;
    let mut widths: Vec<usize> = header.iter().map(|cell| cell.chars().count()).collect();
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let line = |cells: &mut dyn Iterator<Item = &str>| {
        let mut text = String::new();
        for (n, (cell, width)) in cells.zip(&widths).enumerate() {
            let pad = " ".repeat(width - cell.chars().count());
            if n == 0 {
                write!(text, "{cell}{pad}").unwrap();
            } else {
                write!(text, "  {pad}{cell}").unwrap();
            }
        }
        text.trim_end().to_string()
    };
    writeln!(out, "{}", line(&mut header.iter().copied())).unwrap();
    for row in rows {
        writeln!(out, "{}", line(&mut row.iter().map(String::as_str))).unwrap();
    }
}

const COLUMNS: [&str; 9] = [
    "Convs",
    "Conv%",
    "Subagents",
    "Turns",
    "Working",
    "Wall",
    "Tokens",
    "Spend",
    "Spend%",
];

fn cells(name: String, totals: &Totals, all: &Totals) -> Vec<String> {
    vec![
        name,
        totals.conversations.to_string(),
        share(totals.conversations.into(), all.conversations.into()),
        (totals.subagents + totals.reviews).to_string(),
        totals.turns.to_string(),
        span(totals.working),
        span(totals.wall),
        count(totals.tokens.total()),
        money(totals.spend),
        share(totals.spend, all.spend),
    ]
}

/// Print a table of groups, largest spend first, each followed by its members when it has names.
fn grouped(
    out: &mut String,
    title: &str,
    groups: &[(String, Vec<(String, Totals)>)],
    all: &Totals,
) {
    let mut header = vec![title];
    header.extend(COLUMNS);
    let mut rows = Vec::new();
    let mut ordered: Vec<_> = groups
        .iter()
        .map(|(group, members)| {
            let mut sum = Totals::default();
            for (_, totals) in members {
                merge(&mut sum, totals);
            }
            (group, sum, members)
        })
        .collect();
    ordered.sort_by(|a, b| b.1.spend.cmp(&a.1.spend).then(a.0.cmp(b.0)));
    for (group, sum, members) in ordered {
        let flat = members.len() == 1 && members[0].0.is_empty();
        rows.push(cells(format!("  {group}"), &sum, all));
        if !flat {
            let mut members: Vec<&(String, Totals)> = members.iter().collect();
            members.sort_by(|a, b| b.1.spend.cmp(&a.1.spend).then(a.0.cmp(&b.0)));
            rows.extend(
                members
                    .iter()
                    .map(|(name, totals)| cells(format!("    {name}"), totals, all)),
            );
        }
    }
    table(out, &header, &rows);
    out.push('\n');
}

fn merge(sum: &mut Totals, other: &Totals) {
    sum.conversations += other.conversations;
    sum.subagents += other.subagents;
    sum.reviews += other.reviews;
    sum.turns += other.turns;
    sum.unknown_turns += other.unknown_turns;
    sum.working += other.working;
    sum.wall += other.wall;
    sum.tokens += other.tokens;
    sum.spend += other.spend;
    sum.unpriced_requests += other.unpriced_requests;
}

/// Sum the rows into `(group, name)` buckets, in first-seen order.
fn buckets<'a>(
    rows: &'a [Row<'a>],
    key: impl Fn(&'a Row<'a>) -> (String, String),
) -> Vec<(String, Vec<(String, Totals)>)> {
    let mut groups: Vec<(String, Vec<(String, Totals)>)> = Vec::new();
    for row in rows {
        let (group, name) = key(row);
        let at = groups
            .iter()
            .position(|(g, _)| *g == group)
            .unwrap_or_else(|| {
                groups.push((group, Vec::new()));
                groups.len() - 1
            });
        let members = &mut groups[at].1;
        let member = members
            .iter()
            .position(|(n, _)| *n == name)
            .unwrap_or_else(|| {
                members.push((name, Totals::default()));
                members.len() - 1
            });
        members[member].1.add(row.session, &row.cost);
    }
    groups
}

/// The `(group, label)` of a row on one axis; reviews and unlabeled sessions get their own rows.
fn category(row: &Row, entries: &[Entry], pick: impl Fn(&Label) -> &str) -> (String, String) {
    if row.session.kind == Kind::Review {
        return (REVIEWS.to_string(), String::new());
    }
    match row.label {
        Some(label) => {
            let name = pick(label);
            let group = Taxonomy::group(entries, name).unwrap_or("Other");
            (group.to_string(), name.to_string())
        }
        None => (UNLABELED.to_string(), String::new()),
    }
}

/// A session's title, else the first line of its first user message, as one short line.
pub fn title(session: &Session) -> String {
    let first = session.items.iter().find(|item| item.role == Role::User);
    let text = match &session.title {
        Some(title) => title.as_str(),
        None => first.map_or("", |item| item.text.lines().next().unwrap_or_default()),
    };
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        "Untitled".to_string()
    } else {
        line.chars().take(60).collect()
    }
}

/// Split each session's spend evenly among the pull requests it worked on.
///
/// Returns each pull request's sessions, its spend, and whether any of that spend was split.
fn pull_requests(rows: &[Row]) -> Vec<(String, u64, Decimal, bool)> {
    let mut found: BTreeMap<&str, (u64, Decimal, bool)> = BTreeMap::new();
    for row in rows {
        let worked: Vec<&str> = row
            .label
            .iter()
            .flat_map(|label| &label.prs)
            .filter(|pr| WORKED_ON.contains(&pr.role.as_str()))
            .map(|pr| pr.url.as_str())
            .collect();
        for url in &worked {
            let entry = found.entry(url).or_default();
            entry.0 += 1;
            entry.1 += row.cost.spend / Decimal::from(worked.len());
            entry.2 |= worked.len() > 1;
        }
    }
    let mut listed: Vec<_> = found
        .into_iter()
        .map(|(url, (n, spend, split))| (url.to_string(), n, spend, split))
        .collect();
    listed.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    listed
}

impl Report<'_> {
    /// Render the report as text.
    pub fn render(&self) -> String {
        let mut all = Totals::default();
        for row in &self.rows {
            all.add(row.session, &row.cost);
        }
        let mut out = String::new();
        let reviews = if all.reviews > 0 {
            format!(" · {} reviews", all.reviews)
        } else {
            String::new()
        };
        writeln!(
            out,
            "atlas-nano · {} · {} conversations · {} subagents{reviews}\n",
            self.period, all.conversations, all.subagents
        )
        .unwrap();

        let Tokens {
            input,
            cached,
            cache_write,
            output,
        } = all.tokens;
        let unknown = match all.unknown_turns {
            0 => String::new(),
            turns => format!(" ({turns} turns with no recorded completion)"),
        };
        let unpriced = match all.unpriced_requests {
            0 => String::new(),
            requests => format!(" ({requests} requests have no price and count as $0)"),
        };
        writeln!(out, "Totals").unwrap();
        writeln!(
            out,
            "  Spend (API-equivalent)  {}{unpriced}",
            money(all.spend)
        )
        .unwrap();
        writeln!(
            out,
            "  Time    working {}{unknown} · wall {}",
            span(all.working),
            span(all.wall)
        )
        .unwrap();
        writeln!(
            out,
            "  Tokens  in {} · cached {} · cache-write {} · out {}",
            count(input),
            count(cached),
            count(cache_write),
            count(output)
        )
        .unwrap();
        writeln!(
            out,
            "  Conversations {} · subagents {} · turns {}\n",
            all.conversations, all.subagents, all.turns
        )
        .unwrap();

        if let Some(taxonomy) = self.taxonomy {
            let work = buckets(&self.rows, |row| {
                category(row, &taxonomy.work, |label| &label.work)
            });
            grouped(&mut out, "By type of work", &work, &all);
            let topics = buckets(&self.rows, |row| {
                category(row, &taxonomy.topics, |label| &label.topic)
            });
            grouped(&mut out, "By topic", &topics, &all);
        }

        self.models(&mut out, &all);
        let projects = buckets(&self.rows, |row| (row.place.project.clone(), String::new()));
        grouped(&mut out, "By project", &projects, &all);
        let repos = buckets(&self.rows, |row| {
            let repo = &row.place.repo;
            let repo = if repo.is_empty() { NO_REPO } else { repo };
            (repo.to_string(), String::new())
        });
        grouped(&mut out, "By repo", &repos, &all);
        self.days(&mut out);
        self.top(&mut out);

        if self.pull_requests {
            let listed = pull_requests(&self.rows);
            writeln!(
                out,
                "Pull requests (experimental; ~ marks an amount split across several)"
            )
            .unwrap();
            let rows: Vec<Vec<String>> = listed
                .iter()
                .take(15)
                .map(|(url, sessions, spend, split)| {
                    let mark = if *split { "~" } else { "" };
                    vec![
                        format!("  {url}"),
                        sessions.to_string(),
                        format!("{mark}{}", money(*spend)),
                    ]
                })
                .collect();
            table(&mut out, &["  Pull request", "Sessions", "Spend"], &rows);
            if listed.len() > 15 {
                writeln!(out, "  and {} more", listed.len() - 15).unwrap();
            }
            out.push('\n');
        }

        if let Some(labeling) = &self.labeling {
            writeln!(
                out,
                "Labeling run: {} labeled, {} from cache, {} unlabeled · {}",
                labeling.labeled, labeling.from_cache, labeling.unlabeled, labeling.model
            )
            .unwrap();
            let cost = labeling
                .cost
                .map_or("no price for this model".to_string(), |cost| {
                    format!("API-equivalent {}", money(cost))
                });
            writeln!(
                out,
                "  in {} · out {} tokens · {cost}",
                count(labeling.input_tokens),
                count(labeling.output_tokens)
            )
            .unwrap();
        }
        writeln!(out, "Prices checked {}", self.prices.checked).unwrap();
        out
    }

    /// Requests, tokens, and spend per client and model.
    fn models(&self, out: &mut String, all: &Totals) {
        let mut models: BTreeMap<String, (u64, u64, Decimal, u64)> = BTreeMap::new();
        for row in &self.rows {
            for usage in &row.session.usage {
                let name = format!("  {} / {}", row.session.client.name(), usage.model);
                let entry = models.entry(name).or_default();
                entry.0 += 1;
                entry.1 += usage.tokens();
                match self.prices.cost(row.session.client, usage) {
                    Some(cost) => entry.2 += cost,
                    None => entry.3 += 1,
                }
            }
        }
        let mut listed: Vec<_> = models.into_iter().collect();
        listed.sort_by(|a, b| b.1.2.cmp(&a.1.2).then(a.0.cmp(&b.0)));
        let rows: Vec<Vec<String>> = listed
            .into_iter()
            .map(|(name, (requests, tokens, spend, unpriced))| {
                let shown = if unpriced == requests {
                    "no price".to_string()
                } else {
                    money(spend)
                };
                vec![
                    name,
                    requests.to_string(),
                    count(tokens),
                    shown,
                    share(spend, all.spend),
                ]
            })
            .collect();
        table(
            out,
            &["By client / model", "Requests", "Tokens", "Spend", "Spend%"],
            &rows,
        );
        out.push('\n');
    }

    /// Spend per local day of the request, with a bar.
    fn days(&self, out: &mut String) {
        let mut days: BTreeMap<String, Decimal> = BTreeMap::new();
        for row in &self.rows {
            for usage in &row.session.usage {
                let day = local_day(usage.at);
                *days.entry(day).or_default() += self
                    .prices
                    .cost(row.session.client, usage)
                    .unwrap_or_default();
            }
        }
        let top = days.values().copied().max().unwrap_or_default();
        writeln!(out, "By day").unwrap();
        for (day, spend) in &days {
            let width = if top.is_zero() {
                0
            } else {
                (spend * Decimal::from(30) / top)
                    .round()
                    .to_usize()
                    .unwrap_or(0)
            };
            writeln!(out, "  {day}  {:>10}  {}", money(*spend), "█".repeat(width)).unwrap();
        }
        out.push('\n');
    }

    /// The five chats that cost the most, each with the sessions it started.
    fn top(&self, out: &mut String) {
        let mut spend: HashMap<usize, Decimal> = HashMap::new();
        for row in &self.rows {
            *spend.entry(row.root).or_default() += row.cost.spend;
        }
        // A subagent whose chat was not loaded is its own root, but it is not a conversation.
        let mut chats: Vec<(usize, Decimal)> = spend
            .into_iter()
            .filter(|&(root, _)| self.rows[root].session.kind == Kind::Primary)
            .collect();
        chats.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        writeln!(out, "Top 5 conversations by spend, subagents included").unwrap();
        let rows: Vec<Vec<String>> = chats
            .iter()
            .take(5)
            .map(|&(root, spend)| {
                let session = self.rows[root].session;
                let day = session.started.map_or(String::new(), local_day);
                vec![
                    format!("  {}", title(session)),
                    day,
                    self.rows[root].place.project.clone(),
                    money(spend),
                ]
            })
            .collect();
        table(
            out,
            &["  Conversation", "Started", "Project", "Spend"],
            &rows,
        );
        out.push('\n');
    }

    /// Render one CSV row per session, with a header.
    pub fn csv(&self) -> String {
        let mut out = String::from(
            "client,session_id,parent_client,parent_session_id,kind,project,repo,started,ended,working_seconds,\
             wall_seconds,turns,untimed_turns,tool_calls,input,cached,cache_write,output,spend_usd,\
             unpriced_requests,models,work,topic,prs,label_model,label_from_cache\n",
        );
        for row in &self.rows {
            let session = row.session;
            let time = |at: Option<DateTime<Utc>>| at.map_or(String::new(), |at| at.to_rfc3339());
            let mut models: Vec<&str> = session
                .usage
                .iter()
                .map(|usage| usage.model.as_str())
                .collect();
            models.sort_unstable();
            models.dedup();
            let label = |pick: fn(&Label) -> String| row.label.map_or(String::new(), pick);
            let kind = match session.kind {
                Kind::Primary => "primary",
                Kind::Subagent => "subagent",
                Kind::Review => "review",
            };
            let fields = [
                session.client.name().to_string(),
                session.id.clone(),
                session
                    .parent
                    .as_ref()
                    .map_or("", |parent| parent.client.name())
                    .to_string(),
                session
                    .parent
                    .as_ref()
                    .map_or(String::new(), |parent| parent.id.clone()),
                kind.to_string(),
                row.place.project.clone(),
                row.place.repo.clone(),
                time(session.started),
                time(session.ended),
                session.working().num_seconds().to_string(),
                session
                    .wall()
                    .map_or(String::new(), |wall| wall.num_seconds().to_string()),
                session.turns.len().to_string(),
                session
                    .turns
                    .iter()
                    .filter(|turn| turn.working.is_none())
                    .count()
                    .to_string(),
                session.tool_calls.to_string(),
                row.cost.tokens.input.to_string(),
                row.cost.tokens.cached.to_string(),
                row.cost.tokens.cache_write.to_string(),
                row.cost.tokens.output.to_string(),
                row.cost.spend.round_dp(6).to_string(),
                row.cost.unpriced_requests.to_string(),
                models.join(" "),
                label(|label| label.work.clone()),
                label(|label| label.topic.clone()),
                label(|label| {
                    let prs = label.prs.iter().map(|pr| format!("{}={}", pr.role, pr.url));
                    prs.collect::<Vec<_>>().join("; ")
                }),
                label(|label| label.model.clone()),
                label(|label| label.from_cache.to_string()),
            ];
            let quoted: Vec<String> = fields.iter().map(|field| quote(field)).collect();
            writeln!(out, "{}", quoted.join(",")).unwrap();
        }
        out
    }
}

/// The local date of a moment, with the time zone's offset at that moment, so daylight saving time
/// moves no request to the wrong day.
fn local_day(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%Y-%m-%d").to_string()
}

/// Quote a CSV field that holds a comma, a quote, or a line break.
/// Quote a CSV field when it needs it. A field a spreadsheet would read as a formula, starting with
/// `=`, `+`, `-`, `@`, a tab, or a carriage return, gets a leading `'` so it stays text.
fn quote(field: &str) -> String {
    let field = if field.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{field}")
    } else {
        field.to_string()
    };
    let field = field.as_str();
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_csv_fields_as_text() {
        assert_eq!(quote("app"), "app");
        assert_eq!(quote("a,b"), "\"a,b\"");
        assert_eq!(quote("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(quote("=1+1"), "'=1+1");
        assert_eq!(quote("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(quote("-x,y"), "\"'-x,y\"");
    }

    #[test]
    fn shows_control_characters_as_data() {
        assert_eq!(printable("a\u{1b}[2Jb"), "a\u{fffd}[2Jb");
        assert_eq!(printable("file\u{202e}txt.exe"), "file\u{fffd}txt.exe");
        assert_eq!(printable("plain"), "plain");
    }

    #[test]
    fn rounds_halves_up() {
        assert_eq!(money(Decimal::new(125, 3)), "$0.13");
        assert_eq!(money(Decimal::new(1_234_565, 3)), "$1,234.57");
        assert_eq!(share(Decimal::new(25, 1), Decimal::from(100)), "3%");
    }
}
