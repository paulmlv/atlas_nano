//! `atlas-nano run --tui`: the report as an interactive view. Each table is a tab; a line opens the
//! conversations it sums, and a conversation opens its sessions.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Cell, Paragraph, Row as TableRow, Table as TableWidget, TableState, Tabs,
};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use crate::account::Totals;
use crate::model::{Kind, Label, Usage};
use crate::paint::{Paint, Tone, printable};
use crate::report::{
    self, COLUMNS, NO_REPO, Report, Row, category, count, local_day, money, share, span,
};

/// Which part of a session's requests a line holds.
#[derive(Debug, Clone, PartialEq)]
enum Slice {
    All,
    /// The requests to one model, named `client / model`.
    Model(String),
    /// The requests on one local day.
    Day(String),
    /// The session's even share of one pull request, split among those it worked on.
    PullRequest(String),
}

/// A row's tokens and spend within `slice`.
fn part(row: &Row, slice: &Slice) -> (u64, Decimal) {
    let requests = |keep: &dyn Fn(&Usage) -> bool| {
        let kept = row.session.usage.iter().zip(&row.cost.requests);
        kept.filter(|(usage, _)| keep(usage)).fold(
            (0, Decimal::ZERO),
            |(tokens, spend), (usage, price)| {
                (tokens + usage.tokens(), spend + price.unwrap_or_default())
            },
        )
    };
    match slice {
        Slice::All => (row.cost.tokens.total(), row.cost.spend),
        Slice::Model(name) => {
            requests(&|usage| format!("{} / {}", row.session.client.name(), usage.model) == *name)
        }
        Slice::Day(day) => requests(&|usage| local_day(usage.at) == *day),
        Slice::PullRequest(url) => {
            let worked: Vec<_> = row
                .label
                .iter()
                .flat_map(|label| &label.prs)
                .filter(|pr| pr.role.worked_on())
                .collect();
            if worked.iter().any(|pr| pr.url == *url) {
                let n = worked.len() as u64;
                (
                    row.cost.tokens.total() / n,
                    row.cost.spend / Decimal::from(n),
                )
            } else {
                (0, Decimal::ZERO)
            }
        }
    }
}

/// What a line opens.
#[derive(Debug, Clone, PartialEq)]
enum Open {
    /// The conversations of these rows of the report, under a title, counting only `Slice`.
    Chats(String, Vec<usize>, Slice),
    /// One conversation's sessions, by the row of the chat the user started.
    Chat(usize),
}

/// One line of a table: its cells, whether it heads a group, and what it opens.
struct Item {
    cells: Vec<String>,
    strong: bool,
    open: Option<Open>,
}

/// One screen: a table under a title, a note above it, and the selected line.
struct Table {
    title: String,
    note: Vec<String>,
    header: Vec<String>,
    items: Vec<Item>,
    state: TableState,
}

impl Table {
    fn new(title: &str, header: &[&str], items: Vec<Item>) -> Table {
        let selected = (!items.is_empty()).then_some(0);
        Table {
            title: title.to_string(),
            note: Vec::new(),
            header: header.iter().map(|cell| cell.to_string()).collect(),
            items,
            state: TableState::default().with_selected(selected),
        }
    }

    /// Move the selection by `delta` lines, staying on the table.
    fn step(&mut self, delta: isize) {
        if let Some(last) = self.items.len().checked_sub(1) {
            let at = self.state.selected().unwrap_or(0) as isize + delta;
            self.state.select(Some(at.clamp(0, last as isize) as usize));
        }
    }

    fn opens(&self) -> Option<&Open> {
        self.items.get(self.state.selected()?)?.open.as_ref()
    }

