//! The `atlas-nano` command: parse the arguments and run `configure` or `run`.

use std::process::ExitCode;

use clap::Parser;

use atlas_nano::cli::{Cli, Command};
use atlas_nano::config::Config;
use atlas_nano::paint::{Paint, Tone};
use atlas_nano::{configure, run};

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Configure => configure::configure(),
        Command::Run(args) => run::run(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The saved theme when there is one; the default when the error is in loading it.
            let theme = Config::load()
                .ok()
                .flatten()
                .map(|config| config.theme)
                .unwrap_or_default();
            let paint = Paint::stderr(theme);
            // Line by line, so a multi-line error keeps its lines and shows control characters
            // from a client's reply as data.
            let lines: Vec<String> = error
                .lines()
                .map(|line| paint.tone(Tone::Plain, line))
                .collect();
            eprintln!(
                "{} {}",
                paint.tone(Tone::Error, "atlas-nano:"),
                lines.join("\n")
            );
            ExitCode::FAILURE
        }
    }
}
