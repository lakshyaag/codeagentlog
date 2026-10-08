//! One adapter per agent: discover session files and parse them into the
//! shared model. Parsing is lenient: unknown records become generic events
//! and are counted in diagnostics, never fatal.

mod claude;
mod codex;
mod pi;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;
use walkdir::WalkDir;

use crate::model::{Agent, Role, Session, SessionMeta, Title};
use crate::text::{self, home_dir};

pub trait Adapter: Send + Sync {
    /// Directories scanned for sessions.
    fn session_dirs(&self) -> Vec<PathBuf>;
    /// Session files, stat only.
    fn discover(&self) -> Vec<PathBuf> {
        self.session_dirs().iter().flat_map(|d| jsonl_files(d, 6)).filter(|p| self.is_session_file(p)).collect()
    }
    fn is_session_file(&self, path: &Path) -> bool;
    /// Full parse of one session file.
    fn load(&self, path: &Path) -> Result<Session>;
    /// Post-process metadata with agent-wide state (e.g. Codex title index).
    fn decorate(&self, _metas: &mut [SessionMeta]) {}
}

/// Where each agent keeps its data. Env vars and `--root agent=PATH` override defaults.
#[derive(Clone)]
pub struct Roots(BTreeMap<Agent, PathBuf>);

impl Roots {
    pub fn resolve(overrides: &[String]) -> Result<Self> {
        let home = home_dir().map(Path::to_path_buf).unwrap_or_default();
        let env_or =
            |var: &str, default: &str| std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| home.join(default));
        let mut roots = BTreeMap::from([
            (Agent::Pi, env_or("PI_CODING_AGENT_DIR", ".pi/agent")),
            (Agent::Claude, env_or("CLAUDE_CONFIG_DIR", ".claude")),
            (Agent::Codex, env_or("CODEX_HOME", ".codex")),
        ]);
        for o in overrides {
            let Some((name, path)) = o.split_once('=') else {
                bail!("--root expects AGENT=PATH, got {o:?}");
            };
            let agent = Agent::from_name(name).with_context(|| format!("unknown agent {name:?} in --root"))?;
            roots.insert(agent, PathBuf::from(path));
        }
        Ok(Roots(roots))
    }

    pub fn adapter(&self, agent: Agent) -> Box<dyn Adapter> {
        let home = self.0[&agent].clone();
        match agent {
            Agent::Pi => Box::new(pi::Pi { home }),
            Agent::Claude => Box::new(claude::Claude { home }),
            Agent::Codex => Box::new(codex::Codex { home }),
        }
    }

    /// The adapter whose session dirs contain `path`, else a guess from file content.
    pub fn adapter_for_path(&self, path: &Path) -> Box<dyn Adapter> {
        let owner =
            Agent::ALL.into_iter().find(|&a| self.adapter(a).session_dirs().iter().any(|d| path.starts_with(d)));
        self.adapter(owner.unwrap_or_else(|| sniff(path)))
    }
}

/// Guess the agent from a file's first record.
fn sniff(path: &Path) -> Agent {
    let first = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.lines().next().and_then(|l| serde_json::from_str::<Value>(l).ok()));
    match first.as_ref().and_then(|v| text::str_field(v, "type")) {
        Some("session") => Agent::Pi,
        Some("session_meta" | "response_item" | "event_msg" | "turn_context") => Agent::Codex,
        _ => Agent::Claude,
    }
}

fn jsonl_files(dir: &Path, max_depth: usize) -> impl Iterator<Item = PathBuf> {
    WalkDir::new(dir)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "jsonl"))
        .map(walkdir::DirEntry::into_path)
}

/// Parse a JSONL file record by record. Malformed lines are counted, except a
/// truncated final line (a live session mid-write), which is skipped silently.
fn read_jsonl(path: &Path, mut on_record: impl FnMut(Value)) -> Result<usize> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let lines: Vec<&[u8]> = bytes.split(|&b| b == b'\n').filter(|l| !l.iter().all(u8::is_ascii_whitespace)).collect();
    let mut bad = 0;
    for (i, line) in lines.iter().enumerate() {
        match serde_json::from_slice::<Value>(line) {
            Ok(v) => on_record(v),
            Err(_) if i + 1 == lines.len() => {}
            Err(_) => bad += 1,
        }
    }
    Ok(bad)
}

/// Derive the generic metadata fields once all records are parsed.
fn finalize(session: &mut Session, bad_lines: usize) {
    const PROMPTS_KEPT: usize = 10;
    let meta = &mut session.meta;
    meta.diagnostics.bad_lines = bad_lines;
    meta.record_count = session.turns.len();

    let prompts: Vec<String> = session
        .turns
        .iter()
        .filter(|t| t.role == Role::User && !t.aux)
        .map(|t| {
            let text: Vec<&str> = t
                .blocks
                .iter()
                .filter_map(|b| match b {
                    crate::model::Block::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect();
            text.join(" ")
        })
        .collect();
    meta.prompt_count = prompts.len();
    meta.first_prompts = prompts.iter().take(PROMPTS_KEPT).map(|p| text::snippet(p, 200)).collect();
    meta.last_prompts =
        prompts[prompts.len().saturating_sub(PROMPTS_KEPT)..].iter().map(|p| text::snippet(p, 200)).collect();
    if meta.title.is_none()
        && let Some(first) = prompts.iter().find(|p| !p.trim().is_empty())
    {
        meta.title = Some(Title { text: text::snippet(first, 120), source: "first-user".into() });
    }

    let mtime = std::fs::metadata(&meta.path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| jiff::Timestamp::try_from(t).ok())
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    let stamps = session.turns.iter().filter_map(|t| t.ts);
    meta.started = stamps.clone().min().unwrap_or(mtime);
    meta.updated = stamps.max().unwrap_or(mtime);
    if meta.id.is_empty() {
        meta.id = meta.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    }
}

fn count_unknown(meta: &mut SessionMeta, kind: &str) {
    *meta.diagnostics.unknown.entry(kind.to_string()).or_default() += 1;
}