    /// Move each of `columns` whose cells all hold one value into the note, so titles get the
    /// room. A column holding only `-`, nothing known, is left out.
    fn fold(&mut self, columns: &[usize]) {
        let mut moved = Vec::new();
        for &column in columns.iter().rev() {
            let Some(first) = self.items.first().map(|item| item.cells[column].clone()) else {
                return;
            };
            if self.items.iter().any(|item| item.cells[column] != first) {
                continue;
            }
            if first != "-" {
                moved.push(format!("{} {first}", self.header[column]));
            }
            self.header.remove(column);
            for item in &mut self.items {
                item.cells.remove(column);
            }
        }
        if !moved.is_empty() {
            moved.reverse();
            self.note.push(moved.join(" · "));
        }
    }
}

/// Rows of the report by group and member name.
type Groups = BTreeMap<String, BTreeMap<String, Vec<usize>>>;

/// A name, the rows it holds, and their sums.
type Bucket = (String, Vec<usize>, Totals);

fn by_spend(a: &Bucket, b: &Bucket) -> Ordering {
    b.2.spend.cmp(&a.2.spend).then(a.0.cmp(&b.0))
}

fn sum(report: &Report, rows: &[usize]) -> Totals {
    let mut totals = Totals::default();
    for &n in rows {
        totals.add(report.rows[n].session, &report.rows[n].cost);
    }
    totals
}

fn groups(report: &Report, key: impl Fn(&Row) -> (String, String)) -> Groups {
    let mut groups = Groups::new();
    for (n, row) in report.rows.iter().enumerate() {
        let (group, name) = key(row);
        groups
            .entry(group)
            .or_default()
            .entry(name)
            .or_default()
            .push(n);
    }
    groups
}

/// A table of groups, largest spend first, each followed by its members when it has names.
fn grouped(report: &Report, all: &Totals, title: &str, groups: Groups) -> Table {
    let mut ordered: Vec<(Bucket, Vec<Bucket>)> = groups
        .into_iter()
        .map(|(group, members)| {
            let mut members: Vec<Bucket> = members
                .into_iter()
                .map(|(name, rows)| {
                    let totals = sum(report, &rows);
                    (name, rows, totals)
                })
                .collect();
            members.sort_by(by_spend);
            let rows: Vec<usize> = members.iter().flat_map(|m| m.1.iter().copied()).collect();
            let totals = sum(report, &rows);
            ((group, rows, totals), members)
        })
        .collect();
    ordered.sort_by(|a, b| by_spend(&a.0, &b.0));
    let flat = |members: &[Bucket]| members.len() == 1 && members[0].0.is_empty();
    // Groups are bold only when some have members under them.
    let nested = ordered.iter().any(|(_, members)| !flat(members));
    let line =
        |name: &str, indent: &str, strong: bool, (rows, totals): (Vec<usize>, Totals)| Item {
            cells: vec![
                format!("{indent}{name}"),
                totals.conversations.to_string(),
                share(totals.conversations.into(), all.conversations.into()),
                (totals.subagents + totals.reviews).to_string(),
                totals.turns.to_string(),
                span(totals.working),
                span(totals.wall),
                count(totals.tokens.total()),
                money(totals.spend),
                share(totals.spend, all.spend),
            ],
            strong,
            open: Some(Open::Chats(name.to_string(), rows, Slice::All)),
        };
    let mut items = Vec::new();
    for ((group, rows, totals), members) in ordered {
        let is_flat = flat(&members);
        items.push(line(&group, "", nested, (rows, totals)));
        if !is_flat {
            for (name, rows, totals) in members {
                items.push(line(&name, "  ", false, (rows, totals)));
            }
        }
    }
    let mut header = vec![title];
    header.extend(COLUMNS);
    Table::new(title, &header, items)
}

