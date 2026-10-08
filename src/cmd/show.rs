use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::{Ctx, to_json};
use crate::ShowArgs;
use crate::branches::Branches;
use crate::catalog::Catalog;
use crate::model::{Session, Turn};
use crate::render::{Renderer, ToolsMode};
use crate::text::{tilde, truncate};
use crate::time;

/// Inclusive record range; open ends are `None`.
#[derive(Clone, Copy, Default)]
pub struct Range {
    pub start: Option<usize>,
    pub end: Option<usize>,
}

impl Range {
    pub fn parse(s: &str) -> Result<Range> {
        let num = |p: &str| -> Result<Option<usize>> {
            if p.is_empty() {
                Ok(None)
            } else {
                Ok(Some(p.trim().parse().with_context(|| format!("invalid --turns {s:?}"))?))
            }
        };
        match s.split_once("..") {
            Some((a, b)) => Ok(Range { start: num(a)?, end: num(b.trim_start_matches('='))? }),
            None => {
                let n = num(s)?;
                Ok(Range { start: n, end: n })
            }
        }
    }

    fn contains(&self, idx: usize) -> bool {
        self.start.is_none_or(|s| idx >= s) && self.end.is_none_or(|e| idx <= e)
    }
}

pub fn run(ctx: &Ctx, args: ShowArgs) -> Result<String> {
    let catalog = ctx.catalog(&[]);
    let (session, entry) = load(ctx, &catalog, &args.id)?;
    let branches = Branches::of(&session.turns);
    let branch_path = select_branch(branches.as_ref(), &args)?;
    let range = args.turns.as_deref().map(Range::parse).transpose()?.unwrap_or_default();

    let mut visible: Vec<&Turn> = session
        .turns
        .iter()
        .filter(|t| args.events || !t.aux)
        .filter(|t| match (&branch_path, &t.node) {
            (Some(path), Some(node)) => path.contains(&node.id),
            _ => true,
        })
        .filter(|t| range.contains(t.idx))
        .collect();
    if let Some(n) = args.tail {
        visible = visible.split_off(visible.len().saturating_sub(n));
    }

    let id = session.meta.display_id();
    let page = paginate(&session, &visible, &args, &catalog, entry);

    if ctx.json {
        let next = page.next.map(|n| next_command(&id, &args, n, range.end));
        let turns: Vec<&Turn> = visible.iter().copied().filter(|t| page.shown.contains(&t.idx)).collect();
        return to_json(&serde_json::json!({
            "session": entry.map(|i| super::SessionJson::new(&catalog, i, None)),
            "meta": &session.meta,
            "turns": turns,
            "next": next,
        }));
    }

    let mut out = header(&session, &catalog, entry);
    out.push('\n');
    out.push_str(&page.text);
    if visible.is_empty() {
        out.push_str("(no records in this selection)\n");
    }
    if let Some(next) = page.next {
        let total = session.turns.len();
        writeln!(
            out,
            "\n-- truncated at {} chars (record {next}/{total}). next: {}",
            args.max_chars,
            next_command(&id, &args, next, range.end)
        )?;
    }
    if let Some(b) = &branches
        && !b.other_leaves().is_empty()
        && !args.all_branches
    {
        let shown: Vec<String> = b.other_leaves().iter().rev().take(5).map(|l| format!("--branch {l}")).collect();
        let more = if b.other_leaves().len() > 5 { " …" } else { "" };
        writeln!(out, "-- {} other branches: {}{more} | --all-branches", b.other_leaves().len(), shown.join(" "))?;
    }
    Ok(out)
}

/// Load by id (via the catalog) or by file path (even outside known roots).
fn load(ctx: &Ctx, catalog: &Catalog, query: &str) -> Result<(Session, Option<usize>)> {
    let path = Path::new(query);
    let entry = match catalog.resolve(query) {
        Ok(i) => Some(i),
        Err(_) if path.is_file() => None,
        Err(e) => return Err(e),
    };
    let (file, adapter) = match entry {
        Some(i) => {
            let m = &catalog.entries[i].meta;
            (m.path.clone(), ctx.roots.adapter(m.agent))
        }
        None => (path.to_path_buf(), ctx.roots.adapter_for_path(path)),
    };
    let mut session = adapter.load(&file)?;
    if let Some(i) = entry {
        // Catalog metadata includes agent-wide decoration (e.g. Codex thread names).
        session.meta.title = catalog.entries[i].meta.title.clone();
    }
    Ok((session, entry))
}

/// Entry ids on the selected branch; `None` = no filtering.
fn select_branch(branches: Option<&Branches>, args: &ShowArgs) -> Result<Option<HashSet<String>>> {
    let Some(b) = branches else {
        if args.branch.is_some() {
            bail!("--branch only applies to tree-shaped sessions (pi)");
        }
        return Ok(None);
    };
    if args.all_branches {
        return Ok(None);
    }
    let leaf = match &args.branch {
        Some(prefix) => b.find(prefix).with_context(|| format!("no unique entry matches --branch {prefix}"))?,
        None => b.active_leaf().to_string(),
    };
    Ok(Some(b.path(&leaf)))
}

/// Smallest useful slice of an oversized record to print at the end of a page.
const MIN_CUT: usize = 500;

struct Page {
    text: String,
    shown: HashSet<usize>,
    /// First record not shown because of the budget.
    next: Option<usize>,
}

