//! `atlas-nano run`: read the sessions, label them, and print the report.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write as _};

use chrono::{Duration, Utc};

use crate::account;
use crate::cli::{Period, RunArgs};
use crate::config::{self, Config};
use crate::ingest;
use crate::label;
use crate::model::Session;
use crate::paint::{Paint, Tone};
use crate::place::{self, Place};
use crate::prices::PriceTable;
use crate::report::{Report, Row};
use crate::taxonomy::Taxonomy;

/// The hours a period covers, back from the time of the run; None covers everything.
fn hours(period: Period) -> Option<i64> {
    match period {
        Period::Days7 => Some(168),
        Period::Days30 => Some(720),
        Period::All => None,
    }
}

fn name(period: Period) -> &'static str {
    match period {
        Period::Days7 => "last 7 days",
        Period::Days30 => "last 30 days",
        Period::All => "all time",
    }
}

pub fn run(args: &RunArgs) -> Result<(), String> {
    let Some(config) = Config::load()? else {
        return Err("no saved configuration; run `atlas-nano configure` first".to_string());
    };
    let now = Utc::now();
    let home = config::home()?;
    let loaded = ingest::load(&home, &config.clients()?);
    // Paths are quoted with escapes, so a file name cannot send control characters to the terminal.
    let errors = Paint::stderr(config.theme);
    for (path, error) in &loaded.unreadable {
        eprintln!(
            "{} could not read {path:?}: {error}",
            errors.tone(Tone::Warning, "warning:")
        );
    }
    let sessions = loaded.sessions;
    let roots = account::roots(&sessions);
    let cutoff = hours(args.period).map(|hours| now - Duration::hours(hours));
    let covered = account::in_period(&sessions, &roots, cutoff);
    let prices = PriceTable::built_in();

    // Rows hold only the covered sessions, so a root's position is its rank among them.
    let position: Vec<usize> = covered
        .iter()
        .scan(0, |next, &kept| {
            let at = *next;
            *next += usize::from(kept);
            Some(at)
        })
        .collect();
    let taxonomy = Taxonomy::built_in();
    let outcome = if args.no_label {
        None
    } else {
        let selected: Vec<&Session> = (0..sessions.len())
            .filter(|&n| covered[n])
            .map(|n| &sessions[n])
            .collect();
        Some(label::label(
            &selected,
            &config,
            &taxonomy,
            &prices,
            args.no_cache,
            args.confirm,
        )?)
    };
    // A session starts where its root chat did; each starting folder is placed once.
    // A session with no recorded folder is placed as unknown.
    let starts: Vec<&str> = (0..sessions.len())
        .map(|n| sessions[roots[n]].cwd.as_deref().unwrap_or_default())
        .collect();
    let mut places: HashMap<&str, Place> = HashMap::new();
    for n in (0..sessions.len()).filter(|&n| covered[n]) {
        places
            .entry(starts[n])
            .or_insert_with(|| place::place(starts[n], &home));
    }
    let rows: Vec<Row> = (0..sessions.len())
        .filter(|&n| covered[n])
        .map(|n| Row {
            session: &sessions[n],
            cost: account::cost(&sessions[n], &prices),
            label: outcome
                .as_ref()
                .and_then(|outcome| outcome.labels.get(&sessions[n].key())),
            root: position[roots[n]],
            place: &places[starts[n]],
        })
        .collect();
    let report = Report {
        period: name(args.period),
        rows,
        taxonomy: outcome.as_ref().map(|_| &taxonomy),
        labeling: outcome.as_ref().map(|outcome| outcome.summary.clone()),
        pull_requests: outcome.is_some() && config.labeler.pull_requests,
        prices: &prices,
        paint: Paint::stdout(config.theme),
    };
    // The CSV is written first, so a reader that closes the report early, like `head`, loses none of it.
    if let Some(path) = &args.csv {
        fs::write(path, report.csv()).map_err(|error| format!("{}: {error}", path.display()))?;
        eprintln!("{} {}", errors.tone(Tone::Done, "wrote"), path.display());
    }
    match io::stdout().lock().write_all(report.render().as_bytes()) {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => Err(error.to_string()),
        _ => Ok(()),
    }
}