/// Requests, tokens, and spend per client and model.
fn models(report: &Report, all: &Totals) -> Table {
    let mut models: BTreeMap<String, (u64, u64, Decimal, u64, Vec<usize>)> = BTreeMap::new();
    for (n, row) in report.rows.iter().enumerate() {
        for (usage, price) in row.session.usage.iter().zip(&row.cost.requests) {
            let name = format!("{} / {}", row.session.client.name(), usage.model);
            let entry = models.entry(name).or_default();
            entry.0 += 1;
            entry.1 += usage.tokens();
            match price {
                Some(cost) => entry.2 += cost,
                None => entry.3 += 1,
            }
            if entry.4.last() != Some(&n) {
                entry.4.push(n);
            }
        }
    }
    let mut listed: Vec<_> = models.into_iter().collect();
    listed.sort_by(|a, b| b.1.2.cmp(&a.1.2).then(a.0.cmp(&b.0)));
    let items = listed
        .into_iter()
        .map(|(name, (requests, tokens, spend, unpriced, rows))| Item {
            cells: vec![
                name.clone(),
                requests.to_string(),
                count(tokens),
                if unpriced == requests {
                    "no price".to_string()
                } else {
                    money(spend)
                },
                share(spend, all.spend),
            ],
            strong: false,
            open: Some(Open::Chats(name.clone(), rows, Slice::Model(name))),
        })
        .collect();
    Table::new(
        "Models",
        &["By client / model", "Requests", "Tokens", "Spend", "Spend%"],
        items,
    )
}

/// Spend per local day of the request, with a bar.
fn days(report: &Report) -> Table {
    let mut days: BTreeMap<String, (Decimal, Vec<usize>)> = BTreeMap::new();
    for (n, row) in report.rows.iter().enumerate() {
        for (usage, price) in row.session.usage.iter().zip(&row.cost.requests) {
            let entry = days.entry(local_day(usage.at)).or_default();
            entry.0 += price.unwrap_or_default();
            if entry.1.last() != Some(&n) {
                entry.1.push(n);
            }
        }
    }
    let top = days.values().map(|day| day.0).max().unwrap_or_default();
    let items = days
        .into_iter()
        .map(|(day, (spend, rows))| {
            let width = if top.is_zero() {
                0
            } else {
                (spend * Decimal::from(30) / top)
                    .round()
                    .to_usize()
                    .unwrap_or(0)
            };
            Item {
                cells: vec![day.clone(), money(spend), "█".repeat(width)],
                strong: false,
                open: Some(Open::Chats(day.clone(), rows, Slice::Day(day))),
            }
        })
        .collect();
    Table::new("Days", &["By day", "Spend", ""], items)
}

/// The pull requests the conversations worked on.
fn pull_requests(report: &Report) -> Table {
    let items = report::pull_requests(&report.rows)
        .into_iter()
        .map(|pr| {
            let mark = if pr.split { "~" } else { "" };
            Item {
                cells: vec![
                    pr.url.clone(),
                    pr.rows.len().to_string(),
                    format!("{mark}{}", money(pr.spend)),
                ],
                strong: false,
                open: Some(Open::Chats(
                    pr.url.clone(),
                    pr.rows,
                    Slice::PullRequest(pr.url),
                )),
            }
        })
        .collect();
    let mut table = Table::new(
        "Pull requests",
        &["Pull request", "Sessions", "Spend"],
        items,
    );
    table.note = vec!["~ marks an amount split across several pull requests".to_string()];
    table
}

/// The conversations that `rows` belong to, largest spend first, counting only those rows' `slice`.
fn chats(report: &Report, title: &str, rows: &[usize], slice: &Slice) -> Table {
    let mut by_root: HashMap<usize, (u64, u64, Decimal)> = HashMap::new();
    for &n in rows {
        let (tokens, spend) = part(&report.rows[n], slice);
        let chat = by_root.entry(report.rows[n].root).or_default();
        chat.0 += 1;
        chat.1 += tokens;
        chat.2 += spend;
    }
    let mut listed: Vec<(usize, (u64, u64, Decimal))> = by_root.into_iter().collect();
    listed.sort_by(|a, b| b.1.2.cmp(&a.1.2).then(a.0.cmp(&b.0)));
    let whole: Decimal = listed.iter().map(|chat| chat.1.2).sum();
    let items = listed
        .into_iter()
        .map(|(root, (sessions, tokens, spend))| {
            let row = &report.rows[root];
            let name = report::title(row.session);
            // A subagent whose chat was not loaded is its own root.
            let name = match row.session.kind {
                Kind::Primary => name,
                Kind::Subagent => format!("(subagent) {name}"),
                Kind::Review => format!("(review) {name}"),
            };
            Item {
                cells: vec![
                    name,
                    row.session.started.map_or(String::new(), local_day),
                    row.place.project.clone(),
                    row.label.map_or("-", |label| &label.work).to_string(),
                    sessions.to_string(),
                    count(tokens),
                    money(spend),
                    share(spend, whole),
                ],
                strong: false,
                open: Some(Open::Chat(root)),
            }
        })
        .collect();
    let mut table = Table::new(
        title,
        &[
            "Conversation",
            "Started",
            "Project",
            "Type of work",
            "Sessions",
            "Tokens",
            "Spend",
            "Spend%",
        ],
        items,
    );
    table.fold(&[2, 3]);
    table
}