fn paginate(session: &Session, visible: &[&Turn], args: &ShowArgs, catalog: &Catalog, entry: Option<usize>) -> Page {
    let spawns = spawn_links(catalog, entry);
    let renderer = Renderer {
        tools: args.tools,
        thinking: args.thinking,
        spawns: &spawns,
        start_date: time::date(session.meta.started),
    };
    let positions: HashMap<&str, usize> =
        session.turns.iter().filter_map(|t| Some((t.node.as_ref()?.id.as_str(), t.idx))).collect();
    let id = session.meta.display_id();
    let budget = if args.max_chars == 0 { usize::MAX } else { args.max_chars };

    let mut page = Page { text: String::new(), shown: HashSet::new(), next: None };
    let mut used = 0;
    let mut prev_node: Option<&str> = None;
    for t in visible {
        let note = args.all_branches.then(|| branch_note(t, prev_node, &positions)).flatten();
        prev_node = t.node.as_ref().map(|n| n.id.as_str()).or(prev_node);
        let Some(mut text) = renderer.turn(t, note.as_deref()) else { continue };
        let len = text.chars().count();
        let room = budget.saturating_sub(used);
        // An oversized record is cut in place (here if there's useful room left,
        // otherwise at the top of the next page), so paging never loops.
        let cut = len > budget && (used == 0 || room >= MIN_CUT);
        if cut {
            let keep = room.min(budget);
            text = format!(
                "{}\n  … {} chars omitted; full: codeagentlog show {id} --turns {} --tools full --max-chars 0{}\n",
                truncate(&text, keep),
                len - keep,
                t.idx,
                if args.thinking { " --thinking" } else { "" }
            );
        } else if used > 0 && used + len > budget {
            page.next = Some(t.idx);
            break;
        }
        used += text.chars().count();
        page.text.push_str(&text);
        page.text.push('\n');
        page.shown.insert(t.idx);
    }
    page
}

/// With `--all-branches`, note when a record doesn't continue the previous one.
fn branch_note(t: &Turn, prev: Option<&str>, positions: &HashMap<&str, usize>) -> Option<String> {
    let parent = t.node.as_ref()?.parent.as_deref()?;
    (Some(parent) != prev).then(|| format!("branches from [{}]", positions.get(parent).copied().unwrap_or(0)))
}

/// Tool-call id → child display id, for marking spawn points in the parent.
fn spawn_links(catalog: &Catalog, entry: Option<usize>) -> HashMap<String, String> {
    let Some(i) = entry else { return HashMap::new() };
    catalog.entries[i]
        .children
        .iter()
        .filter_map(|&c| {
            let m = &catalog.entries[c].meta;
            Some((m.spawn_ref.clone()?, m.display_id()))
        })
        .collect()
}

fn header(session: &Session, catalog: &Catalog, entry: Option<usize>) -> String {
    let m = &session.meta;
    let mut out = String::new();
    let cwd = m.cwds.first().map(|c| tilde(c)).unwrap_or_else(|| "(unknown cwd)".into());
    let branch = m.git_branch.as_ref().map(|b| format!(" · {b}")).unwrap_or_default();
    let _ = writeln!(out, "# {} · {cwd}{branch}", m.display_id());
    let model = m.model.as_ref().map(|x| format!(" · model {x}")).unwrap_or_default();
    let _ = writeln!(
        out,
        "# {} · {} prompts · {} records{model}",
        time::span(m.started, m.updated),
        m.prompt_count,
        m.record_count
    );
    if let Some(t) = &m.title {
        let _ = writeln!(out, "# title \"{}\" ({})", t.text, t.source);
    }
    let _ = writeln!(out, "# file {}", tilde(&m.path));
    if let Some(parent) = &m.parent_id {
        let kind = m.child_kind.as_deref().unwrap_or("child");
        let _ = writeln!(out, "# {kind} of {}:{parent}", m.agent);
    }
    if let Some(i) = entry {
        let e = &catalog.entries[i];
        if let Some(f) = e.forked_from {
            let _ = writeln!(out, "# forked from {}", catalog.entries[f].meta.display_id());
        }
        if !e.children.is_empty() {
            let _ = writeln!(out, "# children ({}):", e.children.len());
            for &c in e.children.iter().take(10) {
                let cm = &catalog.entries[c].meta;
                let title = cm.title.as_ref().map(|t| format!(": \"{}\"", truncate(&t.text, 80))).unwrap_or_default();
                let _ =
                    writeln!(out, "#   {} ({}{title})", cm.display_id(), cm.child_kind.as_deref().unwrap_or("child"));
            }
            if e.children.len() > 10 {
                let _ = writeln!(out, "#   … +{} more: codeagentlog ls --all --children --json", e.children.len() - 10);
            }
        }
    }
    if let Some(c) = &m.continued_in {
        let _ = writeln!(out, "# continued in {}:{c}", m.agent);
    }
    out
}

/// The exact command that continues from record `next`, preserving flags.
fn next_command(id: &str, args: &ShowArgs, next: usize, end: Option<usize>) -> String {
    let mut cmd = format!("codeagentlog show {id} --turns {next}..{}", end.map(|e| e.to_string()).unwrap_or_default());
    if args.tools != ToolsMode::Brief {
        cmd += &format!(" --tools {}", if args.tools == ToolsMode::Full { "full" } else { "none" });
    }
    for (on, flag) in
        [(args.thinking, " --thinking"), (args.events, " --events"), (args.all_branches, " --all-branches")]
    {
        if on {
            cmd += flag;
        }
    }
    if let Some(b) = &args.branch {
        cmd += &format!(" --branch {b}");
    }
    if args.max_chars != 20000 {
        cmd += &format!(" --max-chars {}", args.max_chars);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges() {
        let r = Range::parse("3..7").unwrap();
        assert!(r.contains(3) && r.contains(7) && !r.contains(8));
        assert!(Range::parse("5..").unwrap().contains(500));
        assert!(!Range::parse("..5").unwrap().contains(6));
        let one = Range::parse("4").unwrap();
        assert!(one.contains(4) && !one.contains(5));
        assert!(Range::parse("x..").is_err());
    }
}
