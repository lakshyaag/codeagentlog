//! All known sessions: discovered, metadata loaded (via cache), cwds
//! canonicalized, and parent/child + fork links resolved.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use rayon::prelude::*;

use crate::adapters::Roots;
use crate::cache::{Cache, Stamp};
use crate::model::{Agent, SessionMeta};
use crate::scope::{self, Scope};

pub struct Entry {
    pub meta: SessionMeta,
    /// Canonicalized cwds; the first is the display cwd.
    pub cwds: Vec<PathBuf>,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub forked_from: Option<usize>,
}

impl Entry {
    pub fn cwd(&self) -> Option<&Path> {
        self.cwds.first().map(PathBuf::as_path)
    }
}

pub struct Failure {
    pub agent: Agent,
    pub path: PathBuf,
    pub error: String,
}

pub struct Catalog {
    pub entries: Vec<Entry>,
    pub failures: Vec<Failure>,
    /// Session files discovered per agent.
    pub file_counts: HashMap<Agent, usize>,
}

impl Catalog {
    pub fn load(roots: &Roots, agents: &[Agent], use_cache: bool) -> Catalog {
        let mut cache = if use_cache { Cache::open() } else { None };
        let cached = cache.as_ref().map(Cache::load_all).unwrap_or_default();

        let mut metas = Vec::new();
        let mut failures = Vec::new();
        let mut file_counts = HashMap::new();
        let mut live = HashSet::new();

        for &agent in agents {
            let adapter = roots.adapter(agent);
            let files = adapter.discover();
            file_counts.insert(agent, files.len());
            live.extend(files.iter().cloned());

            let results: Vec<Loaded> = files
                .into_par_iter()
                .map(|path| {
                    let stamp = Stamp::of(&path);
                    match cached.get(&path) {
                        Some((s, meta)) if Some(*s) == stamp => Loaded::Cached(meta.clone()),
                        _ => match adapter.load(&path) {
                            Ok(session) => Loaded::Fresh(stamp, session.meta),
                            Err(e) => Loaded::Failed(Failure { agent, path, error: format!("{e:#}") }),
                        },
                    }
                })
                .collect();

            let mut agent_metas = Vec::with_capacity(results.len());
            let mut fresh = Vec::new();
            for r in results {
                match r {
                    Loaded::Cached(meta) => agent_metas.push(meta),
                    Loaded::Fresh(stamp, meta) => {
                        if let Some(stamp) = stamp {
                            fresh.push((stamp, meta.clone()));
                        }
                        agent_metas.push(meta);
                    }
                    Loaded::Failed(f) => failures.push(f),
                }
            }
            if let Some(cache) = cache.as_mut() {
                cache.update(&fresh);
            }
            // Runs after caching: it depends on agent-wide state outside the file.
            adapter.decorate(&mut agent_metas);
            metas.extend(agent_metas);
        }

        if let Some(cache) = cache.as_mut() {
            let stale: Vec<PathBuf> = cached
                .iter()
                .filter(|(p, (_, m))| agents.contains(&m.agent) && !live.contains(*p))
                .map(|(p, _)| p.clone())
                .collect();
            cache.remove(&stale);
        }

        let mut catalog = Catalog { entries: canonicalize(metas), failures, file_counts };
        catalog.link();
        catalog
    }

    fn link(&mut self) {
        let by_id: HashMap<(Agent, &str), usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.meta.is_child())
            .map(|(i, e)| ((e.meta.agent, e.meta.id.as_str()), i))
            .collect();
        let by_path: HashMap<&Path, usize> =
            self.entries.iter().enumerate().map(|(i, e)| (e.meta.path.as_path(), i)).collect();

        let links: Vec<(Option<usize>, Option<usize>)> = self
            .entries
            .iter()
            .map(|e| {
                let parent = e.meta.parent_id.as_deref().and_then(|p| by_id.get(&(e.meta.agent, p)).copied());
                let fork = e.meta.forked_from.as_deref().and_then(|f| by_path.get(Path::new(f)).copied());
                (parent, fork)
            })
            .collect();
        for (i, (parent, fork)) in links.into_iter().enumerate() {
            self.entries[i].parent = parent;
            self.entries[i].forked_from = fork;
            if let Some(p) = parent {
                self.entries[p].children.push(i);
            }
        }
        for i in 0..self.entries.len() {
            let mut kids = std::mem::take(&mut self.entries[i].children);
            kids.sort_by_key(|&k| self.entries[k].meta.started);
            self.entries[i].children = kids;
        }
    }

    /// Entry whose transcript is `path`.
    pub fn by_path(&self, path: &Path) -> Option<usize> {
        self.entries.iter().position(|e| e.meta.path == path).or_else(|| {
            let canon = scope::canonical(path);
            self.entries.iter().position(|e| scope::canonical(&e.meta.path) == canon)
        })
    }

    /// Does the entry (or, for children, its parent) live in scope?
    pub fn in_scope(&self, i: usize, scope: Scope, base: &Path) -> bool {
        let owner = &self.entries[self.entries[i].parent.unwrap_or(i)];
        owner.cwds.iter().any(|c| scope.matches(base, c))
    }

    /// Resolve `agent:prefix`, a bare prefix, or a file path.
    pub fn resolve(&self, query: &str) -> Result<usize> {
        if Path::new(query).is_file() {
            return match self.by_path(Path::new(query)) {
                Some(i) => Ok(i),
                None => {
                    bail!("{query} is not a known session file (use `codeagentlog show {query}` to read it directly)")
                }
            };
        }
        let (agent, prefix) = match query.split_once(':').and_then(|(a, rest)| Some((Agent::from_name(a)?, rest))) {
            Some((a, rest)) => (Some(a), rest),
            None => (None, query),
        };
        if prefix.is_empty() {
            bail!("empty session id");
        }
        let candidates: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| agent.is_none_or(|a| e.meta.agent == a))
            // Claude child ids extend the parent's (`<sid>/<agentId>`); match them only when asked.
            .filter(|(_, e)| e.meta.id.starts_with(prefix) && (prefix.contains('/') || !e.meta.id.contains('/')))
            .map(|(i, _)| i)
            .collect();
        if let Some(&exact) = candidates.iter().find(|&&i| self.entries[i].meta.id == prefix) {
            return Ok(exact);
        }
        match candidates.as_slice() {
            [one] => Ok(*one),
            [] => bail!("no session matches {query:?} (try `codeagentlog ls --all` or `codeagentlog grep`)"),
            many => {
                let list: Vec<String> =
                    many.iter().take(20).map(|&i| format!("  {}", self.entries[i].meta.display_id())).collect();
                bail!("{query:?} is ambiguous ({} matches):\n{}", many.len(), list.join("\n"))
            }
        }
    }
}

enum Loaded {
    Cached(SessionMeta),
    Fresh(Option<Stamp>, SessionMeta),
    Failed(Failure),
}

/// Canonicalize every distinct cwd once.
fn canonicalize(metas: Vec<SessionMeta>) -> Vec<Entry> {
    let distinct: HashSet<&PathBuf> = metas.iter().flat_map(|m| &m.cwds).collect();
    let canon: HashMap<PathBuf, PathBuf> = distinct.into_par_iter().map(|p| (p.clone(), scope::canonical(p))).collect();
    metas
        .into_iter()
        .map(|meta| {
            let mut cwds: Vec<PathBuf> = Vec::new();
            for c in meta.cwds.iter().map(|c| canon[c].clone()) {
                if !cwds.contains(&c) {
                    cwds.push(c);
                }
            }
            Entry { meta, cwds, parent: None, children: Vec::new(), forked_from: None }
        })
        .collect()
}
