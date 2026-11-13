//! Command-line interface: the commands and flags described in `docs/CLI.md`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::config::Output;

/// Atlas Nano labels and accounts for local Claude Code and Codex conversations.
#[derive(Debug, Parser)]
#[command(name = "atlas-nano", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Detect the installed clients and save the settings a run needs.
    Configure,
    /// Read the local sessions, label them, and print the report.
    Run(RunArgs),
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Time horizon, measured back from the time of the run.
    #[arg(long, value_enum, default_value_t = Period::Days7)]
    pub period: Period,

    /// Relabel everything, ignoring stored labels.
    #[arg(long)]
    pub no_cache: bool,

    /// Also write one CSV row per session to this path.
    #[arg(long, value_name = "PATH")]
    pub csv: Option<PathBuf>,

    /// Also write one JSON line per session to this path.
    #[arg(long, value_name = "PATH")]
    pub json: Option<PathBuf>,

    /// Accounting only, with no model calls.
    #[arg(long)]
    pub no_label: bool,

    /// Ask before labeling, after the estimate is printed.
    #[arg(long)]
    pub confirm: bool,

    /// Try a failed labeling call this many more times, instead of the saved setting.
    #[arg(long, value_name = "N")]
    pub retries: Option<usize>,

    /// How to show the report, instead of the saved setting.
    #[arg(long, value_enum, value_name = "MODE")]
    pub output: Option<Output>,
}

/// Which sessions a run covers, by when their conversation started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Period {
    /// The last 24 hours.
    #[value(name = "1d")]
    Day1,
    /// The last 168 hours.
    #[value(name = "7d")]
    Days7,
    /// The last 720 hours.
    #[value(name = "30d")]
    Days30,
    /// Every session.
    #[value(name = "all")]
    All,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_args(flags: &[&str]) -> RunArgs {
        let argv = ["atlas-nano", "run"].iter().chain(flags);
        match Cli::try_parse_from(argv).expect("arguments parse").command {
            Command::Run(run) => run,
            Command::Configure => panic!("expected the run command"),
        }
    }

    #[test]
    fn run_defaults_to_seven_days_with_cache_and_labels() {
        let run = run_args(&[]);
        assert_eq!(run.period, Period::Days7);
        assert!(!run.no_cache);
        assert!(!run.no_label);
        assert!(!run.confirm);
        assert_eq!(run.csv, None);
        assert_eq!(run.json, None);
        assert_eq!(run.retries, None);
        assert_eq!(run.output, None);
    }

    #[test]
    fn run_reads_every_flag() {
        let run = run_args(&[
            "--period",
            "30d",
            "--no-cache",
            "--csv",
            "out.csv",
            "--json",
            "out.jsonl",
            "--no-label",
            "--confirm",
            "--retries",
            "0",
            "--output",
            "print",
        ]);
        assert_eq!(run.output, Some(Output::Print));
        assert_eq!(run.period, Period::Days30);
        assert!(run.no_cache);
        assert!(run.no_label);
        assert!(run.confirm);
        assert_eq!(run.csv, Some(PathBuf::from("out.csv")));
        assert_eq!(run.json, Some(PathBuf::from("out.jsonl")));
        assert_eq!(run.retries, Some(0));
    }

    #[test]
    fn run_reads_one_day() {
        assert_eq!(run_args(&["--period", "1d"]).period, Period::Day1);
    }

    #[test]
    fn run_rejects_an_unknown_period() {
        assert!(Cli::try_parse_from(["atlas-nano", "run", "--period", "90d"]).is_err());
    }

    #[test]
    fn configure_takes_no_arguments() {
        let cli = Cli::try_parse_from(["atlas-nano", "configure"]).expect("arguments parse");
        assert!(matches!(cli.command, Command::Configure));
        assert!(Cli::try_parse_from(["atlas-nano", "configure", "extra"]).is_err());
    }
}
