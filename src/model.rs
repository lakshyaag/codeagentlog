//! Agent-neutral data model shared by adapters, the cache and renderers.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use jiff::Timestamp;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Agent {
    Pi,
    Claude,
    Codex,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::Pi, Agent::Claude, Agent::Codex];

    pub fn name(self) -> &'static str {
        match self {
            Agent::Pi => "pi",
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    pub fn from_name(s: &str) -> Option<Agent> {
        Agent::ALL.into_iter().find(|a| a.name() == s)
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Session metadata. Cached in sqlite, so keep it serde-friendly and small.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMeta {
    pub agent: Agent,
    /// Native id (Claude children: `<sessionId>/<agentId>`).
    pub id: String,
    pub path: PathBuf,
    /// Every cwd seen, in order of first appearance. The first one is the display cwd.
    pub cwds: Vec<PathBuf>,
    pub git_branch: Option<String>,
    pub title: Option<Title>,
    pub model: Option<String>,
    pub started: Timestamp,
    pub updated: Timestamp,
    pub record_count: usize,
    pub prompt_count: usize,
    /// First few user messages, one-lined and truncated (for `ls --prompts`).
    pub first_prompts: Vec<String>,
    /// Last few user messages, oldest first (for `ls --recent`).
    pub last_prompts: Vec<String>,
    /// Native id of the parent session, for child sessions.
    pub parent_id: Option<String>,
    pub child_kind: Option<String>,
    /// Tool-call id in the parent transcript that spawned this child.
    pub spawn_ref: Option<String>,
    /// Session this one was forked from (pi `parentSession`: a file path).
    pub forked_from: Option<String>,
    /// Session this one continues in (Claude `continued-in`).
    pub continued_in: Option<String>,
    pub diagnostics: Diagnostics,
}

impl SessionMeta {
    pub fn new(agent: Agent, path: PathBuf) -> Self {
        SessionMeta {
            agent,
            id: String::new(),
            path,
            cwds: Vec::new(),
            git_branch: None,
            title: None,
            model: None,
            started: Timestamp::UNIX_EPOCH,
            updated: Timestamp::UNIX_EPOCH,
            record_count: 0,
            prompt_count: 0,
            first_prompts: Vec::new(),
            last_prompts: Vec::new(),
            parent_id: None,
            child_kind: None,
            spawn_ref: None,
            forked_from: None,
            continued_in: None,
            diagnostics: Diagnostics::default(),
        }
    }

    /// Namespaced id, e.g. `codex:019ff0bf-…`.
    pub fn display_id(&self) -> String {
        format!("{}:{}", self.agent, self.id)
    }

    pub fn add_cwd(&mut self, cwd: &str) {
        if cwd.is_empty() {
            return;
        }
        let p = PathBuf::from(cwd);
        if !self.cwds.contains(&p) {
            self.cwds.push(p);
        }
    }

    pub fn is_child(&self) -> bool {
        self.parent_id.is_some()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Title {
    pub text: String,
    /// Where the title came from: `ai-title`, `thread_name`, `session_info`, `first-user`, …
    pub source: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    /// Lines that failed to parse (a truncated final line is not counted).
    pub bad_lines: usize,
    /// Record types the adapter doesn't know, rendered generically.
    pub unknown: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
    Event,
    System,
    Developer,
    Bash,
    Custom(String),
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        Some(match s {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            "event" => Role::Event,
            "system" => Role::System,
            "developer" => Role::Developer,
            "bash" => Role::Bash,
            _ => return s.strip_prefix("custom:").map(|t| Role::Custom(t.to_string())),
        })
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::User => f.write_str("user"),
            Role::Assistant => f.write_str("assistant"),
            Role::Tool => f.write_str("tool"),
            Role::Event => f.write_str("event"),
            Role::System => f.write_str("system"),
            Role::Developer => f.write_str("developer"),
            Role::Bash => f.write_str("bash"),
            Role::Custom(t) => write!(f, "custom:{t}"),
        }
    }
}

impl Serialize for Role {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// One numbered record of a transcript.
#[derive(Clone, Debug, Serialize)]
pub struct Turn {
    /// 1-based, stable: assigned once in file order, never renumbered by filters.
    pub idx: usize,
    pub role: Role,
    pub ts: Option<Timestamp>,
    /// Raw record type, e.g. `message/assistant`, `event_msg/token_count`.
    pub kind: String,
    pub blocks: Vec<Block>,
    /// Secondary stream (Codex `event_msg`): only shown with `--events`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub aux: bool,
    /// Tree position (pi entries).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<Node>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Node {
    pub id: String,
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    /// Empty text means the reasoning was encrypted or redacted.
    Thinking {
        text: String,
    },
    ToolCall {
        id: Option<String>,
        name: String,
        args: Value,
    },
    ToolResult {
        name: Option<String>,
        ok: Option<bool>,
        text: String,
    },
    Image {
        mime: Option<String>,
        bytes: usize,
    },
    /// A non-conversational record. `summary` is an optional human one-liner;
    /// renderers fall back to the compact payload.
    Event {
        summary: String,
        payload: Value,
    },
}

impl Block {
    pub fn text(s: impl Into<String>) -> Block {
        Block::Text { text: s.into() }
    }

    pub fn event(summary: impl Into<String>, payload: Value) -> Block {
        Block::Event { summary: summary.into(), payload }
    }
}

/// A fully loaded transcript.
pub struct Session {
    pub meta: SessionMeta,
    pub turns: Vec<Turn>,
}

impl Session {
    pub fn new(agent: Agent, path: PathBuf) -> Self {
        Session { meta: SessionMeta::new(agent, path), turns: Vec::new() }
    }

    /// Append a record, assigning the next stable index. Returns its position.
    pub fn push(&mut self, role: Role, ts: Option<Timestamp>, kind: impl Into<String>, blocks: Vec<Block>) -> usize {
        self.turns.push(Turn {
            idx: self.turns.len() + 1,
            role,
            ts,
            kind: kind.into(),
            blocks,
            aux: false,
            node: None,
        });
        self.turns.len() - 1
    }
}
