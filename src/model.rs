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
    /// The session the client recorded the request in, when it says. A copy keeps it.
    pub owner: Option<String>,
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

/// A session named by the client that recorded it and its id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionRef {
    pub client: Client,
    pub id: String,
}

/// One local session.
#[derive(Debug, Clone)]
pub struct Session {
    pub client: Client,
    pub id: String,
    /// The session that started this one, or that it was forked or edited from. A chat started in
    /// a Claude Code scratchpad names a Claude Code parent whichever client recorded it.
    pub parent: Option<SessionRef>,
    pub kind: Kind,
    pub cwd: Option<String>,
    pub title: Option<String>,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
    pub items: Vec<Item>,
    /// The tool calls in this session's own transcript, a call with an id counted once. `items` can
    /// repeat a call and, for a Codex fork, also holds the history inherited from its base.
    pub tool_calls: u64,
    pub turns: Vec<Turn>,
    pub usage: Vec<Usage>,
    pub history_base: Option<HistoryBase>,
    /// The session holds replies but no usage could be read from it, so its spend is missing: for
    /// example, its client changed how it records usage or did not record it. Set when loading.
    pub usage_missing: bool,
}

impl Session {
    /// The client and id that name this session.
    pub fn reference(&self) -> SessionRef {
        SessionRef {
            client: self.client,
            id: self.id.clone(),
        }
    }

    /// `client:id`, unique across both clients.
    pub fn key(&self) -> String {
        format!("{}:{}", self.client.name(), self.id)
    }

    /// The time between the session's recorded start and end, gaps included.
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
    pub role: PrRole,
}

/// A conversation's role in a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrRole {
    ContributedTo,
    Reviewed,
    MentionedOnly,
}

impl PrRole {
    pub const ALL: [PrRole; 3] = [
        PrRole::ContributedTo,
        PrRole::Reviewed,
        PrRole::MentionedOnly,
    ];

    /// The role's name in prompts, answers, the cache, and the CSV.
    pub fn name(self) -> &'static str {
        match self {
            PrRole::ContributedTo => "Contributed to",
            PrRole::Reviewed => "Reviewed",
            PrRole::MentionedOnly => "Mentioned only",
        }
    }

    pub fn from_name(name: &str) -> Option<PrRole> {
        PrRole::ALL.into_iter().find(|role| role.name() == name)
    }

    /// Whether the conversation worked on the pull request, not only mentioned it.
    pub fn worked_on(self) -> bool {
        self != PrRole::MentionedOnly
    }
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
