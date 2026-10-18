//! Cursor's own database, `state.vscdb` (SQLite, table `cursorDiskKV`). Each conversation is a
//! `composerData:<id>` row (title, ordered message headers) plus one `bubbleId:<id>:<bubbleId>`
//! row per message, with timestamps, thinking, and tool calls with their results: everything
//! the transcript JSONL leaves out. `show --db` renders a session from here instead.
//!
//! Opened read-only and `immutable`, so Cursor's file is never locked or written. The format
//! is undocumented; unknown message types render as events.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use super::{count_unknown, finalize};
use crate::model::{Agent, Block, Role, Session, SessionMeta, Title};
use crate::text::{home_dir, str_field};
use crate::time::parse_ts;

/// `$CODEAGENTLOG_CURSOR_DB`, else Cursor's per-OS default.
pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CODEAGENTLOG_CURSOR_DB") {
        return Some(PathBuf::from(p));
    }
    let user = if cfg!(target_os = "macos") {
        home_dir()?.join("Library/Application Support/Cursor/User")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("Cursor/User")
    } else {
        home_dir()?.join(".config/Cursor/User")
    };
    Some(user.join("globalStorage/state.vscdb"))
}

/// The conversation behind a transcript, read from the database. Keeps the transcript's
/// ids and cwd; records are renumbered in database order.
pub fn load(meta: SessionMeta) -> Result<Session> {
    if meta.agent != Agent::Cursor {
        bail!("--db only applies to cursor sessions");
    }
    let db = path().context("can't locate Cursor's database (set CODEAGENTLOG_CURSOR_DB)")?;
    if !db.is_file() {
        bail!("Cursor's database not found at {} (set CODEAGENTLOG_CURSOR_DB)", db.display());
    }
    let conn = open(&db)?;
    // Child ids are `<parent>/<child>`; the database keys children by their own id.
    let id = meta.id.rsplit('/').next().unwrap_or(&meta.id).to_string();
    let composer = get(&conn, &format!("composerData:{id}"))?
        .with_context(|| format!("{} is not in Cursor's database (deleted from Cursor?)", meta.display_id()))?;
    let bubbles = bubbles(&conn, &id)?;

    let mut s = Session { meta, turns: Vec::new() };
    if let Some(name) = str_field(&composer, "name").filter(|n| !n.trim().is_empty()) {
        s.meta.title = Some(Title { text: name.to_string(), source: "cursor-db".into() });
    }
    if let Some(model) = composer.pointer("/modelConfig/modelName").and_then(Value::as_str).filter(|m| *m != "default")
    {
        s.meta.model = Some(model.to_string());
    }
    // Older versions embed the messages in `conversation`; newer ones list headers only.
    let headers = ["fullConversationHeadersOnly", "conversation"]
        .iter()
        .find_map(|k| composer.get(*k).and_then(Value::as_array).filter(|a| !a.is_empty()));
    let Some(headers) = headers else { bail!("{} has no messages in Cursor's database", s.meta.display_id()) };
    for h in headers {
        let bubble = str_field(h, "bubbleId").and_then(|b| bubbles.get(b)).unwrap_or(h);
        push_bubble(&mut s, bubble);
    }
    finalize(&mut s, 0);
    Ok(s)
}

fn open(db: &Path) -> Result<Connection> {
    // SQLite URIs treat `?`, `#` and `%` specially.
    let escaped = db.to_string_lossy().replace('%', "%25").replace('?', "%3f").replace('#', "%23");
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    Connection::open_with_flags(format!("file:{escaped}?immutable=1"), flags)
        .with_context(|| format!("opening {}", db.display()))
}

/// Values are JSON stored as TEXT or BLOB.
fn json(v: rusqlite::types::ValueRef<'_>) -> Option<Value> {
    serde_json::from_slice(v.as_bytes().ok()?).ok()
}

fn get(conn: &Connection, key: &str) -> Result<Option<Value>> {
    let row = conn
        .query_row("SELECT value FROM cursorDiskKV WHERE key = ?1", [key], |r| Ok(json(r.get_ref(0)?)))
        .optional()
        .context("reading Cursor's database")?;
    Ok(row.flatten())
}