/// One conversation: the chat the user started, then the sessions it started, largest spend first.
fn chat(report: &Report, root: usize) -> Table {
    let mut rows: Vec<usize> = (0..report.rows.len())
        .filter(|&n| report.rows[n].root == root)
        .collect();
    let spend = |n: usize| report.rows[n].cost.spend;
    rows.sort_by(|&a, &b| {
        (b == root)
            .cmp(&(a == root))
            .then(spend(b).cmp(&spend(a)))
            .then(a.cmp(&b))
    });
    let first = &report.rows[root];
    let repo = if first.place.repo.is_empty() {
        NO_REPO
    } else {
        &first.place.repo
    };
    let started = first.session.started.map_or(String::new(), |at| {
        at.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    });
    let mut note = vec![format!(
        "{} · project {} · repo {repo} · started {started}",
        first.session.client.name(),
        first.place.project
    )];
    let prs = rows
        .iter()
        .flat_map(|&n| report.rows[n].label.iter().flat_map(|label| &label.prs));
    for pr in prs {
        note.push(format!("{} {}", pr.role.name(), pr.url));
    }
    let items = rows
        .iter()
        .map(|&n| {
            let row = &report.rows[n];
            let session = row.session;
            let kind = match session.kind {
                Kind::Primary => "chat",
                Kind::Subagent => "subagent",
                Kind::Review => "review",
            };
            // Placeholders like Claude Code's `<synthetic>` record no tokens.
            let mut models: Vec<&str> = session
                .usage
                .iter()
                .filter(|usage| usage.tokens() > 0)
                .map(|usage| usage.model.as_str())
                .collect();
            models.sort_unstable();
            models.dedup();
            let label = |pick: fn(&Label) -> &str| row.label.map_or("-", pick).to_string();
            Item {
                cells: vec![
                    format!("{kind}  {}", report::title(session)),
                    label(|label| &label.work),
                    label(|label| &label.topic),
                    models.join(", "),
                    session.turns.len().to_string(),
                    span(session.working()),
                    count(row.cost.tokens.total()),
                    money(row.cost.spend),
                ],
                strong: n == root,
                open: None,
            }
        })
        .collect();
    let mut table = Table::new(
        &report::title(first.session),
        &[
            "Session",
            "Type of work",
            "Topic",
            "Models",
            "Turns",
            "Working",
            "Tokens",
            "Spend",
        ],
        items,
    );
    table.note = note;
    table.fold(&[1, 2, 3]);
    table
}

#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Quit,
}

