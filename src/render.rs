//! Turn → text. Shared by `show` (display) and `grep` (search text).

use std::collections::HashMap;

use serde_json::Value;

use crate::model::{Block, Role, Turn};
use crate::text::{compact_json, indent, one_line, snippet, truncate};
use crate::time;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ToolsMode {
    Brief,
    Full,
    None,
}

pub struct Renderer<'a> {
    pub tools: ToolsMode,
    pub thinking: bool,
    /// Conversation only: user/assistant text, nothing else.
    pub chat: bool,
    /// Tool-call id → child session id, to mark spawn points.
    pub spawns: &'a HashMap<String, String>,
    /// Session start date; turns on other days show their date.
    pub start_date: String,
}

const BRIEF: usize = 200;

impl Renderer<'_> {
    /// Records this view leaves out entirely (`--tools none`, `--chat`).
    pub fn omits(&self, t: &Turn) -> bool {
        let has = |f: fn(&Block) -> bool| t.blocks.iter().any(f);
        let has_text = has(|b| matches!(b, Block::Text { .. }));
        if self.chat {
            return !matches!(t.role, Role::User | Role::Assistant) || !has_text;
        }
        self.tools == ToolsMode::None
            && (t.role == Role::Tool
                || (has(|b| matches!(b, Block::ToolCall { .. }))
                    && !has_text
                    && !has(|b| matches!(b, Block::Thinking { .. }))))
    }

    /// `None` when the record is omitted from this view.
    pub fn turn(&self, t: &Turn, note: Option<&str>) -> Option<String> {
        if self.omits(t) {
            return None;
        }
        let mut out = self.header(t);
        if let Some(note) = note {
            out.push_str(" · ");
            out.push_str(note);
        }
        for b in &t.blocks {
            if let Some(text) = self.block(t, b) {
                out.push('\n');
                out.push_str(&text);
            }
        }
        out.push('\n');
        Some(out)
    }

    fn header(&self, t: &Turn) -> String {
        let mut parts = vec![format!("## [{}] {}", t.idx, t.role)];
        if t.role == Role::Tool
            && let Some(Block::ToolResult { name, ok, .. }) =
                t.blocks.iter().find(|b| matches!(b, Block::ToolResult { .. }))
        {
            parts.push(name.clone().unwrap_or_else(|| "?".into()));
            if let Some(ok) = ok {
                parts.push(if *ok { "ok" } else { "error" }.into());
            }
        }
        if let Some(ts) = t.ts {
            let day = time::date(ts);
            parts.push(if day == self.start_date { time::clock(ts) } else { format!("{day} {}", time::clock(ts)) });
        }
        parts.join(" · ")
    }

    fn block(&self, t: &Turn, b: &Block) -> Option<String> {
        let full = self.tools == ToolsMode::Full;
        if self.chat && !matches!(b, Block::Text { .. }) {
            return None;
        }
        Some(match b {
            Block::Text { text } => text.trim_end().to_string(),
            Block::Thinking { text } if text.is_empty() => "[reasoning, encrypted]".into(),
            Block::Thinking { text } if self.thinking => format!("[thinking]\n{}", indent(text.trim_end(), "  ")),
            Block::Thinking { text } => format!("[thinking, {} chars]", text.chars().count()),
            Block::ToolCall { .. } if self.tools == ToolsMode::None => return None,
            Block::ToolCall { id, name, args } => {
                let mut line = tool_call(name, args, full);
                if let Some(child) = id.as_ref().and_then(|id| self.spawns.get(id)) {
                    line += &format!("\n  → child {child}");
                }
                line
            }
            Block::ToolResult { text, .. } => {
                let summary =
                    format!("  ← {} lines, {} chars", text.lines().count(), human_chars(text.chars().count()));
                let body =
                    if full { Some(text.trim_end().to_string()).filter(|t| !t.is_empty()) } else { preview(text) };
                match body {
                    Some(body) => format!("{summary}\n{}", indent(&body, "    ")),
                    None => summary,
                }
            }
            Block::Image { mime, bytes } => {
                format!("  [image {} {}]", mime.as_deref().unwrap_or("?"), human_bytes(*bytes))
            }
            Block::Event { summary, payload } => {
                // Standalone records name their type; inline content blocks carry it in the summary.
                let label =
                    if t.role == Role::Event || summary.is_empty() { format!("· {}", t.kind) } else { "·".into() };
                if full {
                    let pretty = serde_json::to_string_pretty(payload).unwrap_or_default();
                    let head = if summary.is_empty() { label } else { format!("{label}: {summary}") };
                    format!("{head}\n{}", indent(&pretty, "    "))
                } else {
                    let text =
                        if summary.is_empty() { compact_json(&without_bookkeeping(payload)) } else { summary.clone() };
                    format!("{label}: {}", snippet(&text, BRIEF))
                }
            }
        })
    }
}