/// Every `bubbleId:<id>:*` row, keyed by bubble id (one range scan on the primary key).
fn bubbles(conn: &Connection, id: &str) -> Result<HashMap<String, Value>> {
    let (lo, hi) = (format!("bubbleId:{id}:"), format!("bubbleId:{id};"));
    let mut stmt = conn.prepare("SELECT key, value FROM cursorDiskKV WHERE key >= ?1 AND key < ?2")?;
    let rows = stmt.query_map([&lo, &hi], |r| Ok((r.get::<_, String>(0)?, json(r.get_ref(1)?))))?;
    let mut out = HashMap::new();
    for row in rows {
        let (key, value) = row?;
        if let (Some(bubble), Some(value)) = (key.strip_prefix(&lo), value) {
            out.insert(bubble.to_string(), value);
        }
    }
    Ok(out)
}

/// Bubble `type` 1 is the user, 2 the assistant. A tool bubble becomes the assistant's call
/// followed by a tool record with its result. Bubbles with no content at all (streaming
/// placeholders) are kept as secondary records, shown with `--events`.
fn push_bubble(s: &mut Session, b: &Value) {
    let ts = parse_ts(b.get("createdAt"));
    let text = str_field(b, "text").unwrap_or_default();
    match b.get("type").and_then(Value::as_i64) {
        Some(1) => {
            s.push(Role::User, ts, "user", vec![Block::text(text)]);
        }
        Some(2) => {
            let mut blocks = Vec::new();
            // An empty thinking text is reasoning Cursor didn't keep.
            if let Some(thinking) = b.pointer("/thinking/text").and_then(Value::as_str) {
                blocks.push(Block::Thinking { text: thinking.to_string() });
            }
            if !text.is_empty() {
                blocks.push(Block::text(text));
            }
            let tool = b.get("toolFormerData").filter(|t| t.is_object());
            let name = tool.and_then(|t| str_field(t, "name")).map(String::from);
            if let Some(t) = tool {
                let args = ["params", "rawArgs"]
                    .iter()
                    .find_map(|k| t.get(*k).filter(|v| !v.is_null() && v.as_str() != Some("")))
                    .map(embedded_json)
                    .unwrap_or(Value::Null);
                let id = str_field(t, "toolCallId").map(String::from);
                blocks.push(Block::ToolCall { id, name: name.clone().unwrap_or_else(|| "?".into()), args });
            }
            if let Some(err) = b.get("errorDetails").filter(|e| e.is_object()) {
                let title = err.pointer("/error/details/title").and_then(Value::as_str).unwrap_or("error");
                blocks.push(Block::event(title, err.clone()));
            }
            let empty = blocks.is_empty();
            if empty {
                blocks.push(Block::event("empty message", Value::Null));
            }
            let pos = s.push(Role::Assistant, ts, "assistant", blocks);
            s.turns[pos].aux = empty;
            if let Some(result) = tool.and_then(|t| t.get("result")).filter(|r| !r.is_null()) {
                let ok = match tool.and_then(|t| str_field(t, "status")) {
                    Some("completed") => Some(true),
                    Some("error" | "failed") => Some(false),
                    _ => None,
                };
                s.push(Role::Tool, ts, "tool_result", vec![Block::ToolResult { name, ok, text: result_text(result) }]);
            }
        }
        other => {
            let kind = format!("bubble/{}", other.map(|t| t.to_string()).unwrap_or_else(|| "?".into()));
            count_unknown(&mut s.meta, &kind);
            s.push(Role::Event, ts, kind, vec![Block::event("", b.clone())]);
        }
    }
}

/// Tool params and results are JSON encoded inside a string.
fn embedded_json(v: &Value) -> Value {
    match v {
        Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| v.clone()),
        other => other.clone(),
    }
}

/// A result's string fields (`output`, `contents`, …) as text; anything else as compact JSON.
fn result_text(v: &Value) -> String {
    match embedded_json(v) {
        Value::String(s) => s,
        Value::Object(map) if map.values().any(Value::is_string) => map
            .iter()
            .map(|(k, v)| match v {
                Value::String(s) if map.len() == 1 => s.clone(),
                Value::String(s) => format!("{k}: {s}"),
                other => format!("{k}: {other}"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => other.to_string(),
    }
}
