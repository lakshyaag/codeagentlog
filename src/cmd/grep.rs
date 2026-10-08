use std::fmt::Write;

use anyhow::{Context, Result};
use rayon::prelude::*;
use regex::{Regex, RegexBuilder};
use serde::Serialize;

use super::{Ctx, display_cwd, to_json};
use crate::branches::Branches;
use crate::catalog::Catalog;
use crate::model::Role;
use crate::render::{excerpt, search_text};
use crate::scope::Scope;
use crate::text::shell_quote;
use crate::{GrepArgs, style, time};

/// Hits shown per session before summarizing the rest.
const HITS_PER_SESSION: usize = 5;
/// Records of context around a hit in the suggested `show` command.
const AROUND: usize = 5;

#[derive(Serialize)]
struct Hit {
    turn: usize,
    role: String,
    /// Excerpt split around the first match: (before, match, after).
    snippet: (String, String, String),
    /// Extra matches in the same record.
    more_in_turn: usize,
    /// `show` flags needed to see this hit (empty if visible by default).
    reveal: Vec<String>,
    #[serde(skip)]
    is_event: bool,
}

#[derive(Serialize)]
struct SessionHits {
    id: String,
    cwd: String,
    updated: jiff::Timestamp,
    prompts: usize,
    records: usize,
    child_of: Option<String>,
    current: bool,
    hits: Vec<Hit>,
    #[serde(skip)]
    agent: crate::model::Agent,
}

pub fn run(ctx: &Ctx, args: GrepArgs) -> Result<String> {
    let matcher = build_matcher(&args)?;
    let prefilter = build_prefilter(&args);
    let roles: Vec<Role> = args
        .role
        .iter()
        .map(|r| Role::parse(r).with_context(|| format!("unknown role {r:?}")))
        .collect::<Result<_>>()?;
    let base = ctx.base(args.path.as_deref());
    let scope = args.scope.scope();
    let since = args.filters.since()?;
    let catalog = ctx.catalog(&args.filters.agent);
    let current = crate::current::detect(&catalog, &ctx.cwd);

    let candidates: Vec<usize> = (0..catalog.entries.len())
        .filter(|&i| args.filters.allows(&catalog.entries[i], since) && catalog.in_scope(i, scope, &base))
        .collect();
    let search =
        Search { ctx, catalog: &catalog, matcher: &matcher, prefilter: prefilter.as_ref(), roles: &roles, args: &args };
    let mut results: Vec<SessionHits> = candidates
        .par_iter()
        .filter_map(|&i| {
            search.session(i, current == Some(i) || current.is_some_and(|c| catalog.entries[i].parent == Some(c)))
        })
        .collect();
    results.sort_by_key(|r| std::cmp::Reverse(r.updated));
    let total = results.len();
    results.truncate(args.limit);

    if ctx.json {
        return to_json(&serde_json::json!({ "total_sessions": total, "sessions": results }));
    }
    if results.is_empty() {
        let hint = if scope == Scope::All { "" } else { " (searched cwd and below; try --all)" };
        return Ok(format!("no matches for {:?}{hint}\n", args.pattern));
    }

    let mut out = String::new();
    for r in &results {
        write_session(&mut out, r)?;
    }
    if total > results.len() {
        let note = format!("{} of {total} sessions shown. more:", results.len());
        writeln!(out, "{}", style::footer(&note, Some(&more_command(&args, scope))))?;
    }
    Ok(out)
}

fn write_session(out: &mut String, r: &SessionHits) -> std::fmt::Result {
    let child = r.child_of.as_ref().map(|p| format!("  child of {p}")).unwrap_or_default();
    let current = if r.current { format!("  {}", style::bold("(current)")) } else { String::new() };
    let meta = format!("{}  {} prompts  {} records", time::date(r.updated), r.prompts, r.records);
    writeln!(out, "{}  {}  {}{child}{current}", style::agent(r.agent, &r.id), style::path(&r.cwd), style::dim(&meta))?;

    // Conversation hits first, then events; record order within each.
    let mut order: Vec<&Hit> = r.hits.iter().collect();
    order.sort_by_key(|h| (h.is_event, h.turn));
    for h in order.iter().take(HITS_PER_SESSION) {
        let (before, matched, after) = &h.snippet;
        let more = if h.more_in_turn > 0 { style::dim(&format!(" (+{} more)", h.more_in_turn)) } else { String::new() };
        let label = style::dim(&format!("[{}]", h.turn));
        let role = style::role(&Role::parse(h.role.split(' ').next().unwrap_or("")).unwrap_or(Role::Event), &h.role);
        writeln!(out, "  {label} {role}: {before}{}{after}{more}", style::highlight(matched))?;
        if !h.reveal.is_empty() {
            let cmd = format!("codeagentlog show {} --turns {} {}", r.id, h.turn, h.reveal.join(" "));
            writeln!(out, "      {} {}", style::dim("→"), style::bold(&cmd))?;
        }
    }
    if order.len() > HITS_PER_SESSION {
        let rest: Vec<String> = order[HITS_PER_SESSION..].iter().take(15).map(|h| h.turn.to_string()).collect();
        let ellipsis = if order.len() > HITS_PER_SESSION + 15 { ", …" } else { "" };
        let note = format!("… +{} more hits at [{}{ellipsis}]", order.len() - HITS_PER_SESSION, rest.join(", "));
        writeln!(out, "  {}", style::dim(&note))?;
    }
    if let Some(first) = order.first().filter(|h| h.reveal.is_empty()) {
        let cmd = format!("codeagentlog show {} --turns {}~{AROUND}", r.id, first.turn);
        writeln!(out, "  {} {}", style::dim("→"), style::bold(&cmd))?;
    }
    writeln!(out)
}