fn tool_call(name: &str, args: &Value, full: bool) -> String {
    let args_text = match args {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v if full => serde_json::to_string_pretty(v).unwrap_or_default(),
        v => compact_json(v),
    };
    if !full {
        format!("  → {name} {}", snippet(&args_text, BRIEF))
    } else if args_text.contains('\n') {
        format!("  → {name}\n{}", indent(&args_text, "    "))
    } else {
        format!("  → {name} {args_text}")
    }
}

/// Per-record bookkeeping fields: ids, timestamps, versions. Not content.
const BOOKKEEPING: &[&str] = &[
    "type",
    "id",
    "parentId",
    "timestamp",
    "sessionId",
    "session_id",
    "uuid",
    "parentUuid",
    "leafUuid",
    "messageId",
    "promptId",
    "requestId",
    "thread_id",
    "turn_id",
    "cwd",
    "gitBranch",
    "version",
    "entrypoint",
    "userType",
    "isSidechain",
];

/// Opaque blobs never worth showing or searching.
const OPAQUE: &[&str] = &["data", "encrypted_content", "signature", "thinkingSignature", "textSignature", "image_url"];

fn without_bookkeeping(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| !BOOKKEEPING.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Brief tool output: the head, then the last line (errors and exit codes tend to be at the end).
fn preview(text: &str) -> Option<String> {
    let text = text.trim_end();
    if text.is_empty() {
        return None;
    }
    if text.chars().count() <= BRIEF + 100 && text.lines().count() <= 6 {
        return Some(text.to_string());
    }
    let mut head = String::new();
    for line in text.lines().take(4) {
        if head.chars().count() >= BRIEF {
            break;
        }
        head.push_str(&truncate(line, BRIEF));
        head.push('\n');
    }
    let last = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    Some(format!("{head}…\n(last) {}", truncate(last.trim(), BRIEF)))
}

fn human_chars(n: usize) -> String {
    if n < 1000 { n.to_string() } else { format!("{:.1}k", n as f64 / 1000.0) }
}

pub fn human_bytes(n: usize) -> String {
    match n {
        n if n < 1024 => format!("{n}B"),
        n if n < 1024 * 1024 => format!("{:.1}KB", n as f64 / 1024.0),
        n => format!("{:.1}MB", n as f64 / (1024.0 * 1024.0)),
    }
}

/// Searchable text of a turn, split by what `show` needs to reveal it.
pub struct SearchText {
    pub visible: String,
    pub thinking: String,
}

pub fn search_text(t: &Turn) -> SearchText {
    let mut visible = Vec::new();
    let mut thinking = Vec::new();
    for b in &t.blocks {
        match b {
            Block::Text { text } => visible.push(text.clone()),
            Block::Thinking { text } => thinking.push(text.clone()),
            Block::ToolCall { name, args, .. } => visible.push(format!("{name} {}", leaves(args))),
            Block::ToolResult { text, .. } => visible.push(text.clone()),
            Block::Image { .. } => {}
            Block::Event { summary, payload } => visible.push(format!("{summary} {}", leaves(payload))),
        }
    }
    SearchText { visible: visible.join("\n"), thinking: thinking.join("\n") }
}

/// String and number leaves of a JSON value: no keys, bookkeeping fields, or opaque blobs.
fn leaves(v: &Value) -> String {
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Number(n) => out.push(n.to_string()),
            Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            Value::Object(map) => map
                .iter()
                .filter(|(k, _)| !OPAQUE.contains(&k.as_str()) && !BOOKKEEPING.contains(&k.as_str()))
                .for_each(|(_, v)| walk(v, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, &mut out);
    out.join(" ")
}

/// One-line excerpt around byte range `start..end` of `text`, as (before, match, after).
pub fn excerpt(text: &str, start: usize, end: usize, context: usize) -> (String, String, String) {
    let before: String = text[..start].chars().rev().take(context).collect::<Vec<_>>().into_iter().rev().collect();
    let after: String = text[end..].chars().take(context).collect();
    let lead = if before.len() < start { "…" } else { "" };
    let tail = if after.len() < text.len() - end { "…" } else { "" };
    (
        format!("{lead}{}", one_line_keep_edges(&before)),
        one_line(&text[start..end]),
        format!("{}{tail}", one_line_keep_edges(&after)),
    )
}

/// Like `one_line` but keeps a single leading/trailing space so the match stays separated.
fn one_line_keep_edges(s: &str) -> String {
    let core = one_line(s);
    let lead = if s.starts_with(char::is_whitespace) && !core.is_empty() { " " } else { "" };
    let tail = if s.ends_with(char::is_whitespace) && !core.is_empty() { " " } else { "" };
    format!("{lead}{core}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_head_and_last_line() {
        let text = (1..=50).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let p = preview(&text).unwrap();
        assert!(p.starts_with("line 1\n"));
        assert!(p.ends_with("(last) line 50"));
    }

    #[test]
    fn excerpts_around_match() {
        let text = "aaaa bbbb\nneedle cccc dddd";
        let start = text.find("needle").unwrap();
        let (b, m, a) = excerpt(text, start, start + 6, 5);
        assert_eq!(format!("{b}{m}{a}"), "…bbbb needle cccc…");
    }
}
