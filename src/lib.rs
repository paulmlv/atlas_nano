//! Atlas Nano reads local Claude Code and Codex conversations, accounts for their spend, and labels
//! them by type of work and topic. The `atlas-nano` command is a thin layer over these modules.

pub mod account;
pub mod cli;
pub mod config;
pub mod configure;
pub mod ingest;
pub mod label;
pub mod labeler;
pub mod model;
pub mod paint;
pub mod place;
pub mod prices;
pub mod prompt;
pub mod report;
pub mod run;
pub mod taxonomy;
pub mod text;
