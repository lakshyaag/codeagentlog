//! Detect the caller's own session so listings can mark it `(current)`.

use std::path::Path;

use jiff::Timestamp;

use crate::catalog::Catalog;
use crate::model::Agent;
use crate::scope;

/// Env vars agents expose to child shells.
const ID_VARS: &[(&str, Agent)] = &[
    ("PI_SESSION_ID", Agent::Pi),
    ("CLAUDE_CODE_SESSION_ID", Agent::Claude),
    ("CODEX_THREAD_ID", Agent::Codex),
    ("CURSOR_CONVERSATION_ID", Agent::Cursor),
];

/// Env vars that only tell us which agent is running.
const MARKER_VARS: &[(&str, Agent)] = &[
    ("PI_CODING_AGENT", Agent::Pi),
    ("CLAUDECODE", Agent::Claude),
    ("CODEX_SANDBOX", Agent::Codex),
    ("CODEX_CI", Agent::Codex),
    ("CURSOR_AGENT", Agent::Cursor),
];

pub fn detect(catalog: &Catalog, cwd: &Path) -> Option<usize> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());

    for &(var, agent) in ID_VARS {
        if let Some(id) = env(var)
            // Cursor subagents get their own conversation id; ours is `<parent>/<child>`.
            && let Some(i) = catalog.entries.iter().position(|e| {
                e.meta.agent == agent && (e.meta.id == id || e.meta.id.strip_suffix(&id).is_some_and(|p| p.ends_with('/')))
            })
        {
            return Some(i);
        }
    }
    if let Some(file) = env("PI_SESSION_FILE")
        && let Some(i) = catalog.by_path(Path::new(&file))
    {
        return Some(i);
    }

    // Fallback: newest session of the running agent in this folder, touched in the last minute.
    let agent = MARKER_VARS.iter().find(|(var, _)| env(var).is_some()).map(|&(_, a)| a)?;
    let cwd = scope::canonical(cwd);
    let cutoff = Timestamp::now().as_second() - 60;
    catalog
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.meta.agent == agent && !e.meta.is_child() && e.cwds.contains(&cwd))
        .filter(|(_, e)| e.meta.updated.as_second() >= cutoff)
        .max_by_key(|(_, e)| e.meta.updated)
        .map(|(i, _)| i)
}
