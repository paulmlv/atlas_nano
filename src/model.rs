//! What Atlas Nano keeps of one local session.

use chrono::{DateTime, Duration, Utc};

/// The client that recorded a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Client {
    Claude,
    Codex,
}

impl Client {
    /// The client's name in keys, the price table, and the CSV.
    pub fn name(self) -> &'static str {
        match self {
            Client::Claude => "claude",
            Client::Codex => "codex",
        }
    }
}

/// Who started a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A chat the user started, including a fork or an edit of one.
    Primary,
    /// A session an agent started.
    Subagent,
    /// A Codex safety review, which is never labeled.
    Review,
}

/// The tokens one model request reported, split the way its provider bills them.
///
/// `id` is the same in every session that holds a copy of the request. `input` excludes cached and
/// cache-write tokens, and `cache_write` excludes the one-hour writes in `cache_write_1h`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage {
    pub id: String,
    pub at: DateTime<Utc>,
    pub model: String,
    pub fast: bool,
    pub input: u64,
    pub cached: u64,
    pub cache_write: u64,
    pub cache_write_1h: u64,
    pub output: u64,
}

impl Usage {
    /// All input tokens of the request, cached and cache-write tokens included.
    pub fn prompt(&self) -> u64 {
        self.input + self.cached + self.cache_write + self.cache_write_1h
    }

    /// All input and output tokens of the request.
    pub fn tokens(&self) -> u64 {
        self.prompt() + self.output
    }
}

/// Who wrote one part of the text sent to the labeling model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Agent,
    Tool,
}

impl Role {
    /// The marker that starts the part in the rendered text.
    pub fn marker(self) -> &'static str {
        match self {
            Role::User => "[user]",
            Role::Assistant => "[assistant]",
            Role::Agent => "[agent]",
            Role::Tool => "[tool]",
        }
    }
}

/// One kept part of a session: a message or a tool call, at its record position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub position: u64,
    pub role: Role,
    pub text: String,
}

/// One visible user request and how long the model worked on it, when that is recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub at: DateTime<Utc>,
    pub working: Option<Duration>,
}

/// The rollout a Codex session inherits history from, up to an exclusive record position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryBase {
    pub rollout: String,
    pub end: u64,
}

/// One local session.
#[derive(Debug, Clone)]
pub struct Session {
    pub client: Client,
    pub id: String,
    /// The session that started this one, or that it was forked or edited from.
    pub parent: Option<String>,
    /// The client that recorded `parent`: Claude Code for a chat started in its scratchpad.
    pub parent_client: Client,
    pub kind: Kind,
    pub cwd: String,
    pub title: String,
    pub repository: String,
    pub branch: String,
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
    pub items: Vec<Item>,
    pub tool_calls: u64,
    pub turns: Vec<Turn>,
    pub usage: Vec<Usage>,
    pub history_base: Option<HistoryBase>,
}

impl Session {
    /// `client:id`, unique across both clients.
    pub fn key(&self) -> String {
        format!("{}:{}", self.client.name(), self.id)
    }

    /// The time from the first record to the last, gaps included.
    pub fn wall(&self) -> Option<Duration> {
        Some(self.ended? - self.started?)
    }

    /// The working time of the turns that record a completion.
    pub fn working(&self) -> Duration {
        self.turns.iter().filter_map(|turn| turn.working).sum()
    }
}

/// A pull request the labeling model found in a conversation, with the conversation's role in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    pub url: String,
    pub role: String,
}

/// The labels one session received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub work: String,
    pub topic: String,
    pub prs: Vec<PullRequest>,
    pub model: String,
    pub from_cache: bool,
}
