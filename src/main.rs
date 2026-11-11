//! The `atlas-nano` command: parse the arguments and run `configure` or `run`.

use std::process::ExitCode;

use clap::Parser;

use atlas_nano::cli::{Cli, Command};
use atlas_nano::{configure, paint, run};

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Configure => configure::configure(),
        Command::Run(args) => run::run(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let paint = paint::Paint::stderr(paint::Theme::default());
            eprintln!("{} {error}", paint.tone(paint::Tone::Error, "atlas-nano:"));
            ExitCode::FAILURE
        }
    }
}
