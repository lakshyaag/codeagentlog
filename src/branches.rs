//! Branch structure of tree-shaped transcripts (pi entries with `id`/`parentId`).

use std::collections::{HashMap, HashSet};

use crate::model::Turn;

pub struct Branches {
    parent: HashMap<String, Option<String>>,
    /// Leaf entry ids in file order; the last one is the active leaf.
    leaves: Vec<String>,
}

impl Branches {
    /// `None` for linear transcripts (no tree nodes).
    pub fn of(turns: &[Turn]) -> Option<Branches> {
        let nodes: Vec<_> = turns.iter().filter_map(|t| t.node.as_ref()).collect();
        if nodes.is_empty() {
            return None;
        }
        let referenced: HashSet<&str> = nodes.iter().filter_map(|n| n.parent.as_deref()).collect();
        let mut leaves: Vec<String> =
            nodes.iter().filter(|n| !referenced.contains(n.id.as_str())).map(|n| n.id.clone()).collect();
        // The active leaf is the last entry written, even if it isn't last among leaves.
        let last = nodes.last().map(|n| n.id.clone()).expect("non-empty");
        leaves.retain(|l| *l != last);
        leaves.push(last);
        let parent = nodes.iter().map(|n| (n.id.clone(), n.parent.clone())).collect();
        Some(Branches { parent, leaves })
    }

    pub fn active_leaf(&self) -> &str {
        self.leaves.last().expect("at least one leaf")
    }

    pub fn other_leaves(&self) -> &[String] {
        &self.leaves[..self.leaves.len() - 1]
    }

    /// Ids from `entry` up to the root.
    pub fn path(&self, entry: &str) -> HashSet<String> {
        let mut out = HashSet::new();
        let mut cur = Some(entry.to_string());
        while let Some(id) = cur {
            if !out.insert(id.clone()) {
                break; // cycle guard
            }
            cur = self.parent.get(&id).cloned().flatten();
        }
        out
    }

    /// Resolve an entry id or unique prefix.
    pub fn find(&self, prefix: &str) -> Option<String> {
        let mut matches = self.parent.keys().filter(|id| id.starts_with(prefix));
        let first = matches.next()?;
        if first == prefix || matches.next().is_none() { Some(first.clone()) } else { None }
    }

    /// A leaf whose path contains `entry` (for pointing at off-branch hits).
    pub fn leaf_containing(&self, entry: &str) -> Option<&str> {
        self.leaves.iter().rev().find(|l| self.path(l).contains(entry)).map(String::as_str)
    }
}
