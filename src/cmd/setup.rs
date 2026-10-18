//! `setup`: install the agent skill (`skills/codeagentlog/SKILL.md`) where agents look for it.
//! The only command that writes outside the cache, and only with explicit approval: `--yes`,
//! or a "y" at the prompt on a terminal. Without either it lists what it would write and stops.

use std::fmt::Write as _;
use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::guide::SKILL;
use super::{Ctx, to_json};
use crate::model::Agent;
use crate::style;
use crate::text::{home_dir, tilde};

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Missing,
    Outdated,
    Current,
}

#[derive(Serialize)]
pub struct Target {
    pub path: PathBuf,
    /// Agents that read skills from this folder.
    pub agents: Vec<Agent>,
    pub state: State,
}

/// Claude Code's skills folder, and the shared `~/.agents/skills` that Codex, pi and Cursor
/// read (Cursor reads both). Only folders for agents found on this machine.
pub fn targets(ctx: &Ctx) -> Vec<Target> {
    let present = |a: Agent| ctx.roots.home(a).is_dir();
    let mut dirs = Vec::new();
    if present(Agent::Claude) {
        let agents = [Agent::Claude, Agent::Cursor].into_iter().filter(|&a| present(a)).collect();
        dirs.push((ctx.roots.home(Agent::Claude).join("skills"), agents));
    }
    let shared: Vec<Agent> = [Agent::Codex, Agent::Pi, Agent::Cursor].into_iter().filter(|&a| present(a)).collect();
    if !shared.is_empty()
        && let Some(home) = home_dir()
    {
        dirs.push((home.join(".agents/skills"), shared));
    }
    dirs.into_iter()
        .map(|(dir, agents)| {
            let path = dir.join("codeagentlog/SKILL.md");
            let state = match std::fs::read_to_string(&path) {
                Ok(s) if s == SKILL => State::Current,
                Ok(_) => State::Outdated,
                Err(_) => State::Missing,
            };
            Target { path, agents, state }
        })
        .collect()
}

pub fn run(ctx: &Ctx, yes: bool, remove: bool) -> Result<String> {
    let targets = targets(ctx);
    if targets.is_empty() {
        bail!("no supported agent found (pi, Claude Code, Codex, Cursor); see `codeagentlog doctor`");
    }
    let todo: Vec<&Target> =
        targets.iter().filter(|t| if remove { t.state != State::Missing } else { t.state != State::Current }).collect();
    let interactive = !ctx.json && !yes && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    let approved = !todo.is_empty() && (yes || (interactive && confirm(&todo, remove)?));
    if approved {
        for t in &todo {
            apply(t, remove)?;
        }
    }

    if ctx.json {
        return to_json(&serde_json::json!({
            "action": if remove { "remove" } else { "install" },
            "targets": targets,
            "written": approved,
            "needs_approval": !todo.is_empty() && !approved,
        }));
    }
    let mut out = String::new();
    if todo.is_empty() {
        let msg = if remove { "The skill isn't installed." } else { "The skill is installed and up to date:" };
        writeln!(out, "{msg}")?;
        for t in targets.iter().filter(|t| t.state != State::Missing) {
            writeln!(out, "  {}", line(t, "current"))?;
        }
        return Ok(out);
    }
    if approved {
        for t in &todo {
            writeln!(out, "{}", line(t, done(t, remove)))?;
        }
        if remove {
            writeln!(out, "Removed. Nothing else was changed.")?;
        } else {
            writeln!(out, "Agents pick up the skill in new sessions. Undo with `codeagentlog setup --remove`.")?;
        }
    } else if interactive {
        writeln!(out, "Nothing was written.")?;
    } else {
        out.push_str(&plan(&todo, remove));
        let cmd = if remove { "codeagentlog setup --remove --yes" } else { "codeagentlog setup --yes" };
        writeln!(out, "Nothing was written. Agents: show the user this list and ask for approval before running")?;
        writeln!(out, "`{cmd}`, which changes exactly these files and nothing else.")?;
    }
    Ok(out)
}

fn plan(todo: &[&Target], remove: bool) -> String {
    let mut out = String::new();
    let what = if remove { "remove its agent skill" } else { "install its agent skill" };
    let _ = writeln!(out, "codeagentlog setup will {what}:");
    for t in todo {
        let _ = writeln!(out, "  {}", line(t, verb(t, remove)));
    }
    out
}

fn line(t: &Target, action: &str) -> String {
    let agents: Vec<&str> = t.agents.iter().map(|a| a.name()).collect();
    format!("{action:<7}  {}  {}", style::path(&tilde(&t.path)), style::dim(&format!("({})", agents.join(", "))))
}

fn verb(t: &Target, remove: bool) -> &'static str {
    match (remove, t.state) {
        (true, _) => "remove",
        (false, State::Outdated) => "update",
        (false, _) => "create",
    }
}

fn done(t: &Target, remove: bool) -> &'static str {
    match verb(t, remove) {
        "remove" => "removed",
        "update" => "updated",
        _ => "created",
    }
}

/// Ask on the terminal. Anything but y/yes is a no.
fn confirm(todo: &[&Target], remove: bool) -> Result<bool> {
    let mut err = std::io::stderr();
    write!(err, "{}{} [y/N] ", plan(todo, remove), if remove { "Remove?" } else { "Write these files?" })?;
    err.flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn apply(t: &Target, remove: bool) -> Result<()> {
    let dir = t.path.parent().unwrap_or(Path::new("."));
    if remove {
        std::fs::remove_file(&t.path).with_context(|| format!("removing {}", t.path.display()))?;
        let _ = std::fs::remove_dir(dir); // only if now empty
    } else {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::write(&t.path, SKILL).with_context(|| format!("writing {}", t.path.display()))?;
    }
    Ok(())
}
