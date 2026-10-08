use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use jiff::Timestamp;
use serde::Serialize;

use super::{Ctx, to_json};
use crate::model::Agent;
use crate::text::{home_dir, tilde};
use crate::time;
use crate::{TreeArgs, style};

#[derive(Default, Clone, Copy, Serialize)]
struct Count {
    roots: usize,
    children: usize,
}

impl Count {
    fn add(&mut self, other: Count) {
        self.roots += other.roots;
        self.children += other.children;
    }

    fn label(self) -> String {
        if self.children > 0 { format!("{}+{}", self.roots, self.children) } else { self.roots.to_string() }
    }
}

#[derive(Default)]
struct Node {
    own: BTreeMap<Agent, Count>,
    /// Most recent update in this subtree.
    last: Option<Timestamp>,
    kids: BTreeMap<String, Node>,
}

impl Node {
    fn insert(&mut self, rel: &[String], agent: Agent, count: Count, updated: Timestamp) {
        self.last = self.last.max(Some(updated));
        match rel.split_first() {
            None => self.own.entry(agent).or_default().add(count),
            Some((head, rest)) => self.kids.entry(head.clone()).or_default().insert(rest, agent, count, updated),
        }
    }

    /// Own counts plus all descendants'.
    fn total(&self) -> BTreeMap<Agent, Count> {
        let mut sum = self.own.clone();
        for kid in self.kids.values() {
            for (a, c) in kid.total() {
                sum.entry(a).or_default().add(c);
            }
        }
        sum
    }

    fn kids_by_recency(&self) -> Vec<(&String, &Node)> {
        let mut kids: Vec<_> = self.kids.iter().collect();
        kids.sort_by_key(|(_, n)| std::cmp::Reverse(n.last));
        kids
    }
}

struct Row {
    label: String,
    counts: BTreeMap<Agent, Count>,
    last: Option<Timestamp>,
}

#[derive(Serialize)]
struct FolderJson {
    path: PathBuf,
    counts: BTreeMap<Agent, Count>,
    last: Option<Timestamp>,
}

pub fn run(ctx: &Ctx, args: TreeArgs) -> Result<String> {
    let base = match &args.path {
        Some(p) => crate::scope::canonical(p),
        None => home_dir().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/")),
    };
    let since = args.filters.since()?;
    let catalog = ctx.catalog(&args.filters.agent);
    let agents: Vec<Agent> =
        if args.filters.agent.is_empty() { Agent::ALL.to_vec() } else { args.filters.agent.clone() };

    let mut root = Node::default();
    let mut folders: BTreeMap<PathBuf, (BTreeMap<Agent, Count>, Timestamp)> = BTreeMap::new();
    let mut outside = 0;
    for e in catalog.entries.iter().filter(|e| e.parent.is_none() && args.filters.allows(e, since)) {
        let Some(cwd) = e.cwd() else { continue };
        let Ok(rel) = cwd.strip_prefix(&base) else {
            outside += 1;
            continue;
        };
        let rel: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let count = Count { roots: 1, children: e.children.len() };
        root.insert(&rel, e.meta.agent, count, e.meta.updated);
        let f = folders.entry(cwd.to_path_buf()).or_insert_with(|| (BTreeMap::new(), e.meta.updated));
        f.0.entry(e.meta.agent).or_default().add(count);
        f.1 = f.1.max(e.meta.updated);
    }

    if ctx.json {
        let list: Vec<FolderJson> =
            folders.into_iter().map(|(path, (counts, last))| FolderJson { path, counts, last: Some(last) }).collect();
        return to_json(&serde_json::json!({ "root": base, "folders": list, "outside_root": outside }));
    }

    let mut rows = vec![Row { label: tilde(&base), counts: root.own.clone(), last: root.last }];
    walk(&root, "", args.depth.unwrap_or(usize::MAX), 1, &mut rows);

    let now = Timestamp::now();
    let width = rows.iter().map(|r| r.label.chars().count()).max().unwrap_or(0) + 2;
    let mut out = String::new();
    let head: String = agents.iter().map(|a| style::agent(*a, &format!("{:>8}", a.name()))).collect();
    writeln!(out, "{:width$}{head}{}", "", style::dim(&format!("{:>7}", "last")))?;
    for (n, r) in rows.iter().enumerate() {
        let label = format!("{:width$}", r.label);
        let label = if n == 0 { style::bold(&label) } else { label };
        let cols: String = agents
            .iter()
            .map(|a| match r.counts.get(a) {
                Some(c) => format!("{:>8}", c.label()),
                None => style::dim(&format!("{:>8}", "·")),
            })
            .collect();
        let last = r.last.map(|t| time::age(t, now)).unwrap_or_default();
        writeln!(out, "{label}{cols}{}", style::dim(&format!("{last:>7}")))?;
    }
    if outside > 0 && args.path.is_none() {
        let note = format!("{outside} sessions outside {}. see:", tilde(&base));
        writeln!(out, "\n{}", style::footer(&note, Some("codeagentlog tree /")))?;
    }
    let note = "counts are root sessions (+children). list one:";
    writeln!(out, "{}", style::footer(note, Some("codeagentlog ls <folder> --here")))?;
    Ok(out)
}

/// Append rows for `node`'s children, collapsing single-child chains without
/// sessions (`a/b/c`) and folding everything below `max_depth` into its row.
fn walk(node: &Node, prefix: &str, max_depth: usize, depth: usize, rows: &mut Vec<Row>) {
    let kids = node.kids_by_recency();
    for (k, (name, child)) in kids.iter().enumerate() {
        let last_kid = k + 1 == kids.len();
        let (mut label, mut cur) = ((*name).clone(), *child);
        while cur.own.is_empty() && cur.kids.len() == 1 {
            let (n, c) = cur.kids.iter().next().expect("one kid");
            label = format!("{label}/{n}");
            cur = c;
        }
        let folded = depth >= max_depth && !cur.kids.is_empty();
        let counts = if folded { cur.total() } else { cur.own.clone() };
        if folded {
            label += "/…";
        }
        let branch = if last_kid { "└─ " } else { "├─ " };
        rows.push(Row { label: format!("{prefix}{branch}{label}"), counts, last: cur.last });
        if !folded {
            let next_prefix = format!("{prefix}{}", if last_kid { "   " } else { "│  " });
            walk(cur, &next_prefix, max_depth, depth + 1, rows);
        }
    }
}
