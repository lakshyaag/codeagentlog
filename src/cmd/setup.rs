//! `setup`: install the agent skill (`skills/codeagentlog/SKILL.md`) where agents look for it.
//! The only command that writes outside the cache, and only with explicit approval: `--yes`,
//! or a "y" at the prompt on a terminal. Without either it lists what it would write and stops.

use std::fmt::Write as _;
use std::io::IsTerminal;
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
    if !ctx.json && std::io::stdin().is_terminal() && std::io::stderr().is_terminal() {
        return walkthrough(&targets, &todo, yes, remove);
    }

    let approved = !todo.is_empty() && yes;
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
    } else if approved {
        for t in &todo {
            writeln!(out, "{}", line(t, done(t, remove)))?;
        }
        if remove {
            writeln!(out, "Removed. Nothing else was changed.")?;
        } else {
            writeln!(out, "Agents pick up the skill in new sessions. Undo with `codeagentlog setup --remove`.")?;
        }
    } else {
        let what = if remove { "remove its agent skill" } else { "install its agent skill" };
        writeln!(out, "codeagentlog setup will {what}:")?;
        for t in &todo {
            writeln!(out, "  {}", line(t, verb(t, remove)))?;
        }
        let cmd = if remove { "codeagentlog setup --remove --yes" } else { "codeagentlog setup --yes" };
        writeln!(out, "Nothing was written. Agents: show the user this list and ask for approval before running")?;
        writeln!(out, "`{cmd}`, which changes exactly these files and nothing else.")?;
    }
    Ok(out)
}

/// On a terminal: a short walkthrough on stderr, then one yes/no (skipped with `--yes`).
fn walkthrough(targets: &[Target], todo: &[&Target], yes: bool, remove: bool) -> Result<String> {
    use cliclack::{confirm, intro, log, note, outro, outro_cancel};
    intro(if remove { "codeagentlog setup --remove" } else { "codeagentlog setup" })?;
    if todo.is_empty() {
        if remove {
            outro("The skill isn't installed, so there's nothing to remove.")?;
        } else {
            let installed: Vec<String> = targets.iter().map(|t| line(t, "")).collect();
            note("Installed", installed.join("\n"))?;
            outro("The skill is already installed and up to date.")?;
        }
        return Ok(String::new());
    }
    let files: Vec<String> = todo.iter().map(|t| line(t, verb(t, remove))).collect();
    if remove {
        note("Remove the skill", format!("These files will be deleted:\n\n{}", files.join("\n")))?;
    } else {
        let mut found: Vec<Agent> = targets.iter().flat_map(|t| t.agents.iter().copied()).collect();
        found.sort();
        found.dedup();
        log::info(format!("Found {}.", found.iter().map(|&a| label(a)).collect::<Vec<_>>().join(", ")))?;
        note(
            "Install the skill",
            format!(
                "The skill teaches your agents to find and read past sessions\nwith codeagentlog. It will be written to:\n\n{}",
                files.join("\n")
            ),
        )?;
    }
    let question = if remove { "Remove these files?" } else { "Write these files?" };
    if !(yes || confirm(question).initial_value(false).interact().unwrap_or(false)) {
        outro_cancel("Nothing was changed.")?;
        return Ok(String::new());
    }
    for t in todo {
        apply(t, remove)?;
        log::success(line(t, done(t, remove)))?;
    }
    outro(if remove {
        "Removed. Nothing else was changed."
    } else {
        "Done. Start a new agent session to use it. Undo with `codeagentlog setup --remove`."
    })?;
    Ok(String::new())
}

/// `<action>  <path>  (<agents>)`
fn line(t: &Target, action: &str) -> String {
    let agents: Vec<&str> = t.agents.iter().map(|&a| label(a)).collect();
    let action = if action.is_empty() { String::new() } else { format!("{action:<7}  ") };
    format!("{action}{}  {}", style::path(&tilde(&t.path)), style::dim(&format!("({})", agents.join(", "))))
}

fn label(a: Agent) -> &'static str {
    match a {
        Agent::Pi => "pi",
        Agent::Claude => "Claude Code",
        Agent::Codex => "Codex",
        Agent::Cursor => "Cursor",
    }
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
