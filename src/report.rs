//! The printed report and the CSV, as `docs/REPORT.md` describes them.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use chrono::{DateTime, Duration, Local, Utc};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};

use crate::account::{Cost, Tokens, Totals};
use crate::label::Labeling;
use crate::model::{Kind, Label, Role, Session};
#[cfg(test)]
use crate::paint::Theme;
use crate::paint::{self, Paint, Tone};
use crate::place::Place;
use crate::prices::PriceTable;
use crate::taxonomy::{Entry, Taxonomy};

const UNLABELED: &str = "Unlabeled";
const REVIEWS: &str = "Automatic reviews";
pub(crate) const NO_REPO: &str = "(no repo)";
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
    /// The most rows the ranked tables show; the rest share one row. None shows them all.
    pub top: Option<usize>,
    pub prices: &'a PriceTable,
    pub paint: Paint,
}

/// Halves round up, as people expect of a price: $0.125 is $0.13, not $0.12.
const HALF_UP: RoundingStrategy = RoundingStrategy::MidpointAwayFromZero;

pub(crate) fn money(amount: Decimal) -> String {
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

/// A whole percentage; `0%` only for none, and `<1%` for a part too small to round to 1%.
pub(crate) fn share(part: Decimal, whole: Decimal) -> String {
    if whole.is_zero() {
        return "-".to_string();
    }
    let percent = (part * Decimal::from(100) / whole).round_dp_with_strategy(0, HALF_UP);
    if percent.is_zero() && !part.is_zero() {
        return "<1%".to_string();
    }
    format!("{percent}%")
}

/// A token count in a short form. Each range ends where its form would round up into the next
/// one, so 999,999 is `1.0M`, not `1000k`.
pub(crate) fn count(tokens: u64) -> String {
    let value = tokens as f64;
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..999_500 => format!("{:.0}k", value / 1e3),
        999_500..9_950_000 => format!("{:.1}M", value / 1e6),
        9_950_000..999_500_000 => format!("{:.0}M", value / 1e6),
        _ => format!("{:.1}B", value / 1e9),
    }
}

/// A length of time in a short form. One decimal of hours ends at 9.95 hours, which is `10h`.
pub(crate) fn span(duration: Duration) -> String {
    let seconds = duration.num_seconds();
    let hours = seconds as f64 / 3600.0;
    match seconds {
        0 => "-".to_string(),
        1..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..35_820 => format!("{hours:.1}h"),
        _ => format!("{hours:.0}h"),
    }
}

/// Lay out rows of cells: the first column left-aligned, the others right-aligned.
///
/// Every cell comes from `paint`, which shows control characters as data, so text from a
/// transcript or folder name cannot act on the terminal. The header's first cell is the table's
/// title unless it is indented, in which case it names a column like the other header cells.
fn table(out: &mut String, paint: Paint, header: &[&str], rows: &[Vec<String>]) {
    let header: Vec<String> = header
        .iter()
        .enumerate()
        .map(|(n, cell)| {
            let title = n == 0 && !cell.starts_with(' ');
            paint.tone(if title { Tone::Heading } else { Tone::Dim }, cell)
        })
        .collect();
    let mut widths: Vec<usize> = header.iter().map(|cell| paint::width(cell)).collect();
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(paint::width(cell));
        }
    }
    let line = |cells: &mut dyn Iterator<Item = &str>| {
        let mut text = String::new();
        for (n, (cell, width)) in cells.zip(&widths).enumerate() {
            let pad = " ".repeat(width - paint::width(cell));
            if n == 0 {
                write!(text, "{cell}{pad}").unwrap();
            } else {
                write!(text, "  {pad}{cell}").unwrap();
            }
        }
        text.trim_end().to_string()
    };
    writeln!(out, "{}", line(&mut header.iter().map(String::as_str))).unwrap();
    for row in rows {
        writeln!(out, "{}", line(&mut row.iter().map(String::as_str))).unwrap();
    }
}

