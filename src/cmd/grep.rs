use std::fmt::Write;

use anyhow::{Context, Result};
use rayon::prelude::*;
use regex::{Regex, RegexBuilder};
use serde::Serialize;

use super::{Ctx, display_cwd, to_json};
use crate::GrepArgs;
use crate::branches::Branches;
use crate::catalog::Catalog;
use crate::model::Role;
use crate::render::{excerpt, search_text};
use crate::text::shell_quote;
use crate::time;

/// Hits shown per session before summarizing the rest.
const HITS_PER_SESSION: usize = 5;

#[derive(Serialize)]
struct Hit {
    turn: usize,
    role: String,
    snippet: String,
    /// Extra matches in the same record.
    more_in_turn: usize,
    /// `show` flags needed to see this hit (empty if visible by default).
    reveal: Vec<String>,
}

#[derive(Serialize)]
struct SessionHits {
    id: String,
    cwd: String,
    updated: jiff::Timestamp,
    prompts: usize,
    records: usize,
    child_of: Option<String>,
    hits: Vec<Hit>,
}

pub fn run(ctx: &Ctx, args: GrepArgs) -> Result<String> {
    let matcher = build_matcher(&args)?;
    let prefilter = build_prefilter(&args)?;
    let roles: Vec<Role> = args
        .role
        .iter()
        .map(|r| Role::parse(r).with_context(|| format!("unknown role {r:?}")))
        .collect::<Result<_>>()?;
    let base = ctx.base(args.path.as_deref());
    let scope = args.scope.scope();
    let since = args.filters.since()?;
    let catalog = ctx.catalog(&args.filters.agent);

    let candidates: Vec<usize> = (0..catalog.entries.len())
        .filter(|&i| args.filters.allows(&catalog.entries[i], since) && catalog.in_scope(i, scope, &base))
        .collect();

    let mut results: Vec<SessionHits> = candidates
        .par_iter()
        .filter_map(|&i| search_session(ctx, &catalog, i, &matcher, prefilter.as_ref(), &roles, &args))
        .collect();
    results.sort_by_key(|r| std::cmp::Reverse(r.updated));
    let total = results.len();
    results.truncate(args.limit);

    if ctx.json {
        return to_json(&serde_json::json!({ "total_sessions": total, "sessions": results }));
    }
    if results.is_empty() {
        let hint = if scope == crate::scope::Scope::All { "" } else { " (searched cwd and below; try --all)" };
        return Ok(format!("no matches for {:?}{hint}\n", args.pattern));
    }

    let mut out = String::new();
    for r in &results {
        let child = r.child_of.as_ref().map(|p| format!("  child of {p}")).unwrap_or_default();
        writeln!(
            out,
            "{}  {}  {}  {} prompts  {} records{child}",
            r.id,
            r.cwd,
            time::date(r.updated),
            r.prompts,
            r.records
        )?;
        for h in r.hits.iter().take(HITS_PER_SESSION) {
            let more = if h.more_in_turn > 0 { format!(" (+{} more)", h.more_in_turn) } else { String::new() };
            writeln!(out, "  [{}] {}: {}{more}", h.turn, h.role, h.snippet)?;
            if !h.reveal.is_empty() {
                writeln!(out, "      → codeagentlog show {} --turns {} {}", r.id, h.turn, h.reveal.join(" "))?;
            }
        }
        if r.hits.len() > HITS_PER_SESSION {
            let rest: Vec<String> = r.hits[HITS_PER_SESSION..].iter().take(15).map(|h| h.turn.to_string()).collect();
            let ellipsis = if r.hits.len() > HITS_PER_SESSION + 15 { ", …" } else { "" };
            writeln!(out, "  … +{} more hits at [{}{ellipsis}]", r.hits.len() - HITS_PER_SESSION, rest.join(", "))?;
        }
        out.push('\n');
    }
    if total > results.len() {
        let mut cmd = vec!["codeagentlog grep".to_string(), shell_quote(&args.pattern)];
        if let Some(p) = &args.path {
            cmd.push(shell_quote(&p.to_string_lossy()));
        }
        if scope != crate::scope::Scope::Under {
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
        writeln!(out, "-- {} of {total} sessions shown. more: {}", results.len(), cmd.join(" "))?;
    }
    Ok(out)
}

fn search_session(
    ctx: &Ctx,
    catalog: &Catalog,
    i: usize,
    matcher: &Regex,
    prefilter: Option<&regex::bytes::Regex>,
    roles: &[Role],
    args: &GrepArgs,
) -> Option<SessionHits> {
    let e = &catalog.entries[i];
    if let Some(pre) = prefilter {
        let bytes = std::fs::read(&e.meta.path).ok()?;
        if !pre.is_match(&bytes) {
            return None;
        }
    }
    let session = ctx.roots.adapter(e.meta.agent).load(&e.meta.path).ok()?;
    let branches = Branches::of(&session.turns);
    let active = branches.as_ref().map(|b| b.path(b.active_leaf()));

    let mut hits = Vec::new();
    for t in &session.turns {
        if (t.aux && !args.events) || (!roles.is_empty() && !roles.contains(&t.role)) {
            continue;
        }
        let text = search_text(t);
        let (body, in_thinking) = if matcher.is_match(&text.visible) {
            (&text.visible, false)
        } else if matcher.is_match(&text.thinking) {
            (&text.thinking, true)
        } else {
            continue;
        };
        let mut found = matcher.find_iter(body);
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
            snippet: excerpt(body, first.start(), first.end(), args.context),
            more_in_turn: found.count(),
            reveal,
        });
    }
    if hits.is_empty() {
        return None;
    }
    let m = &e.meta;
    Some(SessionHits {
        id: m.display_id(),
        cwd: display_cwd(e.parent.map_or(e, |p| &catalog.entries[p])),
        updated: m.updated,
        prompts: m.prompt_count,
        records: m.record_count,
        child_of: e.parent.map(|p| catalog.entries[p].meta.display_id()),
        hits,
    })
}

fn build_matcher(args: &GrepArgs) -> Result<Regex> {
    let pattern = if args.regex { args.pattern.clone() } else { regex::escape(&args.pattern) };
    RegexBuilder::new(&pattern).case_insensitive(args.ignore_case).build().context("invalid pattern")
}

/// Raw-byte prefilter over the JSONL file. Skipped (`None`) when the pattern
/// can't be matched reliably against JSON-encoded bytes: escapes change quotes,
/// backslashes, control chars and (sometimes) non-ASCII; anchors see JSON lines.
fn build_prefilter(args: &GrepArgs) -> Result<Option<regex::bytes::Regex>> {
    let p = &args.pattern;
    let json_escaped = p.chars().any(|c| c == '"' || c == '\\' || c.is_control() || !c.is_ascii());
    let anchored = args.regex && (p.contains('^') || p.contains('$'));
    if json_escaped || anchored {
        return Ok(None);
    }
    let pattern = if args.regex { p.clone() } else { regex::escape(p) };
    Ok(regex::bytes::RegexBuilder::new(&pattern).case_insensitive(args.ignore_case).unicode(false).build().ok())
}