fn more_command(args: &GrepArgs, scope: Scope) -> String {
    let mut cmd = vec!["codeagentlog grep".to_string(), shell_quote(&args.pattern)];
    if let Some(p) = &args.path {
        cmd.push(shell_quote(&p.to_string_lossy()));
    }
    if scope != Scope::Under {
        cmd.push(scope.flag().into());
    }
    cmd.extend(args.filters.to_args());
    for (on, flag) in [(args.ignore_case, "-i"), (args.regex, "--regex"), (args.events, "--events")] {
        if on {
            cmd.push(flag.into());
        }
    }
    if !args.role.is_empty() {
        cmd.push(format!("--role {}", args.role.join(",")));
    }
    cmd.push(format!("--limit {}", args.limit * 2));
    cmd.join(" ")
}

struct Search<'a> {
    ctx: &'a Ctx,
    catalog: &'a Catalog,
    matcher: &'a Regex,
    prefilter: Option<&'a regex::bytes::Regex>,
    roles: &'a [Role],
    args: &'a GrepArgs,
}

impl Search<'_> {
    fn session(&self, i: usize, current: bool) -> Option<SessionHits> {
        let e = &self.catalog.entries[i];
        if let Some(pre) = self.prefilter {
            let bytes = std::fs::read(&e.meta.path).ok()?;
            if !pre.is_match(&bytes) {
                return None;
            }
        }
        let session = self.ctx.roots.adapter(e.meta.agent).load(&e.meta.path).ok()?;
        let branches = Branches::of(&session.turns);
        let active = branches.as_ref().map(|b| b.path(b.active_leaf()));

        let mut hits = Vec::new();
        for t in &session.turns {
            if (t.aux && !self.args.events) || (!self.roles.is_empty() && !self.roles.contains(&t.role)) {
                continue;
            }
            let text = search_text(t);
            let (body, in_thinking) = if self.matcher.is_match(&text.visible) {
                (&text.visible, false)
            } else if self.matcher.is_match(&text.thinking) {
                (&text.thinking, true)
            } else {
                continue;
            };
            let mut found = self.matcher.find_iter(body);
            let first = found.next().expect("is_match");

            let mut reveal = Vec::new();
            if in_thinking {
                reveal.push("--thinking".to_string());
            }
            if t.aux {
                reveal.push("--events".into());
            }
            if let (Some(b), Some(active), Some(node)) = (&branches, &active, &t.node)
                && !active.contains(&node.id)
            {
                reveal.push(format!("--branch {}", b.leaf_containing(&node.id).unwrap_or(&node.id)));
            }
            hits.push(Hit {
                turn: t.idx,
                role: if in_thinking { format!("{} thinking", t.role) } else { t.role.to_string() },
                snippet: excerpt(body, first.start(), first.end(), self.args.context),
                more_in_turn: found.count(),
                reveal,
                is_event: t.role == Role::Event,
            });
        }
        if hits.is_empty() {
            return None;
        }
        let m = &e.meta;
        Some(SessionHits {
            id: m.display_id(),
            cwd: display_cwd(e.parent.map_or(e, |p| &self.catalog.entries[p])),
            updated: m.updated,
            prompts: m.prompt_count,
            records: m.record_count,
            child_of: e.parent.map(|p| self.catalog.entries[p].meta.display_id()),
            current,
            hits,
            agent: m.agent,
        })
    }
}

fn build_matcher(args: &GrepArgs) -> Result<Regex> {
    let pattern = if args.regex { args.pattern.clone() } else { regex::escape(&args.pattern) };
    RegexBuilder::new(&pattern).case_insensitive(args.ignore_case).build().context("invalid pattern")
}

/// Raw-byte prefilter over the JSONL file. Skipped (`None`) when the pattern
/// can't be matched reliably against JSON-encoded bytes: escapes change quotes,
/// backslashes, control chars and (sometimes) non-ASCII; anchors see JSON lines.
fn build_prefilter(args: &GrepArgs) -> Option<regex::bytes::Regex> {
    let p = &args.pattern;
    let json_escaped = p.chars().any(|c| c == '"' || c == '\\' || c.is_control() || !c.is_ascii());
    let anchored = args.regex && (p.contains('^') || p.contains('$'));
    if json_escaped || anchored {
        return None;
    }
    let pattern = if args.regex { p.clone() } else { regex::escape(p) };
    regex::bytes::RegexBuilder::new(&pattern).case_insensitive(args.ignore_case).unicode(false).build().ok()
}
