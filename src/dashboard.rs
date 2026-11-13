//! `atlas-nano run --tui`: the report as an interactive view. Each table is a tab; a line opens the
//! conversations it sums, and a conversation opens its sessions.

use std::collections::{HashMap, HashSet};

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Cell, Paragraph, Row as TableRow, Table as TableWidget, TableState, Tabs,
};
use rust_decimal::Decimal;

use crate::account::Totals;
use crate::model::{Kind, Label, Usage};
use crate::paint::{Paint, Tone, printable};
use crate::report::{
    self, COLUMNS, Group, NO_REPO, Report, Row, category, count, local_day, money, share, span,
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
            let worked = report::worked_on(row);
            if worked.contains(&url.as_str()) {
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

/// One line of a table: its cells, each in its tone, whether it heads a group, and what it opens.
struct Item {
    cells: Vec<(String, Tone)>,
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

    /// Select line `at`, or the last line when there are fewer.
    fn go(&mut self, at: usize) {
        if let Some(last) = self.items.len().checked_sub(1) {
            self.state.select(Some(at.min(last)));
        }
    }

    /// Move the selection by `delta` lines, staying on the table.
    fn step(&mut self, delta: isize) {
        let at = self.state.selected().unwrap_or(0);
        self.go(at.saturating_add_signed(delta));
    }

    fn opens(&self) -> Option<&Open> {
        self.items.get(self.state.selected()?)?.open.as_ref()
    }

    /// Move each of `columns` whose cells all hold one value into the note, so titles get the
    /// room. A column holding only `-` or nothing, so nothing known, is left out.
    fn fold(&mut self, columns: &[usize]) {
        let mut moved = Vec::new();
        for &column in columns.iter().rev() {
            let Some(first) = self.items.first().map(|item| item.cells[column].0.clone()) else {
                return;
            };
            if self.items.iter().any(|item| item.cells[column].0 != first) {
                continue;
            }
            if first != "-" && !first.is_empty() {
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

fn plain(text: impl Into<String>) -> (String, Tone) {
    (text.into(), Tone::Plain)
}

/// A table of groups, each followed by its members, as the printed report sorts them.
fn grouped(all: &Totals, title: &str, groups: Vec<Group>) -> Table {
    let nested = report::nested(&groups);
    let line = |line: report::Line, indent: &str, strong: bool| {
        let totals = &line.totals;
        Item {
            cells: vec![
                plain(format!("{indent}{}", line.name)),
                plain(totals.conversations.to_string()),
                (
                    share(totals.conversations.into(), all.conversations.into()),
                    Tone::Dim,
                ),
                plain((totals.subagents + totals.reviews).to_string()),
                plain(totals.turns.to_string()),
                plain(span(totals.working)),
                plain(span(totals.wall)),
                plain(count(totals.tokens.total())),
                (money(totals.spend), Tone::Money),
                (share(totals.spend, all.spend), Tone::Dim),
            ],
            strong,
            open: Some(Open::Chats(line.name, line.rows, Slice::All)),
        }
    };
    let mut items = Vec::new();
    for Group {
        line: group,
        members,
    } in groups
    {
        items.push(line(group, "", nested));
        items.extend(members.into_iter().map(|member| line(member, "  ", false)));
    }
    let mut header = vec![title];
    header.extend(COLUMNS);
    Table::new(title, &header, items)
}

/// Requests, tokens, and spend per client and model.
fn models(report: &Report, all: &Totals) -> Table {
    let items = report::models(&report.rows)
        .into_iter()
        .map(|model| {
            let spend = if model.unpriced == model.requests {
                ("no price".to_string(), Tone::Warning)
            } else {
                (money(model.spend), Tone::Money)
            };
            Item {
                cells: vec![
                    plain(model.name.clone()),
                    plain(model.requests.to_string()),
                    plain(count(model.tokens)),
                    spend,
                    (share(model.spend, all.spend), Tone::Dim),
                ],
                strong: false,
                open: Some(Open::Chats(
                    model.name.clone(),
                    model.rows,
                    Slice::Model(model.name),
                )),
            }
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
    let days = report::days(&report.rows);
    let top = days.iter().map(|day| day.spend).max().unwrap_or_default();
    let items = days
        .into_iter()
        .map(|day| Item {
            cells: vec![
                plain(day.day.clone()),
                (money(day.spend), Tone::Money),
                (report::bar(day.spend, top), Tone::Accent),
            ],
            strong: false,
            open: Some(Open::Chats(day.day.clone(), day.rows, Slice::Day(day.day))),
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
                    plain(pr.url.clone()),
                    plain(pr.rows.len().to_string()),
                    (format!("{mark}{}", money(pr.spend)), Tone::Money),
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

/// One conversation's part of a line: its sessions there, their tokens and spend, and the session
/// whose type of work it shows.
struct ChatPart {
    sessions: u64,
    tokens: u64,
    spend: Decimal,
    labeled: usize,
}

/// The conversations that `rows` belong to, largest spend first, counting only those rows' `slice`.
fn chats(report: &Report, title: &str, rows: &[usize], slice: &Slice) -> Table {
    let mut by_root: HashMap<usize, ChatPart> = HashMap::new();
    for &n in rows {
        let row = &report.rows[n];
        let (tokens, spend) = part(row, slice);
        let chat = by_root.entry(row.root).or_insert(ChatPart {
            sessions: 0,
            tokens: 0,
            spend: Decimal::ZERO,
            labeled: n,
        });
        chat.sessions += 1;
        chat.tokens += tokens;
        chat.spend += spend;
        // The chat's own label when it is one of the rows, else the first session's that is.
        if n == row.root {
            chat.labeled = n;
        }
    }
    let mut listed: Vec<(usize, ChatPart)> = by_root.into_iter().collect();
    listed.sort_by(|a, b| b.1.spend.cmp(&a.1.spend).then(a.0.cmp(&b.0)));
    let whole: Decimal = listed.iter().map(|chat| chat.1.spend).sum();
    let items = listed
        .into_iter()
        .map(|(root, chat)| {
            let row = &report.rows[root];
            let name = report::title(row.session);
            // A subagent whose chat was not loaded is its own root.
            let name = match row.session.kind {
                Kind::Primary => name,
                Kind::Subagent => format!("(subagent) {name}"),
                Kind::Review => format!("(review) {name}"),
            };
            let work = report.rows[chat.labeled]
                .label
                .map_or("-", |label| &label.work);
            Item {
                cells: vec![
                    plain(name),
                    (
                        row.session.started.map_or(String::new(), local_day),
                        Tone::Dim,
                    ),
                    plain(row.place.project.clone()),
                    plain(work),
                    plain(chat.sessions.to_string()),
                    plain(count(chat.tokens)),
                    (money(chat.spend), Tone::Money),
                    (share(chat.spend, whole), Tone::Dim),
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
    // Each pull request once, though several sessions name it.
    let prs = rows
        .iter()
        .flat_map(|&n| report.rows[n].label.iter().flat_map(|label| &label.prs));
    for pr in prs {
        let line = format!("{} {}", pr.role.name(), pr.url);
        if !note.contains(&line) {
            note.push(line);
        }
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
            let label = |pick: fn(&Label) -> &str| plain(row.label.map_or("-", pick));
            Item {
                cells: vec![
                    plain(format!("{kind}  {}", report::title(session))),
                    label(|label| &label.work),
                    label(|label| &label.topic),
                    plain(models.join(", ")),
                    plain(session.turns.len().to_string()),
                    plain(span(session.working())),
                    plain(count(row.cost.tokens.total())),
                    (money(row.cost.spend), Tone::Money),
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
        let mut all = Totals::default();
        for row in &report.rows {
            all.add(row.session, &row.cost);
        }
        let groups = |key: &dyn Fn(&Row) -> (String, String)| report::groups(&report.rows, key);
        let mut tabs = Vec::new();
        if let Some(taxonomy) = report.taxonomy {
            let work = groups(&|row| category(row, &taxonomy.work, |l| &l.work));
            tabs.push(("Work", grouped(&all, "By type of work", work)));
            let topics = groups(&|row| category(row, &taxonomy.topics, |l| &l.topic));
            tabs.push(("Topic", grouped(&all, "By topic", topics)));
        }
        tabs.push(("Models", models(report, &all)));
        let projects = groups(&|row| (row.place.project.clone(), String::new()));
        tabs.push(("Projects", grouped(&all, "By project", projects)));
        tabs.push(("Repos", grouped(&all, "By repo", groups(&report::repo))));
        tabs.push(("Days", days(report)));
        // The conversations the printed report ranks, with every session they started.
        let ranked: HashSet<usize> = report::chats(&report.rows)
            .into_iter()
            .map(|(root, _)| root)
            .collect();
        let every: Vec<usize> = (0..report.rows.len())
            .filter(|&n| ranked.contains(&report.rows[n].root))
            .collect();
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
            KeyCode::Home | KeyCode::Char('g') => self.screen().go(0),
            KeyCode::End | KeyCode::Char('G') => self.screen().go(usize::MAX),
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
        let (dim, warning) = (p.style(Tone::Dim, false), p.style(Tone::Warning, false));
        let caveat = |n: u64, text: &str| match n {
            0 => Span::raw(""),
            n => Span::styled(format!(" ({n} {text})"), warning),
        };
        let header = vec![
            Line::from(vec![
                Span::styled("atlas-nano", p.style(Tone::Heading, false)),
                format!(" · {} · ", self.report.period).into(),
                Span::styled(money(all.spend), p.style(Tone::Money, true)),
                caveat(
                    all.unpriced_requests,
                    "requests have no price and count as $0",
                ),
                format!(
                    " · {} conversations · {} subagents{reviews}",
                    all.conversations, all.subagents
                )
                .into(),
            ]),
            Line::from(vec![
                Span::styled(format!("working {}", span(all.working)), dim),
                caveat(all.unknown_turns, "turns with no recorded completion"),
                Span::styled(
                    format!(
                        " · wall {} · tokens in {} · cached {} · cache-write {} · out {}",
                        span(all.wall),
                        count(tokens.input),
                        count(tokens.cached),
                        count(tokens.cache_write),
                        count(tokens.output)
                    ),
                    dim,
                ),
            ]),
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
                .highlight_style(p.open_tab()),
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
        let opens = self.screen().items.iter().any(|item| item.open.is_some());
        let open = if opens { "  enter open" } else { "" };
        let help = format!("←/→ tab  ↑/↓ move{open}{back}  q quit");
        frame.render_widget(Paragraph::new(help).style(p.style(Tone::Dim, false)), foot);
        render(self.screen(), frame, inner, p);
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
    // The note takes at most a third of the height, so the table always shows.
    let room = (area.height / 3) as usize;
    let mut lines: Vec<Line> = table
        .note
        .iter()
        .map(|text| Line::styled(printable(text), p.style(Tone::Dim, false)))
        .collect();
    if lines.len() > room {
        let past = lines.len() + 1 - room;
        lines.truncate(room.saturating_sub(1));
        if room > 0 {
            lines.push(Line::styled(
                format!("and {past} more"),
                p.style(Tone::Dim, false),
            ));
        }
    }
    let [note, area] =
        Layout::vertical([Constraint::Length(lines.len() as u16), Constraint::Fill(1)]).areas(area);
    frame.render_widget(Paragraph::new(lines), note);
    let mut widths: Vec<usize> = table
        .header
        .iter()
        .map(|cell| Span::raw(cell).width())
        .collect();
    for item in &table.items {
        for (width, (cell, _)) in widths.iter_mut().zip(&item.cells) {
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
    let widths: Vec<Constraint> = widths
        .iter()
        .map(|&width| Constraint::Length(width as u16))
        .collect();
    // Cut each cell to the width ratatui draws, which is less than asked for on a narrow screen,
    // so a cut number ends in `…` rather than losing its first digits.
    let symbol = Span::raw(SYMBOL).width() as u16;
    let drawn: Vec<usize> = Layout::horizontal(widths.clone())
        .flex(Flex::Start)
        .spacing(SPACING as u16)
        .split(Rect::new(0, 0, area.width.saturating_sub(symbol), 1))
        .iter()
        .map(|column| column.width as usize)
        .collect();
    let cell = |n: usize, text: &str, style: Style| {
        let line = Line::styled(fit(&printable(text), drawn[n]), style);
        Cell::from(line.alignment(if left(n) {
            Alignment::Left
        } else {
            Alignment::Right
        }))
    };
    let header = TableRow::new(table.header.iter().enumerate().map(|(n, text)| {
        let tone = if n == 0 { Tone::Heading } else { Tone::Dim };
        cell(n, text, p.style(tone, false))
    }));
    let rows = table.items.iter().map(|item| {
        TableRow::new(
            item.cells
                .iter()
                .enumerate()
                .map(|(n, (text, tone))| cell(n, text, p.style(*tone, item.strong))),
        )
    });
    // Reversed text would hide the selected day's bar, so its cell is never reversed.
    let bar = table.header.iter().position(|cell| cell.is_empty());
    table.state.select_column(bar);
    let widget = TableWidget::new(rows, widths)
        .header(header)
        .column_spacing(SPACING as u16)
        .row_highlight_style(p.selected())
        .cell_highlight_style(Style::new().not_reversed())
        .highlight_symbol(SYMBOL);
    frame.render_stateful_widget(widget, area, &mut table.state);
}

/// Show the report until the user quits, then give the terminal back as it was.
pub fn show(report: &Report) -> Result<(), String> {
    let mut dashboard = Dashboard::new(report);
    // `try_init` turns raw mode on before steps that can fail, so give the terminal back on error.
    let mut terminal = ratatui::try_init().map_err(|error| {
        ratatui::restore();
        error.to_string()
    })?;
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
    use ratatui::style::{Color, Modifier};

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
        draw_labeled(sessions, &[], keys, paint, width)
    }

    /// `draw`, with each session's type of work from `works`, "Writing code" past its end.
    fn draw_labeled(
        sessions: &[Session],
        works: &[&str],
        keys: &[KeyCode],
        paint: Paint,
        width: u16,
    ) -> Buffer {
        let prices = PriceTable::built_in();
        let taxonomy = Taxonomy::built_in();
        let place = Place {
            project: "app".to_string(),
            repo: "app".to_string(),
        };
        let labels: Vec<Label> = (0..sessions.len())
            .map(|n| label(works.get(n).copied().unwrap_or("Writing code")))
            .collect();
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
        text(&draw(sessions, keys, Paint::OFF, width))
    }

    fn text(buffer: &Buffer) -> String {
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

    #[test]
    fn the_open_tab_is_not_dim() {
        let buffer = draw(&sessions(), &[], Paint::colored(Theme::Minimal), 120);
        let tab = buffer
            .content()
            .iter()
            .skip(2 * 120)
            .find(|c| c.symbol() == "W")
            .unwrap();
        assert!(tab.modifier.contains(Modifier::REVERSED | Modifier::BOLD));
        assert!(!tab.modifier.contains(Modifier::DIM));
    }

    #[test]
    fn the_selected_day_keeps_its_bar() {
        let days = [KeyCode::Char('6')];
        let bar = |buffer: &Buffer| {
            buffer
                .content()
                .iter()
                .find(|c| c.symbol() == "█")
                .cloned()
                .unwrap()
        };
        // Soft marks the row with a background, so the bar keeps its color.
        let soft = bar(&draw(&sessions(), &days, Paint::colored(Theme::Soft), 120));
        assert_eq!(
            (soft.fg, soft.bg),
            (Color::Indexed(67), Color::Indexed(237))
        );
        // Minimal uses the same gray. With colors off the row is reversed, but not the bar.
        let minimal = bar(&draw(
            &sessions(),
            &days,
            Paint::colored(Theme::Minimal),
            120,
        ));
        assert_eq!(minimal.bg, Color::Indexed(237));
        let off = bar(&draw(&sessions(), &days, Paint::OFF, 120));
        assert!(!off.modifier.contains(Modifier::REVERSED));
        assert!(minimal.modifier.contains(Modifier::DIM));
    }

    #[test]
    fn the_header_says_what_the_totals_leave_out() {
        // Each session has one request to an unpriced model and one turn with no completion.
        let shown = screen(&sessions(), &[]);
        assert!(
            shown.contains("(2 requests have no price and count as $0)"),
            "{shown}"
        );
        assert!(
            shown.contains("(2 turns with no recorded completion)"),
            "{shown}"
        );
    }

    #[test]
    fn a_narrow_screen_cuts_numbers_at_the_end() {
        let mut cut = false;
        for width in 40..90 {
            let shown = narrow(&sessions(), &[], width);
            let line = shown.lines().find(|line| line.contains("▸")).unwrap();
            // Spend is the only column with a `$`: whole, or cut with `…`, never missing its start.
            if let Some(at) = line.find('$') {
                let before = line[..at].chars().last().unwrap();
                assert!(before == ' ' || before == '~', "{width}: {line}");
                cut |= line[at..].starts_with("$1…") || line[at..].starts_with("$…");
            }
        }
        assert!(cut, "no width cut the spend");
    }

    #[test]
    fn a_conversation_lists_each_pull_request_once() {
        let shown = screen(&sessions(), &[KeyCode::Enter, KeyCode::Enter]);
        let found = shown
            .matches("https://github.com/owner/repo/pull/7")
            .count();
        assert_eq!(found, 1, "{shown}");
    }

    #[test]
    fn a_long_note_leaves_room_for_the_table() {
        let item = Item {
            cells: vec![plain("line")],
            strong: false,
            open: None,
        };
        let mut table = Table::new("t", &["Name"], vec![item]);
        table.note = (0..20).map(|n| format!("note {n}")).collect();
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal
            .draw(|frame| render(&mut table, frame, frame.area(), Paint::OFF))
            .unwrap();
        let shown = text(terminal.backend().buffer());
        assert!(shown.contains("note 2\nand 17 more\n  Name"), "{shown}");
        assert!(shown.contains("▸ line"), "{shown}");
    }

    #[test]
    fn leaves_out_what_the_printed_report_leaves_out() {
        let mut sessions = sessions();
        // A subagent whose chat was not loaded, and a chat that used no tokens.
        let mut orphan = session("orphan", Kind::Subagent, Some("gone"), 3);
        orphan.title = Some("Orphan task".to_string());
        let mut idle = session("idle", Kind::Primary, None, 4);
        idle.title = Some("Idle chat".to_string());
        for usage in &mut idle.usage {
            usage.input = 0;
            usage.cached = 0;
            usage.cache_write = 0;
            usage.cache_write_1h = 0;
            usage.output = 0;
        }
        idle.usage[0].model = "<synthetic>".to_string();
        sessions.extend([orphan, idle]);
        let shown = screen(&sessions, &[KeyCode::Char('7')]);
        assert!(shown.contains("Fix the export"), "{shown}");
        assert!(!shown.contains("Orphan task"), "{shown}");
        assert!(!shown.contains("Idle chat"), "{shown}");
        let shown = screen(&sessions, &[KeyCode::Char('3')]);
        assert!(!shown.contains("<synthetic>"), "{shown}");
    }

    #[test]
    fn a_line_shows_the_type_of_work_of_the_sessions_it_holds() {
        // The chat writes code; its subagent debugs. Debugging opens the chat as debugging.
        let buffer = draw_labeled(
            &sessions(),
            &["Writing code", "Debugging"],
            &[KeyCode::Down, KeyCode::Enter],
            Paint::OFF,
            120,
        );
        let shown = text(&buffer);
        assert!(shown.contains("Work › Debugging"), "{shown}");
        assert!(shown.contains("Type of work Debugging"), "{shown}");
    }

    #[test]
    fn home_and_end_go_to_the_first_and_last_line() {
        let shown = screen(&sessions(), &[KeyCode::End]);
        assert!(shown.contains("▸   Writing code"), "{shown}");
        let shown = screen(&sessions(), &[KeyCode::End, KeyCode::Home]);
        assert!(shown.contains("▸ Software"), "{shown}");
    }

    #[test]
    fn the_footer_offers_enter_only_where_it_opens() {
        assert!(screen(&sessions(), &[]).contains("enter open"));
        let shown = screen(&sessions(), &[KeyCode::Enter, KeyCode::Enter]);
        assert!(!shown.contains("enter open"), "{shown}");
    }
}