pub(crate) const COLUMNS: [&str; 9] = [
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

/// A row of the category and project tables; `strong` rows head a group of members.
fn cells(paint: Paint, strong: bool, name: &str, totals: &Totals, all: &Totals) -> Vec<String> {
    let cell = |tone: Tone, text: String| paint.styled(tone, strong, &text);
    vec![
        cell(Tone::Plain, name.to_string()),
        cell(Tone::Plain, totals.conversations.to_string()),
        cell(
            Tone::Dim,
            share(totals.conversations.into(), all.conversations.into()),
        ),
        cell(Tone::Plain, (totals.subagents + totals.reviews).to_string()),
        cell(Tone::Plain, totals.turns.to_string()),
        cell(Tone::Plain, span(totals.working)),
        cell(Tone::Plain, span(totals.wall)),
        cell(Tone::Plain, count(totals.tokens.total())),
        cell(Tone::Money, money(totals.spend)),
        cell(Tone::Dim, share(totals.spend, all.spend)),
    ]
}

/// Whether a row has no conversations and no tokens, so no spend; such rows are left out.
fn empty(totals: &Totals) -> bool {
    totals.conversations == 0 && totals.tokens.total() == 0
}

/// A ranked table's title, saying how many rows it shows when they are limited.
fn ranked(title: &str, top: Option<usize>) -> String {
    match top {
        Some(n) => format!("{title} (top {n})"),
        None => title.to_string(),
    }
}

/// The name of the row that stands for the `n` rows past the limit.
fn others(n: usize) -> String {
    format!("  ({n} {})", if n == 1 { "other" } else { "others" })
}

/// How many of `len` ranked rows are shown before the rest share one row.
fn shown(len: usize, top: Option<usize>) -> usize {
    top.map_or(len, |top| top.min(len))
}

/// Print a table of groups, each followed by its members. With `top`, the groups past it share one
/// row.
fn grouped(
    out: &mut String,
    paint: Paint,
    title: &str,
    groups: &[Group],
    all: &Totals,
    top: Option<usize>,
) {
    let title = ranked(title, top);
    let mut header = vec![title.as_str()];
    header.extend(COLUMNS);
    let nested = nested(groups);
    let shown = shown(groups.len(), top);
    let mut rows = Vec::new();
    for group in &groups[..shown] {
        let line = &group.line;
        rows.push(cells(
            paint,
            nested,
            &format!("  {}", line.name),
            &line.totals,
            all,
        ));
        rows.extend(
            group
                .members
                .iter()
                .map(|m| cells(paint, false, &format!("    {}", m.name), &m.totals, all)),
        );
    }
    let rest = &groups[shown..];
    if !rest.is_empty() {
        let mut sum = Totals::default();
        for group in rest {
            sum += &group.line.totals;
        }
        rows.push(cells(paint, nested, &others(rest.len()), &sum, all));
    }
    table(out, paint, &header, &rows);
    out.push('\n');
}

/// A line of a grouped table: its name, the report rows it holds, and their sums.
pub(crate) struct Line {
    pub(crate) name: String,
    pub(crate) rows: Vec<usize>,
    pub(crate) totals: Totals,
}

/// A group and the members under it; a group with no names under it has no members.
pub(crate) struct Group {
    pub(crate) line: Line,
    pub(crate) members: Vec<Line>,
}

/// Groups are bold only when some have members under them.
pub(crate) fn nested(groups: &[Group]) -> bool {
    groups.iter().any(|group| !group.members.is_empty())
}

/// Sort the rows into `(group, name)` lines, largest spend first. Lines with no conversations and
/// no tokens are left out.
pub(crate) fn groups(rows: &[Row], key: impl Fn(&Row) -> (String, String)) -> Vec<Group> {
    let mut found: BTreeMap<String, BTreeMap<String, Vec<usize>>> = BTreeMap::new();
    for (n, row) in rows.iter().enumerate() {
        let (group, name) = key(row);
        found
            .entry(group)
            .or_default()
            .entry(name)
            .or_default()
            .push(n);
    }
    let line = |name: String, at: Vec<usize>| {
        let mut totals = Totals::default();
        for &n in &at {
            totals.add(rows[n].session, &rows[n].cost);
        }
        Line {
            name,
            rows: at,
            totals,
        }
    };
    let by_spend = |a: &Line, b: &Line| {
        (b.totals.spend.cmp(&a.totals.spend)).then_with(|| a.name.cmp(&b.name))
    };
    let mut groups: Vec<Group> = found
        .into_iter()
        .map(|(group, members)| {
            let mut members: Vec<Line> = members
                .into_iter()
                .map(|(name, at)| line(name, at))
                .collect();
            let all = members
                .iter()
                .flat_map(|m| m.rows.iter().copied())
                .collect();
            if members.len() == 1 && members[0].name.is_empty() {
                members.clear();
            }
            members.retain(|member| !empty(&member.totals));
            members.sort_by(by_spend);
            Group {
                line: line(group, all),
                members,
            }
        })
        .filter(|group| !empty(&group.line.totals))
        .collect();
    groups.sort_by(|a, b| by_spend(&a.line, &b.line));
    groups
}

/// A row's `(repo, "")`, `(no repo)` outside a repository.
pub(crate) fn repo(row: &Row) -> (String, String) {
    let repo = &row.place.repo;
    let repo = if repo.is_empty() { NO_REPO } else { repo };
    (repo.to_string(), String::new())
}

/// The `(group, label)` of a row on one axis; reviews and unlabeled sessions get their own rows.
pub(crate) fn category(
    row: &Row,
    entries: &[Entry],
    pick: impl Fn(&Label) -> &str,
) -> (String, String) {
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

/// A session's title, else the first line of its first user message, else of the task an agent gave
/// it, as one short line.
pub fn title(session: &Session) -> String {
    let first = [Role::User, Role::Agent]
        .into_iter()
        .find_map(|role| session.items.iter().find(|item| item.role == role));
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

/// One pull request's sessions and its share of their spend.
pub(crate) struct PrSpend {
    pub(crate) url: String,
    /// The rows of the sessions that worked on it.
    pub(crate) rows: Vec<usize>,
    pub(crate) spend: Decimal,
    /// Whether any of the spend comes from a session that worked on several pull requests.
    pub(crate) split: bool,
    /// The tokens of those sessions; a pull request with none is left out.
    tokens: u64,
}

/// The pull requests a session worked on, each of which gets an even share of its spend.
pub(crate) fn worked_on<'a>(row: &Row<'a>) -> Vec<&'a str> {
    row.label
        .iter()
        .flat_map(|label| &label.prs)
        .filter(|pr| pr.role.worked_on())
        .map(|pr| pr.url.as_str())
        .collect()
}

/// Split each session's spend evenly among the pull requests it worked on, largest spend first.
/// A pull request whose sessions used no tokens is left out.
pub(crate) fn pull_requests(rows: &[Row]) -> Vec<PrSpend> {
    let mut found: BTreeMap<&str, PrSpend> = BTreeMap::new();
    for (n, row) in rows.iter().enumerate() {
        let worked = worked_on(row);
        for url in &worked {
            let pr = found.entry(url).or_insert_with(|| PrSpend {
                url: url.to_string(),
                rows: Vec::new(),
                spend: Decimal::ZERO,
                split: false,
                tokens: 0,
            });
            pr.rows.push(n);
            pr.spend += row.cost.spend / Decimal::from(worked.len());
            pr.split |= worked.len() > 1;
            pr.tokens += row.cost.tokens.total();
        }
    }
    let mut listed: Vec<PrSpend> = found.into_values().filter(|pr| pr.tokens > 0).collect();
    listed.sort_by(|a, b| b.spend.cmp(&a.spend).then(a.url.cmp(&b.url)));
    listed
}

/// One model's requests, tokens, and spend, how many of the requests have no price, and the rows
/// that made them.
#[derive(Default)]
pub(crate) struct ModelUse {
    /// `client / model`.
    pub(crate) name: String,
    pub(crate) requests: u64,
    pub(crate) tokens: u64,
    pub(crate) spend: Decimal,
    pub(crate) unpriced: u64,
    pub(crate) rows: Vec<usize>,
}

/// The requests per client and model, largest spend first. A model whose requests used no tokens
/// is left out.
pub(crate) fn models(rows: &[Row]) -> Vec<ModelUse> {
    let mut models: BTreeMap<String, ModelUse> = BTreeMap::new();
    for (n, row) in rows.iter().enumerate() {
        for (usage, price) in row.session.usage.iter().zip(&row.cost.requests) {
            let name = format!("{} / {}", row.session.client.name(), usage.model);
            let model = models.entry(name.clone()).or_default();
            model.name = name;
            model.requests += 1;
            model.tokens += usage.tokens();
            match price {
                Some(cost) => model.spend += cost,
                None => model.unpriced += 1,
            }
            if model.rows.last() != Some(&n) {
                model.rows.push(n);
            }
        }
    }
    let mut listed: Vec<ModelUse> = models
        .into_values()
        .filter(|model| model.tokens > 0)
        .collect();
    listed.sort_by(|a, b| b.spend.cmp(&a.spend).then_with(|| a.name.cmp(&b.name)));
    listed
}

/// One local day's spend and the rows with requests on it.
pub(crate) struct DaySpend {
    pub(crate) day: String,
    pub(crate) spend: Decimal,
    pub(crate) rows: Vec<usize>,
}

/// Spend per local day of the request, in date order.
pub(crate) fn days(rows: &[Row]) -> Vec<DaySpend> {
    let mut days: BTreeMap<String, (Decimal, Vec<usize>)> = BTreeMap::new();
    for (n, row) in rows.iter().enumerate() {
        for (usage, price) in row.session.usage.iter().zip(&row.cost.requests) {
            let day = days.entry(local_day(usage.at)).or_default();
            day.0 += price.unwrap_or_default();
            if day.1.last() != Some(&n) {
                day.1.push(n);
            }
        }
    }
    days.into_iter()
        .map(|(day, (spend, rows))| DaySpend { day, spend, rows })
        .collect()
}

/// A day's bar: 30 blocks for the largest day, `top`.
pub(crate) fn bar(spend: Decimal, top: Decimal) -> String {
    let width = if top.is_zero() {
        0
    } else {
        (spend * Decimal::from(30) / top)
            .round()
            .to_usize()
            .unwrap_or(0)
    };
    "█".repeat(width)
}

/// The chats the user started, by row, each with the spend of the sessions it started, largest
/// first. A subagent whose chat was not loaded is its own root, but it is not a conversation. A chat
/// that, with its subagents, used no tokens is left out.
pub(crate) fn chats(rows: &[Row]) -> Vec<(usize, Decimal)> {
    let mut found: HashMap<usize, (Decimal, u64)> = HashMap::new();
    for row in rows {
        let entry = found.entry(row.root).or_default();
        entry.0 += row.cost.spend;
        entry.1 += row.cost.tokens.total();
    }
    let mut chats: Vec<(usize, Decimal)> = found
        .into_iter()
        .filter(|&(root, (_, tokens))| rows[root].session.kind == Kind::Primary && tokens > 0)
        .map(|(root, (spend, _))| (root, spend))
        .collect();
    chats.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    chats
}

impl Report<'_> {
    /// Render the report as text.
    pub fn render(&self) -> String {
        let mut all = Totals::default();
        for row in &self.rows {
            all.add(row.session, &row.cost);
        }
        let p = self.paint;
        let mut out = String::new();
        self.header(&mut out, &all);
        self.totals(&mut out, &all);

        if let Some(taxonomy) = self.taxonomy {
            let work = groups(&self.rows, |row| {
                category(row, &taxonomy.work, |label| &label.work)
            });
            grouped(&mut out, p, "By type of work", &work, &all, None);
            let topics = groups(&self.rows, |row| {
                category(row, &taxonomy.topics, |label| &label.topic)
            });
            grouped(&mut out, p, "By topic", &topics, &all, None);
        }

        self.models(&mut out, &all);
        let projects = groups(&self.rows, |row| (row.place.project.clone(), String::new()));
        grouped(&mut out, p, "By project", &projects, &all, self.top);
        let repos = groups(&self.rows, repo);
        grouped(&mut out, p, "By repo", &repos, &all, self.top);
        self.days(&mut out);
        self.top(&mut out);

        if self.pull_requests {
            self.prs(&mut out);
        }
        self.footer(&mut out);
        out
    }

    /// The first line: the period and how many sessions it covers.
    fn header(&self, out: &mut String, all: &Totals) {
        let p = self.paint;
        let dot = p.dot();
        let reviews = if all.reviews > 0 {
            format!("{dot}{} reviews", all.reviews)
        } else {
            String::new()
        };
        writeln!(
            out,
            "{}{dot}{}{dot}{} conversations{dot}{} subagents{reviews}\n",
            p.tone(Tone::Heading, "atlas-nano"),
            p.bold(self.period),
            all.conversations,
            all.subagents
        )
        .unwrap();
    }

    /// The spend, time, tokens, and counts of all the sessions together.
    fn totals(&self, out: &mut String, all: &Totals) {
        let p = self.paint;
        let dot = p.dot();
        let Tokens {
            input,
            cached,
            cache_write,
            output,
        } = all.tokens;
        let note = |text: String| format!(" {}", p.tone(Tone::Warning, &text));
        let unknown = match all.unknown_turns {
            0 => String::new(),
            turns => note(format!("({turns} turns with no recorded completion)")),
        };
        let unpriced = match all.unpriced_requests {
            0 => String::new(),
            requests => note(format!(
                "({requests} requests have no price and count as $0)"
            )),
        };
        // Labels are dim so the values stand out.
        let (d, b) = (|text: &str| p.dim(text), |text: String| p.bold(&text));
        writeln!(out, "{}", p.tone(Tone::Heading, "Totals")).unwrap();
        writeln!(
            out,
            "  {}  {}{unpriced}",
            d("Spend (API-equivalent)"),
            p.styled(Tone::Money, true, &money(all.spend))
        )
        .unwrap();
        writeln!(
            out,
            "  {}    {} {}{unknown}{dot}{} {}",
            d("Time"),
            d("working"),
            b(span(all.working)),
            d("wall"),
            b(span(all.wall))
        )
        .unwrap();
        writeln!(
            out,
            "  {}  {} {}{dot}{} {}{dot}{} {}{dot}{} {}",
            d("Tokens"),
            d("in"),
            b(count(input)),
            d("cached"),
            b(count(cached)),
            d("cache-write"),
            b(count(cache_write)),
            d("out"),
            b(count(output))
        )
        .unwrap();
        writeln!(
            out,
            "  {} {}{dot}{} {}{dot}{} {}\n",
            d("Conversations"),
            b(all.conversations.to_string()),
            d("subagents"),
            b(all.subagents.to_string()),
            d("turns"),
            b(all.turns.to_string())
        )
        .unwrap();
    }

    /// Requests, tokens, and spend per client and model.
    fn models(&self, out: &mut String, all: &Totals) {
        let p = self.paint;
        let mut listed = models(&self.rows);
        for model in &mut listed {
            model.name = format!("  {}", model.name);
        }
        let shown = shown(listed.len(), self.top);
        let rest = listed.split_off(shown);
        if !rest.is_empty() {
            let mut sum = ModelUse {
                name: others(rest.len()),
                ..ModelUse::default()
            };
            for model in &rest {
                sum.requests += model.requests;
                sum.tokens += model.tokens;
                sum.spend += model.spend;
                sum.unpriced += model.unpriced;
            }
            listed.push(sum);
        }
        let rows: Vec<Vec<String>> = listed
            .into_iter()
            .map(|model| {
                let spend = if model.unpriced == model.requests {
                    p.tone(Tone::Warning, "no price")
                } else {
                    p.tone(Tone::Money, &money(model.spend))
                };
                vec![
                    p.tone(Tone::Plain, &model.name),
                    p.tone(Tone::Plain, &model.requests.to_string()),
                    p.tone(Tone::Plain, &count(model.tokens)),
                    spend,
                    p.dim(&share(model.spend, all.spend)),
                ]
            })
            .collect();
        let title = ranked("By client / model", self.top);
        table(
            out,
            p,
            &[&title, "Requests", "Tokens", "Spend", "Spend%"],
            &rows,
        );
        out.push('\n');
    }

    /// Spend per local day of the request, with a bar.
    fn days(&self, out: &mut String) {
        let days = days(&self.rows);
        let p = self.paint;
        let top = days.iter().map(|day| day.spend).max().unwrap_or_default();
        writeln!(out, "{}", p.tone(Tone::Heading, "By day")).unwrap();
        for DaySpend { day, spend, .. } in &days {
            writeln!(
                out,
                "  {day}  {}  {}",
                p.tone(Tone::Money, &format!("{:>10}", money(*spend))),
                p.tone(Tone::Accent, &bar(*spend, top))
            )
            .unwrap();
        }
        out.push('\n');
    }

    /// The chats that cost the most, each with the sessions it started.
    fn top(&self, out: &mut String) {
        let chats = chats(&self.rows);
        let p = self.paint;
        let heading = match self.top {
            Some(n) => format!("Top {n} conversations by spend"),
            None => "Conversations by spend".to_string(),
        };
        writeln!(
            out,
            "{}{}",
            p.tone(Tone::Heading, &heading),
            p.dim(", subagents included")
        )
        .unwrap();
        let shown = shown(chats.len(), self.top);
        let mut rows: Vec<Vec<String>> = chats[..shown]
            .iter()
            .map(|&(root, spend)| {
                let session = self.rows[root].session;
                let day = session.started.map_or(String::new(), local_day);
                vec![
                    p.tone(Tone::Plain, &format!("  {}", title(session))),
                    p.dim(&day),
                    p.tone(Tone::Plain, &self.rows[root].place.project),
                    p.tone(Tone::Money, &money(spend)),
                ]
            })
            .collect();
        let rest = &chats[shown..];
        if !rest.is_empty() {
            let spend: Decimal = rest.iter().map(|chat| chat.1).sum();
            rows.push(vec![
                p.tone(Tone::Plain, &others(rest.len())),
                String::new(),
                String::new(),
                p.tone(Tone::Money, &money(spend)),
            ]);
        }
        table(
            out,
            p,
            &["  Conversation", "Started", "Project", "Spend"],
            &rows,
        );
        out.push('\n');
    }

    /// The pull requests that cost the most.
    fn prs(&self, out: &mut String) {
        let p = self.paint;
        let listed = pull_requests(&self.rows);
        let title = match self.top {
            Some(n) => format!("Top {n} pull requests by spend"),
            None => "Pull requests by spend".to_string(),
        };
        writeln!(
            out,
            "{} {}",
            p.tone(Tone::Heading, &title),
            p.dim("(~ marks an amount split across several)")
        )
        .unwrap();
        let shown = shown(listed.len(), self.top);
        let line = |name: &str, sessions: String, spend: Decimal, split: bool| {
            let mark = if split { "~" } else { "" };
            vec![
                p.tone(Tone::Plain, name),
                p.tone(Tone::Plain, &sessions),
                p.tone(Tone::Money, &format!("{mark}{}", money(spend))),
            ]
        };
        let mut rows: Vec<Vec<String>> = listed[..shown]
            .iter()
            .map(|pr| {
                let name = format!("  {}", pr.url);
                line(&name, pr.rows.len().to_string(), pr.spend, pr.split)
            })
            .collect();
        // A session can work on several of the rest, so their sessions are not added up.
        let rest = &listed[shown..];
        if !rest.is_empty() {
            let spend = rest.iter().map(|pr| pr.spend).sum();
            let split = rest.iter().any(|pr| pr.split);
            rows.push(line(&others(rest.len()), String::new(), spend, split));
        }
        table(out, p, &["  Pull request", "Sessions", "Spend"], &rows);
        out.push('\n');
    }

    /// The labeling run's own details and the date of the prices close the report, dimmed.
    fn footer(&self, out: &mut String) {
        let p = self.paint;
        if let Some(labeling) = &self.labeling {
            let run = format!(
                "Labeling run: {} labeled, {} from cache, {} unlabeled · {}",
                labeling.labeled, labeling.from_cache, labeling.unlabeled, labeling.model
            );
            writeln!(out, "{}", p.dim(&run)).unwrap();
            let cost = labeling
                .cost
                .map_or("no price for this model".to_string(), |cost| {
                    format!("API-equivalent {}", money(cost))
                });
            let tokens = format!(
                "  in {} · out {} tokens · {cost}",
                count(labeling.input_tokens),
                count(labeling.output_tokens)
            );
            writeln!(out, "{}", p.dim(&tokens)).unwrap();
        }
        let checked = format!("Prices checked {}", self.prices.checked);
        writeln!(out, "{}", p.dim(&checked)).unwrap();
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
                    let prs = label
                        .prs
                        .iter()
                        .map(|pr| format!("{}={}", pr.role.name(), pr.url));
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
pub(crate) fn local_day(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local).format("%Y-%m-%d").to_string()
}

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
    fn colors_keep_the_table_aligned() {
        let render = |paint: Paint| {
            let all = Totals {
                spend: Decimal::from(12),
                conversations: 3,
                ..Totals::default()
            };
            let rows = vec![
                cells(paint, true, "  Software", &all, &all),
                cells(paint, false, "    Writing code", &Totals::default(), &all),
            ];
            let mut out = String::new();
            table(
                &mut out,
                paint,
                &["By type of work", "Convs", "Spend"],
                &rows,
            );
            out
        };
        for theme in Theme::ALL {
            let colored = render(Paint::colored(theme));
            assert_ne!(colored, render(Paint::OFF));
            assert_eq!(paint::strip(&colored), render(Paint::OFF));
        }
    }

    #[test]
    fn a_short_form_never_rounds_past_its_unit() {
        let counts = [
            (999, "999"),
            (999_499, "999k"),
            (999_999, "1.0M"),
            (9_949_999, "9.9M"),
            (9_999_999, "10M"),
            (999_499_999, "999M"),
            (999_999_999, "1.0B"),
        ];
        for (tokens, shown) in counts {
            assert_eq!(count(tokens), shown);
        }
        let spans = [
            (59, "59s"),
            (3599, "59m"),
            (35_819, "9.9h"),
            (35_999, "10h"),
        ];
        for (seconds, shown) in spans {
            assert_eq!(span(Duration::seconds(seconds)), shown);
        }
    }

    #[test]
    fn puts_the_groups_past_the_limit_in_one_row() {
        let group = |name: &str, cents: i64| Group {
            line: Line {
                name: name.to_string(),
                rows: Vec::new(),
                totals: Totals {
                    conversations: 1,
                    spend: Decimal::new(cents, 2),
                    ..Totals::default()
                },
            },
            members: Vec::new(),
        };
        let groups: Vec<Group> = [("a", 400), ("b", 300), ("c", 200), ("d", 100)]
            .into_iter()
            .map(|(name, cents)| group(name, cents))
            .collect();
        let mut all = Totals::default();
        for group in &groups {
            all += &group.line.totals;
        }
        let mut out = String::new();
        grouped(&mut out, Paint::OFF, "By project", &groups, &all, Some(2));
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("By project (top 2)"), "{out}");
        assert!(
            lines[1].starts_with("  a ") && lines[2].starts_with("  b "),
            "{out}"
        );
        assert!(lines[3].starts_with("  (2 others)"), "{out}");
        assert!(
            lines[3].contains("$3.00") && lines[3].contains("30%"),
            "{out}"
        );
        assert_eq!(lines[4], "", "{out}");

        let mut out = String::new();
        grouped(&mut out, Paint::OFF, "By project", &groups, &all, None);
        assert!(out.starts_with("By project  "), "{out}");
        assert!(out.contains("  d ") && !out.contains("others"), "{out}");
    }

    #[test]
    fn leaves_out_lines_with_no_conversations_and_no_tokens() {
        use crate::account::{cost, tests::session};

        let idle = |mut session: Session| {
            session.usage.clear();
            session
        };
        let sessions = [
            (
                "Software",
                "Writing code",
                session("a", Kind::Primary, None, 1),
            ),
            (
                "Software",
                "Learning",
                idle(session("b", Kind::Primary, None, 1)),
            ),
            (
                "Software",
                "Testing",
                session("c", Kind::Subagent, Some("a"), 1),
            ),
            (
                "Software",
                "Brainstorming",
                idle(session("d", Kind::Subagent, Some("a"), 1)),
            ),
            (
                "Design",
                "Naming",
                idle(session("e", Kind::Subagent, Some("a"), 1)),
            ),
        ];
        let prices = PriceTable::built_in();
        let place = Place {
            project: "app".to_string(),
            repo: "app".to_string(),
        };
        let rows: Vec<Row> = sessions
            .iter()
            .map(|(_, _, session)| Row {
                session,
                cost: cost(session, &prices),
                label: None,
                root: 0,
                place: &place,
            })
            .collect();
        let groups = groups(&rows, |row| {
            let at = rows
                .iter()
                .position(|r| r.session.id == row.session.id)
                .unwrap();
            (sessions[at].0.to_string(), sessions[at].1.to_string())
        });
        let names: Vec<(&str, Vec<&str>)> = groups
            .iter()
            .map(|g| {
                (
                    g.line.name.as_str(),
                    g.members.iter().map(|m| m.name.as_str()).collect(),
                )
            })
            .collect();
        // Learning has a conversation and Testing has tokens; the others have neither.
        assert_eq!(names.len(), 1, "{:?}", names);
        assert_eq!(names[0].0, "Software");
        let mut members = names[0].1.clone();
        members.sort_unstable();
        assert_eq!(members, ["Learning", "Testing", "Writing code"]);
        // The group's own line still holds every row, so its sums match its members'.
        assert_eq!(groups[0].line.rows.len(), 4);
    }

    #[test]
    fn rounds_halves_up() {
        assert_eq!(money(Decimal::new(125, 3)), "$0.13");
        assert_eq!(money(Decimal::new(1_234_565, 3)), "$1,234.57");
        assert_eq!(share(Decimal::new(25, 1), Decimal::from(100)), "3%");
    }

    #[test]
    fn leaves_out_models_chats_and_pull_requests_with_no_tokens() {
        use crate::account::{cost, tests::session};
        use crate::model::{PrRole, PullRequest};

        let mut used = session("used", Kind::Primary, None, 1);
        used.title = Some("Busy chat".to_string());
        let mut idle = session("idle", Kind::Primary, None, 3);
        idle.title = Some("Idle chat".to_string());
        idle.usage.truncate(1);
        idle.usage[0].model = "idle-model".to_string();
        idle.usage[0].input = 0;
        idle.usage[0].output = 0;
        let label = |url: &str| Label {
            work: "Writing code".to_string(),
            topic: "Backend and APIs".to_string(),
            prs: vec![PullRequest {
                url: url.to_string(),
                role: PrRole::ContributedTo,
            }],
            model: "test".to_string(),
            from_cache: false,
        };
        let (busy_pr, idle_pr) = (label("https://pr/busy"), label("https://pr/idle"));
        let prices = PriceTable::built_in();
        let place = Place {
            project: "app".to_string(),
            repo: "app".to_string(),
        };
        let rows = [(&used, &busy_pr), (&idle, &idle_pr)]
            .into_iter()
            .enumerate()
            .map(|(root, (session, label))| Row {
                session,
                cost: cost(session, &prices),
                label: Some(label),
                root,
                place: &place,
            })
            .collect();
        let report = Report {
            period: "test",
            rows,
            taxonomy: None,
            labeling: None,
            pull_requests: true,
            top: Some(10),
            prices: &prices,
            paint: Paint::OFF,
        };
        let out = report.render();
        for kept in ["claude-haiku-5-5", "Busy chat", "https://pr/busy"] {
            assert!(out.contains(kept), "{kept} missing:\n{out}");
        }
        for dropped in ["idle-model", "Idle chat", "https://pr/idle"] {
            assert!(!out.contains(dropped), "{dropped} shown:\n{out}");
        }
    }

    #[test]
    fn shows_zero_only_for_none() {
        let whole = Decimal::from(1000);
        assert_eq!(share(Decimal::ZERO, whole), "0%");
        assert_eq!(share(Decimal::new(1, 2), whole), "<1%");
        assert_eq!(share(Decimal::from(4), whole), "<1%");
        assert_eq!(share(Decimal::from(5), whole), "1%");
        assert_eq!(share(Decimal::ONE, Decimal::ZERO), "-");
    }
}
