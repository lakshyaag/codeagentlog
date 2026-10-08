use std::collections::BTreeMap;
use std::fmt::Write;

use anyhow::Result;
use serde::Serialize;

use super::{Ctx, to_json};
use crate::cache::Cache;
use crate::model::Agent;
use crate::text::tilde;
use crate::{style, time};

#[derive(Serialize)]
struct AgentReport {
    agent: Agent,
    dirs: Vec<(String, bool)>,
    files: usize,
    sessions: usize,
    children: usize,
    bad_lines: usize,
    failures: Vec<(String, String)>,
    unknown_types: BTreeMap<String, usize>,
    oldest: Option<jiff::Timestamp>,
    newest: Option<jiff::Timestamp>,
}

pub fn run(ctx: &Ctx) -> Result<String> {
    let catalog = ctx.catalog(&[]);
    let current = crate::current::detect(&catalog, &ctx.cwd);

    let reports: Vec<AgentReport> = Agent::ALL
        .iter()
        .map(|&agent| {
            let adapter = ctx.roots.adapter(agent);
            let mine: Vec<_> = catalog.entries.iter().filter(|e| e.meta.agent == agent).collect();
            let mut unknown = BTreeMap::new();
            for e in &mine {
                for (k, n) in &e.meta.diagnostics.unknown {
                    *unknown.entry(k.clone()).or_default() += n;
                }
            }
            AgentReport {
                agent,
                dirs: adapter.session_dirs().iter().map(|d| (tilde(d), d.is_dir())).collect(),
                files: catalog.file_counts.get(&agent).copied().unwrap_or(0),
                sessions: mine.iter().filter(|e| e.parent.is_none()).count(),
                children: mine.iter().filter(|e| e.parent.is_some()).count(),
                bad_lines: mine.iter().map(|e| e.meta.diagnostics.bad_lines).sum(),
                failures: catalog
                    .failures
                    .iter()
                    .filter(|f| f.agent == agent)
                    .map(|f| (tilde(&f.path), f.error.clone()))
                    .collect(),
                unknown_types: unknown,
                oldest: mine.iter().map(|e| e.meta.started).min(),
                newest: mine.iter().map(|e| e.meta.updated).max(),
            }
        })
        .collect();

    let cache_path = Cache::path();
    let cache_size = std::fs::metadata(&cache_path).map(|m| m.len()).ok();
    let current_id = current.map(|i| catalog.entries[i].meta.display_id());

    if ctx.json {
        return to_json(&serde_json::json!({
            "agents": reports,
            "cache": { "path": cache_path, "bytes": cache_size, "enabled": ctx.use_cache },
            "current": current_id,
        }));
    }

    let mut out = String::new();
    for r in &reports {
        let dirs: Vec<String> =
            r.dirs.iter().map(|(d, ok)| if *ok { d.clone() } else { format!("{d} (missing)") }).collect();
        writeln!(out, "{}  {}", style::agent(r.agent, r.agent.name()), style::path(&dirs.join(", ")))?;
        writeln!(
            out,
            "  files {}  sessions {}  children {}  bad lines {}  failures {}",
            r.files,
            r.sessions,
            r.children,
            r.bad_lines,
            r.failures.len()
        )?;
        if let (Some(o), Some(n)) = (r.oldest, r.newest) {
            writeln!(out, "  oldest {}  newest {}", time::datetime(o), time::datetime(n))?;
        }
        if !r.unknown_types.is_empty() {
            let list: Vec<String> = r.unknown_types.iter().map(|(k, n)| format!("{k}×{n}")).collect();
            writeln!(out, "  unknown record types: {}", list.join(", "))?;
        }
        for (path, err) in r.failures.iter().take(5) {
            writeln!(out, "  failed: {path}: {err}")?;
        }
    }
    let size = cache_size.map(|b| crate::render::human_bytes(b as usize)).unwrap_or_else(|| "absent".into());
    writeln!(out, "cache  {} ({size}{})", tilde(&cache_path), if ctx.use_cache { "" } else { ", disabled" })?;
    writeln!(out, "current  {}", current_id.as_deref().unwrap_or("(not detected)"))?;
    Ok(out)
}
