//! Metadata cache: `(path, mtime, size)` → `SessionMeta` in sqlite.
//! Purely an accelerator: any failure degrades to re-parsing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

use crate::model::SessionMeta;

/// Bump whenever `SessionMeta` or adapter output changes.
const SCHEMA_VERSION: i32 = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub mtime_ns: i64,
    pub size: i64,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Stamp> {
        let md = std::fs::metadata(path).ok()?;
        let mtime = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(Stamp { mtime_ns: mtime.as_nanos() as i64, size: md.len() as i64 })
    }
}

pub struct Cache {
    conn: Connection,
}

impl Cache {
    pub fn dir() -> PathBuf {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| crate::text::home_dir().map(|h| h.join(".cache")))
            .unwrap_or_else(std::env::temp_dir)
            .join("codeagentlog")
    }

    pub fn path() -> PathBuf {
        Self::dir().join("meta.sqlite")
    }

    pub fn open() -> Option<Cache> {
        std::fs::create_dir_all(Self::dir()).ok()?;
        let conn = Connection::open(Self::path()).ok()?;
        conn.busy_timeout(std::time::Duration::from_secs(2)).ok()?;
        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).ok()?;
        if version != SCHEMA_VERSION {
            conn.execute_batch(&format!("DROP TABLE IF EXISTS meta; PRAGMA user_version = {SCHEMA_VERSION};")).ok()?;
        }
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS meta (path TEXT PRIMARY KEY, mtime_ns INTEGER, size INTEGER, json TEXT);",
        )
        .ok()?;
        Some(Cache { conn })
    }

    pub fn load_all(&self) -> HashMap<PathBuf, (Stamp, SessionMeta)> {
        let Ok(mut stmt) = self.conn.prepare("SELECT path, mtime_ns, size, json FROM meta") else {
            return HashMap::new();
        };
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, Stamp { mtime_ns: r.get(1)?, size: r.get(2)? }, r.get::<_, String>(3)?))
        });
        rows.into_iter()
            .flatten()
            .flatten()
            .filter_map(|(path, stamp, json)| Some((PathBuf::from(path), (stamp, serde_json::from_str(&json).ok()?))))
            .collect()
    }

    pub fn update(&mut self, fresh: &[(Stamp, SessionMeta)]) {
        if fresh.is_empty() {
            return;
        }
        let Ok(tx) = self.conn.transaction() else { return };
        for (stamp, meta) in fresh {
            let Ok(json) = serde_json::to_string(meta) else { continue };
            let _ = tx.execute(
                "INSERT OR REPLACE INTO meta (path, mtime_ns, size, json) VALUES (?1, ?2, ?3, ?4)",
                params![meta.path.to_string_lossy(), stamp.mtime_ns, stamp.size, json],
            );
        }
        let _ = tx.commit();
    }

    /// Drop rows for files that no longer exist.
    pub fn remove(&mut self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let Ok(tx) = self.conn.transaction() else { return };
        for path in paths {
            let _ = tx.execute("DELETE FROM meta WHERE path = ?1", params![path.to_string_lossy()]);
        }
        let _ = tx.commit();
    }
}