/// The view's state: the tabs, the one shown, and the screens opened from it.
struct Dashboard<'r, 'a> {
    report: &'r Report<'a>,
    all: Totals,
    tabs: Vec<(&'static str, Table)>,
    tab: usize,
    /// The screens opened from the tab, the last one shown.
    stack: Vec<Table>,
}

impl<'r, 'a> Dashboard<'r, 'a> {
    fn new(report: &'r Report<'a>) -> Self {
        let all = sum(report, &(0..report.rows.len()).collect::<Vec<_>>());
        let mut tabs = Vec::new();
        if let Some(taxonomy) = report.taxonomy {
            let work = groups(report, |row| category(row, &taxonomy.work, |l| &l.work));
            tabs.push(("Work", grouped(report, &all, "By type of work", work)));
            let topics = groups(report, |row| category(row, &taxonomy.topics, |l| &l.topic));
            tabs.push(("Topic", grouped(report, &all, "By topic", topics)));
        }
        tabs.push(("Models", models(report, &all)));
        let projects = groups(report, |row| (row.place.project.clone(), String::new()));
        tabs.push(("Projects", grouped(report, &all, "By project", projects)));
        let repos = groups(report, |row| {
            let repo = &row.place.repo;
            let repo = if repo.is_empty() { NO_REPO } else { repo };
            (repo.to_string(), String::new())
        });
        tabs.push(("Repos", grouped(report, &all, "By repo", repos)));
        tabs.push(("Days", days(report)));
        let every: Vec<usize> = (0..report.rows.len()).collect();
        tabs.push((
            "Conversations",
            chats(report, "Conversations", &every, &Slice::All),
        ));
        if report.pull_requests {
            tabs.push(("PRs", pull_requests(report)));
        }
        Dashboard {
            report,
            all,
            tabs,
            tab: 0,
            stack: Vec::new(),
        }
    }

    fn screen(&mut self) -> &mut Table {
        match self.stack.last_mut() {
            Some(table) => table,
            None => &mut self.tabs[self.tab].1,
        }
    }

    fn switch(&mut self, tab: usize) {
        self.tab = tab % self.tabs.len();
        self.stack.clear();
    }

