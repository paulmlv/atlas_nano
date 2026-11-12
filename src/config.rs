//! The saved settings a run needs, and where Atlas Nano keeps its files.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::Client;
use crate::paint::Theme;

/// Which client makes the labeling calls, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Labeler {
    /// `claude` or `codex`.
    pub client: String,
    pub model: String,
    /// Whether tool calls are sent; false is the no-tools mode.
    pub tools: bool,
    /// The experimental pull request mapping.
    pub pull_requests: bool,
    /// How many labeling calls run at once.
    pub jobs: usize,
}

/// What `atlas-nano configure` saves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `conversation`; per-turn labeling is not built.
    pub unit: String,
    /// The clients whose sessions are read: `claude`, `codex`, or both.
    pub coverage: Vec<String>,
    /// The color theme of the output; settings saved before themes existed use the default.
    #[serde(default)]
    pub theme: Theme,
    pub labeler: Labeler,
}

/// Parse a client's name as the configuration writes it.
pub fn client(name: &str) -> Result<Client, String> {
    match name {
        "claude" => Ok(Client::Claude),
        "codex" => Ok(Client::Codex),
        other => Err(format!(
            "unknown client `{other}`; expected `claude` or `codex`"
        )),
    }
}

/// The one model each client labels with.
pub fn labeling_model(client: Client) -> &'static str {
    match client {
        Client::Claude => "claude-haiku-5-5",
        Client::Codex => "gpt-6-luna",
    }
}

impl Config {
    /// The clients whose sessions are read.
    pub fn clients(&self) -> Result<Vec<Client>, String> {
        self.coverage.iter().map(|name| client(name)).collect()
    }

    fn validate(self) -> Result<Config, String> {
        self.clients()?;
        let expected = labeling_model(client(&self.labeler.client)?);
        if self.labeler.model != expected {
            return Err(format!(
                "unknown labeling model `{}`; {} labels with `{expected}`",
                self.labeler.model, self.labeler.client
            ));
        }
        if self.unit != "conversation" {
            return Err(format!(
                "unknown unit `{}`; expected `conversation`",
                self.unit
            ));
        }
        if self.coverage.is_empty() || self.labeler.jobs == 0 {
            return Err("coverage must name a client and jobs must be at least 1".to_string());
        }
        Ok(self)
    }

    /// Decode saved settings, rejecting unknown fields and values.
    pub fn decode(text: &str) -> Result<Config, String> {
        toml::from_str::<Config>(text)
            .map_err(|error| error.to_string())?
            .validate()
    }

    pub fn encode(&self) -> String {
        toml::to_string(self).expect("settings encode as TOML")
    }

    /// Read the saved settings; None when `configure` has not run.
    pub fn load() -> Result<Option<Config>, String> {
        let path = config_path()?;
        match fs::read_to_string(&path) {
            Ok(text) => Config::decode(&text)
                .map(Some)
                .map_err(|error| format!("{}: {error}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }

    pub fn save(&self) -> Result<PathBuf, String> {
        let path = config_path()?;
        let folder = path.parent().expect("the settings file has a folder");
        fs::create_dir_all(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
        fs::write(&path, self.encode()).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(path)
    }
}

/// The user's home folder.
pub fn home() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set".to_string())
}

/// An XDG base folder, or `fallback` under the home folder. As the XDG specification says, a
/// relative value is ignored.
fn base(variable: &str, fallback: &str) -> Result<PathBuf, String> {
    match env::var_os(variable).filter(|value| Path::new(value).is_absolute()) {
        Some(folder) => Ok(PathBuf::from(folder)),
        None => Ok(home()?.join(fallback)),
    }
}

/// `$XDG_CONFIG_HOME/atlas-nano/config.toml`, by default under `~/.config`.
pub fn config_path() -> Result<PathBuf, String> {
    Ok(base("XDG_CONFIG_HOME", ".config")?.join("atlas-nano/config.toml"))
}

/// `$XDG_CACHE_HOME/atlas-nano`, by default under `~/.cache`.
pub fn cache_folder() -> Result<PathBuf, String> {
    Ok(base("XDG_CACHE_HOME", ".cache")?.join("atlas-nano"))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn config() -> Config {
        Config {
            unit: "conversation".to_string(),
            coverage: vec!["claude".to_string(), "codex".to_string()],
            theme: Theme::Soft,
            labeler: Labeler {
                client: "claude".to_string(),
                model: "claude-haiku-5-5".to_string(),
                tools: true,
                pull_requests: false,
                jobs: 16,
            },
        }
    }

    #[test]
    fn settings_round_trip_and_reject_what_they_do_not_know() {
        let saved = config().encode();
        assert_eq!(Config::decode(&saved), Ok(config()));
        assert!(Config::decode(&format!("extra = 1\n{saved}")).is_err());
        assert!(Config::decode(&saved.replace("\"codex\"]", "\"cursor\"]")).is_err());
        assert!(Config::decode(&saved.replace("conversation", "turn")).is_err());
        assert!(Config::decode(&saved.replace("jobs = 16", "jobs = 0")).is_err());
        assert!(Config::decode(&saved.replace("\"soft\"", "\"neon\"")).is_err());
        let before_themes = saved.replace("theme = \"soft\"\n", "");
        assert_eq!(Config::decode(&before_themes).unwrap().theme, Theme::default());
        // Each client labels with one model: Haiku through Claude Code, Luna through Codex.
        assert!(Config::decode(&saved.replace("claude-haiku-5-5", "claude-opus-5-5")).is_err());
        let luna = saved.replace("claude-haiku-5-5", "gpt-6-luna");
        assert!(Config::decode(&luna).is_err());
        assert!(Config::decode(&luna.replace("client = \"claude\"", "client = \"codex\"")).is_ok());
    }
}
