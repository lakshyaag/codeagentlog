//! Command implementations. Each returns its full output as a string.

pub mod doctor;
pub mod grep;
pub mod guide;
pub mod ls;
pub mod path;
pub mod show;
pub mod tree;

use std::path::{Path, PathBuf};

use anyhow::Result;
use jiff::Timestamp;
use serde::Serialize;

use crate::Filters;
use crate::adapters::Roots;
use crate::catalog::{Catalog, Entry};
use crate::model::Agent;
use crate::text::{shell_quote, tilde};
use crate::time;

pub struct Ctx {
    pub roots: Roots,
    pub json: bool,
    pub use_cache: bool,
    pub cwd: PathBuf,
}

impl Ctx {
    pub fn new(root_overrides: &[String], json: bool, use_cache: bool) -> Result<Ctx> {
        Ok(Ctx { roots: Roots::resolve(root_overrides)?, json, use_cache, cwd: std::env::current_dir()? })
    }

    pub fn catalog(&self, agents: &[Agent]) -> Catalog {
        let agents = if agents.is_empty() { &Agent::ALL[..] } else { agents };
        Catalog::load(&self.roots, agents, self.use_cache)
    }

    /// Canonical folder argument, defaulting to cwd.
    pub fn base(&self, path: Option<&Path>) -> PathBuf {
        crate::scope::canonical(path.unwrap_or(&self.cwd))
    }
}

impl Filters {
    pub fn since(&self) -> Result<Option<Timestamp>> {
        self.since.as_deref().map(|s| time::parse_since(s, Timestamp::now())).transpose()
    }

    pub fn allows(&self, e: &Entry, since: Option<Timestamp>) -> bool {
        (self.agent.is_empty() || self.agent.contains(&e.meta.agent)) && since.is_none_or(|s| e.meta.updated >= s)
    }

    /// Flags to repeat in a follow-up command.
    pub fn to_args(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.agent.is_empty() {
            out.push(format!("--agent {}", self.agent.iter().map(|a| a.name()).collect::<Vec<_>>().join(",")));
        }
        if let Some(s) = &self.since {
            out.push(format!("--since {}", shell_quote(s)));
        }
        out
    }
}

pub fn to_json<T: Serialize>(v: &T) -> Result<String> {
    Ok(serde_json::to_string_pretty(v)? + "\n")
}

/// `<id>  <span>  N prompts  M records`
pub fn session_summary(e: &Entry) -> String {
    let m = &e.meta;
    format!(
        "{}  {}  {} prompts  {} records",
        m.display_id(),
        time::span(m.started, m.updated),
        m.prompt_count,
        m.record_count
    )
}

pub fn display_cwd(e: &Entry) -> String {
    e.cwd().map(tilde).unwrap_or_else(|| "(unknown cwd)".into())
}

/// Machine-friendly session summary for `--json` listings.
#[derive(Serialize)]
pub struct SessionJson<'a> {
    pub id: String,
    pub agent: Agent,
    pub cwd: Option<&'a Path>,
    pub cwds: &'a [PathBuf],
    pub path: &'a Path,
    pub title: Option<&'a crate::model::Title>,
    pub git_branch: Option<&'a str>,
    pub model: Option<&'a str>,
    pub started: Timestamp,
    pub updated: Timestamp,
    pub prompt_count: usize,
    pub record_count: usize,
    pub parent: Option<String>,
    pub child_kind: Option<&'a str>,
    pub forked_from: Option<String>,
    pub continued_in: Option<String>,
    pub current: bool,
}

impl<'a> SessionJson<'a> {
    pub fn new(catalog: &'a Catalog, i: usize, current: Option<usize>) -> Self {
        let e = &catalog.entries[i];
        let m = &e.meta;
        SessionJson {
            id: m.display_id(),
            agent: m.agent,
            cwd: e.cwd(),
            cwds: &e.cwds,
            path: &m.path,
            title: m.title.as_ref(),
            git_branch: m.git_branch.as_deref(),
            model: m.model.as_deref(),
            started: m.started,
            updated: m.updated,
            prompt_count: m.prompt_count,
            record_count: m.record_count,
            parent: m.parent_id.as_ref().map(|p| format!("{}:{p}", m.agent)),
            child_kind: m.child_kind.as_deref(),
            forked_from: e.forked_from.map(|f| catalog.entries[f].meta.display_id()).or_else(|| m.forked_from.clone()),
            continued_in: m.continued_in.as_ref().map(|c| format!("{}:{c}", m.agent)),
            current: current == Some(i),
        }
    }
}