    fn key(&mut self, key: KeyEvent) -> Flow {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => return Flow::Quit,
            KeyCode::Char('c') if control => return Flow::Quit,
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => self.switch(self.tab + 1),
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.switch(self.tab + self.tabs.len() - 1)
            }
            KeyCode::Char(digit @ '1'..='9') => {
                let n = digit as usize - '1' as usize;
                if n < self.tabs.len() {
                    self.switch(n);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.screen().step(1),
            KeyCode::Up | KeyCode::Char('k') => self.screen().step(-1),
            KeyCode::PageDown => self.screen().step(10),
            KeyCode::PageUp => self.screen().step(-10),
            KeyCode::Home | KeyCode::Char('g') => self.screen().step(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.screen().step(isize::MAX / 2),
            KeyCode::Enter => {
                let next = match self.screen().opens().cloned() {
                    Some(Open::Chats(title, rows, slice)) => {
                        chats(self.report, &title, &rows, &slice)
                    }
                    Some(Open::Chat(root)) => chat(self.report, root),
                    None => return Flow::Continue,
                };
                self.stack.push(next);
            }
            KeyCode::Esc | KeyCode::Backspace => {
                self.stack.pop();
            }
            _ => {}
        }
        Flow::Continue
    }

    fn draw(&mut self, frame: &mut Frame) {
        let p = self.report.paint;
        let [head, tabs, body, foot] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        let all = &self.all;
        let reviews = if all.reviews > 0 {
            format!(" · {} reviews", all.reviews)
        } else {
            String::new()
        };
        let tokens = all.tokens;
        let header = vec![
            Line::from(vec![
                Span::styled("atlas-nano", p.style(Tone::Heading, false)),
                format!(" · {} · ", self.report.period).into(),
                Span::styled(money(all.spend), p.style(Tone::Money, true)),
                format!(
                    " · {} conversations · {} subagents{reviews}",
                    all.conversations, all.subagents
                )
                .into(),
            ]),
            Line::styled(
                format!(
                    "working {} · wall {} · tokens in {} · cached {} · cache-write {} · out {}",
                    span(all.working),
                    span(all.wall),
                    count(tokens.input),
                    count(tokens.cached),
                    count(tokens.cache_write),
                    count(tokens.output)
                ),
                p.style(Tone::Dim, false),
            ),
        ];
        frame.render_widget(Paragraph::new(header), head);
        let names = self
            .tabs
            .iter()
            .enumerate()
            .map(|(n, (name, _))| format!("{} {name}", n + 1));
        frame.render_widget(
            Tabs::new(names)
                .select(self.tab)
                .style(p.style(Tone::Dim, false))
                .highlight_style(p.style(Tone::Heading, true).reversed()),
            tabs,
        );
        let mut trail: Vec<&str> = vec![self.tabs[self.tab].0];
        trail.extend(self.stack.iter().map(|table| table.title.as_str()));
        let title = Span::styled(printable(&trail.join(" › ")), p.style(Tone::Heading, false));
        let block = Block::bordered()
            .title(title)
            .border_style(p.style(Tone::Dim, false));
        let inner = block.inner(body);
        frame.render_widget(block, body);
        let back = if self.stack.is_empty() {
            ""
        } else {
            "  esc back"
        };
        let help = format!("←/→ tab  ↑/↓ move  enter open{back}  q quit");
        frame.render_widget(Paragraph::new(help).style(p.style(Tone::Dim, false)), foot);
        render(self.screen(), frame, inner, p);
    }
}

/// The tone of a cell, from its column and text.
fn tone(header: &str, text: &str) -> Tone {
    match header {
        _ if text == "no price" => Tone::Warning,
        _ if text.starts_with('█') => Tone::Accent,
        "Spend" => Tone::Money,
        "Conv%" | "Spend%" | "Started" => Tone::Dim,
        _ => Tone::Plain,
    }
}

/// Columns of words rather than numbers: left-aligned, and no wider than `TEXT_WIDTH`.
const TEXT: [&str; 5] = ["Started", "Project", "Type of work", "Topic", "Models"];
const TEXT_WIDTH: usize = 20;
/// The first column, which holds names and titles, is kept at least this wide.
const TITLE_WIDTH: usize = 16;
const SPACING: usize = 2;
const SYMBOL: &str = "▸ ";

/// `text` cut to `width` columns, ending in `…` when it was longer.
fn fit(text: &str, width: usize) -> String {
    let wide = |text: &str| Span::raw(text).width();
    if wide(text) <= width {
        return text.to_string();
    }
    let mut cut = String::new();
    for c in text.chars() {
        if wide(&cut) + wide(c.encode_utf8(&mut [0; 4])) >= width {
            break;
        }
        cut.push(c);
    }
    cut + "…"
}

/// Draw a table with its note above it. Numbers are right-aligned; the first column gets the room
/// the others leave, up to its widest cell.
fn render(table: &mut Table, frame: &mut Frame, area: Rect, p: Paint) {
    let [note, area] = Layout::vertical([
        Constraint::Length(table.note.len() as u16),
        Constraint::Fill(1),
    ])
    .areas(area);
    let lines: Vec<Line> = table
        .note
        .iter()
        .map(|text| Line::styled(printable(text), p.style(Tone::Dim, false)))
        .collect();
    frame.render_widget(Paragraph::new(lines), note);
    let mut widths: Vec<usize> = table
        .header
        .iter()
        .map(|cell| Span::raw(cell).width())
        .collect();
    for item in &table.items {
        for (width, cell) in widths.iter_mut().zip(&item.cells) {
            *width = (*width).max(Span::raw(printable(cell)).width());
        }
    }
    // The days table's bar is under an empty header, and grows left to right.
    let left = |n: usize| n == 0 || table.header[n].is_empty() || TEXT.contains(&&*table.header[n]);
    for (n, width) in widths.iter_mut().enumerate().skip(1) {
        if left(n) && !table.header[n].is_empty() {
            *width = (*width).min(TEXT_WIDTH);
        }
    }
    let taken = Span::raw(SYMBOL).width()
        + SPACING * (widths.len() - 1)
        + widths[1..].iter().sum::<usize>();
    let room = (area.width as usize).saturating_sub(taken);
    widths[0] = widths[0].min(room.max(TITLE_WIDTH));
    let cell = |n: usize, text: &str, style: Style| {
        let line = Line::styled(fit(&printable(text), widths[n]), style);
        Cell::from(line.alignment(if left(n) {
            Alignment::Left
        } else {
            Alignment::Right
        }))
    };
    let header = TableRow::new(
        table
            .header
            .iter()
            .enumerate()
            .map(|(n, text)| cell(n, text, p.style(Tone::Dim, false))),
    );
    let rows = table.items.iter().map(|item| {
        TableRow::new(item.cells.iter().enumerate().map(|(n, text)| {
            let style = p.style(tone(&table.header[n], text), item.strong);
            cell(n, text, style)
        }))
    });
    let widths = widths.iter().map(|&width| Constraint::Length(width as u16));
    let widget = TableWidget::new(rows, widths)
        .header(header)
        .column_spacing(SPACING as u16)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol(SYMBOL);
    frame.render_stateful_widget(widget, area, &mut table.state);
}

/// Show the report until the user quits, then give the terminal back as it was.
pub fn show(report: &Report) -> Result<(), String> {
    let mut dashboard = Dashboard::new(report);
    let mut terminal = ratatui::try_init().map_err(|error| error.to_string())?;
    let result = (|| {
        loop {
            terminal.draw(|frame| dashboard.draw(frame))?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && dashboard.key(key) == Flow::Quit
            {
                return Ok::<(), std::io::Error>(());
            }
        }
    })();
    ratatui::restore();
    result.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;

    use super::*;
    use crate::account::{self, tests::session};
    use crate::model::{Item as Part, PrRole, PullRequest, Role, Session};
    use crate::paint::Theme;
    use crate::place::Place;
    use crate::prices::PriceTable;
    use crate::taxonomy::Taxonomy;

    fn label(work: &str) -> Label {
        Label {
            work: work.to_string(),
            topic: "Backend and APIs".to_string(),
            prs: vec![PullRequest {
                url: "https://github.com/owner/repo/pull/7".to_string(),
                role: PrRole::ContributedTo,
            }],
            model: "claude-haiku-5-5".to_string(),
            from_cache: false,
        }
    }

    /// Draw `sessions`, each labeled "Writing code", on a screen `width` wide, after `keys`.
    fn draw(sessions: &[Session], keys: &[KeyCode], paint: Paint, width: u16) -> Buffer {
        let prices = PriceTable::built_in();
        let taxonomy = Taxonomy::built_in();
        let place = Place {
            project: "app".to_string(),
            repo: "app".to_string(),
        };
        let labels: Vec<Label> = sessions.iter().map(|_| label("Writing code")).collect();
        let roots = account::roots(sessions);
        let report = Report {
            period: "last 7 days",
            rows: sessions
                .iter()
                .enumerate()
                .map(|(n, session)| Row {
                    session,
                    cost: account::cost(session, &prices),
                    label: Some(&labels[n]),
                    root: roots[n],
                    place: &place,
                })
                .collect(),
            taxonomy: Some(&taxonomy),
            labeling: None,
            pull_requests: true,
            top: None,
            prices: &prices,
            paint,
        };
        let mut dashboard = Dashboard::new(&report);
        for &code in keys {
            dashboard.key(KeyEvent::from(code));
        }
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal.draw(|frame| dashboard.draw(frame)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn screen(sessions: &[Session], keys: &[KeyCode]) -> String {
        narrow(sessions, keys, 120)
    }

    fn narrow(sessions: &[Session], keys: &[KeyCode], width: u16) -> String {
        let buffer = draw(sessions, keys, Paint::OFF, width);
        let width = buffer.area.width as usize;
        let symbols: Vec<&str> = buffer.content().iter().map(|cell| cell.symbol()).collect();
        symbols
            .chunks(width)
            .map(|line| line.concat().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A chat, and a subagent it started that has no title, only the task it was given.
    fn sessions() -> Vec<Session> {
        let mut chat = session("chat", Kind::Primary, None, 1);
        chat.title = Some("Fix the export".to_string());
        let mut agent = session("agent", Kind::Subagent, Some("chat"), 2);
        agent.items = vec![Part {
            position: 0,
            role: Role::Agent,
            text: "Search the code\nfor the export".to_string(),
        }];
        vec![chat, agent]
    }

    #[test]
    fn opens_a_tab_on_the_first_table() {
        let shown = screen(&sessions(), &[]);
        assert!(shown.contains("1 Work"), "{shown}");
        assert!(shown.contains("8 PRs"), "{shown}");
        assert!(shown.contains("▸ Software"), "{shown}");
        assert!(shown.contains("  Writing code"), "{shown}");
    }

    #[test]
    fn opens_a_line_then_a_conversation_and_goes_back() {
        let shown = screen(&sessions(), &[KeyCode::Enter]);
        assert!(shown.contains("Work › Software"), "{shown}");
        assert!(shown.contains("▸ Fix the export"), "{shown}");
        // Every conversation here is in one project with one label, so those move above.
        assert!(
            shown.contains("Project app · Type of work Writing code"),
            "{shown}"
        );

        let shown = screen(&sessions(), &[KeyCode::Enter, KeyCode::Enter]);
        assert!(
            shown.contains("Work › Software › Fix the export"),
            "{shown}"
        );
        assert!(shown.contains("chat  Fix the export"), "{shown}");
        assert!(shown.contains("subagent  Search the code"), "{shown}");
        assert!(
            shown.contains("https://github.com/owner/repo/pull/7"),
            "{shown}"
        );

        let keys = [KeyCode::Enter, KeyCode::Enter, KeyCode::Esc, KeyCode::Esc];
        assert_eq!(screen(&sessions(), &keys), screen(&sessions(), &[]));
    }

    #[test]
    fn a_model_opens_only_its_requests() {
        // Each session has 8 tokens to an unpriced model; the rest of its 1.1M go to Haiku.
        let keys = [KeyCode::Char('3'), KeyCode::Down, KeyCode::Enter];
        let shown = screen(&sessions(), &keys);
        assert!(shown.contains("Models › claude / unpriced"), "{shown}");
        let line = shown.lines().find(|line| line.contains("▸")).unwrap();
        assert!(line.contains(" 16 ") && line.contains("$0.00"), "{line}");
    }

    #[test]
    fn switching_tabs_closes_what_was_open() {
        let shown = screen(&sessions(), &[KeyCode::Enter, KeyCode::Char('7')]);
        assert!(shown.contains("┌Conversations─"), "{shown}");
        assert!(shown.contains("▸ Fix the export"), "{shown}");
        let shown = screen(&sessions(), &[KeyCode::Left]);
        assert!(shown.contains("┌PRs─"), "{shown}");
    }

    #[test]
    fn shows_control_characters_as_data() {
        let mut sessions = sessions();
        sessions[0].title = Some("evil\u{1b}[31m".to_string());
        let shown = screen(&sessions, &[KeyCode::Char('7')]);
        assert!(!shown.contains('\u{1b}'), "{shown}");
        assert!(shown.contains("evil\u{fffd}[31m"), "{shown}");
    }

    #[test]
    fn cuts_long_text_and_keeps_room_for_titles() {
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abcd", 4), "abcd");
        let mut sessions = sessions();
        sessions[0].title =
            Some("A title much longer than the room the other columns leave".into());
        // Only the title is too wide for 50 columns, so it is cut, to no less than 16.
        let shown = narrow(&sessions, &[KeyCode::Enter, KeyCode::Enter], 50);
        let line = shown.lines().find(|line| line.contains("▸")).unwrap();
        assert!(line.contains("▸ chat  A title m…"), "{line}");
        assert!(line.contains('…'), "{line}");
    }

    #[test]
    fn colors_follow_the_theme() {
        let soft = draw(&sessions(), &[], Paint::colored(Theme::Soft), 120);
        let plain = draw(&sessions(), &[], Paint::OFF, 120);
        let at = |buffer: &Buffer| {
            let first = buffer
                .content()
                .iter()
                .position(|c| c.symbol() == "$")
                .unwrap();
            buffer.content()[first].fg
        };
        // The header's spend comes first: money, in sage.
        assert_eq!(at(&soft), Color::Indexed(108));
        assert_eq!(at(&plain), Color::Reset);
    }
}
