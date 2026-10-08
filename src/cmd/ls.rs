use std::collections::BTreeMap;
use std::fmt::Write;

use anyhow::Result;

use super::{Ctx, SessionJson, display_cwd, session_summary, to_json};
use crate::catalog::Catalog;
use crate::text::{shell_quote, tilde};
use crate::{Group, LsArgs};

pub fn run(ctx: &Ctx, args: LsArgs) -> Result<String> {
    let base = ctx.base(args.path.as_deref());
    let scope = args.scope.scope();
    let since = args.filters.since()?;
    let catalog = ctx.catalog(&args.filters.agent);
    let current = crate::current::detect(&catalog, &ctx.cwd);

    let mut found: Vec<usize> = (0..catalog.entries.len())
        .filter(|&i| catalog.entries[i].parent.is_none())
        .filter(|&i| args.filters.allows(&catalog.entries[i], since) && catalog.in_scope(i, scope, &base))
        .collect();
    found.sort_by_key(|&i| std::cmp::Reverse(catalog.entries[i].meta.updated));
    let total = found.len();
    found.truncate(args.limit);

    if ctx.json {
        let sessions: Vec<_> = found
            .iter()
            .map(|&i| {
                let children: Vec<_> =
                    catalog.entries[i].children.iter().map(|&c| SessionJson::new(&catalog, c, current)).collect();
                serde_json::json!({ "session": SessionJson::new(&catalog, i, current), "children": children })
            })
            .collect();
        return to_json(&serde_json::json!({ "total": total, "sessions": sessions }));
    }

    if found.is_empty() {
        return Ok(format!(
            "no sessions {} {}{}\ntry: codeagentlog ls --near, codeagentlog ls --all, or codeagentlog tree\n",
            if scope == crate::scope::Scope::All { "found" } else { "in" },
            if scope == crate::scope::Scope::All { String::new() } else { tilde(&base) },
            if since.is_some() { " (with --since)" } else { "" },
        ));
    }

    let printer = Printer { catalog: &catalog, current, prompts: args.prompts, expand_children: args.children };
    let mut out = String::new();
    match args.group {
        Group::Agent => {
            for agent in crate::model::Agent::ALL {
                let mine: Vec<usize> =
                    found.iter().copied().filter(|&i| catalog.entries[i].meta.agent == agent).collect();
                if mine.is_empty() {
                    continue;
                }
                writeln!(out, "{agent}")?;
                for (folder, sessions) in by_folder(&catalog, &mine) {
                    writeln!(out, "  {folder}")?;
                    sessions.iter().for_each(|&i| printer.session(&mut out, i, "    "));
                }
            }
        }
        Group::Folder => {
            for (folder, sessions) in by_folder(&catalog, &found) {
                writeln!(out, "{folder}")?;
                sessions.iter().for_each(|&i| printer.session(&mut out, i, "  "));
            }
        }
        Group::None => {
            for &i in &found {
                writeln!(out, "{}", display_cwd(&catalog.entries[i]))?;
                printer.session(&mut out, i, "  ");
            }
        }
    }

    if total > found.len() {
        let mut cmd = vec!["codeagentlog ls".to_string()];
        if let Some(p) = &args.path {
            cmd.push(shell_quote(&p.to_string_lossy()));
        }
        if scope != crate::scope::Scope::Under {
            cmd.push(scope.flag().into());
        }
        cmd.extend(args.filters.to_args());
        cmd.push(format!("--limit {}", (args.limit * 2).max(args.limit + 20)));
        writeln!(out, "\n-- showing {} of {total}. more: {}", found.len(), cmd.join(" "))?;
    }
    Ok(out)
}

/// Group sessions by display folder, folders ordered by their most recent session.
fn by_folder(catalog: &Catalog, sessions: &[usize]) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for &i in sessions {
        let folder = display_cwd(&catalog.entries[i]);
        match groups.iter_mut().find(|(f, _)| *f == folder) {
            Some((_, v)) => v.push(i),
            None => groups.push((folder, vec![i])),
        }
    }
    groups
}

struct Printer<'a> {
    catalog: &'a Catalog,
    current: Option<usize>,
    prompts: usize,
    expand_children: bool,
}

impl Printer<'_> {
    fn session(&self, out: &mut String, i: usize, indent: &str) {
        let e = &self.catalog.entries[i];
        let m = &e.meta;
        let mut line = format!("{indent}{}", session_summary(e));
        if let Some(b) = &m.git_branch {
            line += &format!("  {b}");
        }
        if !e.children.is_empty() && !self.expand_children {
            line += &format!("  +{} children ({})", e.children.len(), self.child_kinds(&e.children));
        }
        if self.current == Some(i) {
            line += "  (current)";
        }
        let _ = writeln!(out, "{line}");
        if let Some(t) = &m.title {
            let _ = writeln!(out, "{indent}  \"{}\"  ({})", t.text, t.source);
        }
        if let Some(f) = e.forked_from {
            let _ = writeln!(out, "{indent}  forked from {}", self.catalog.entries[f].meta.display_id());
        }
        if let Some(c) = &m.continued_in {
            let _ = writeln!(out, "{indent}  continued in {}:{c}", m.agent);
        }
        for p in m.first_prompts.iter().take(self.prompts) {
            let _ = writeln!(out, "{indent}  > {p}");
        }
        if self.expand_children {
            for &c in &e.children {
                let child = &self.catalog.entries[c];
                let kind = child.meta.child_kind.as_deref().unwrap_or("child");
                let title = child.meta.title.as_ref().map(|t| format!("  \"{}\"", t.text)).unwrap_or_default();
                let _ = writeln!(out, "{indent}  ↳ {}  {kind}{title}", session_summary(child));
            }
        }
    }

    /// `guardian×10, subagent×2`
    fn child_kinds(&self, children: &[usize]) -> String {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for &c in children {
            *counts.entry(self.catalog.entries[c].meta.child_kind.as_deref().unwrap_or("child")).or_default() += 1;
        }
        counts.iter().map(|(k, n)| format!("{k}×{n}")).collect::<Vec<_>>().join(", ")
    }
}
