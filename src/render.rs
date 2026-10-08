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
    /// Tool-call id → child session id, to mark spawn points.
    pub spawns: &'a HashMap<String, String>,
    /// Session start date; turns on other days show their date.
    pub start_date: String,
}

const BRIEF: usize = 200;

impl Renderer<'_> {
    /// `None` when the turn is omitted entirely (`--tools none`).
    pub fn turn(&self, t: &Turn, note: Option<&str>) -> Option<String> {
        if self.tools == ToolsMode::None && t.role == Role::Tool {
            return None;
        }
        let mut body: Vec<String> = Vec::new();
        for b in &t.blocks {
            self.block(t, b, &mut body);
        }
        if body.is_empty()
            && self.tools == ToolsMode::None
            && t.blocks.iter().any(|b| matches!(b, Block::ToolCall { .. }))
        {
            return None;
        }
        let mut out = self.header(t);
        if let Some(note) = note {
            out.push_str(" · ");
            out.push_str(note);
        }
        for line in body {
            out.push('\n');
            out.push_str(&line);
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

    fn block(&self, t: &Turn, b: &Block, out: &mut Vec<String>) {
        let full = self.tools == ToolsMode::Full;
        match b {
            Block::Text { text } => out.push(text.trim_end().to_string()),
            Block::Thinking { text } if text.is_empty() => out.push("[reasoning, encrypted]".into()),
            Block::Thinking { text } if self.thinking => {
                out.push(format!("[thinking]\n{}", indent(text.trim_end(), "  ")))
            }
            Block::Thinking { text } => out.push(format!("[thinking, {} chars]", text.chars().count())),
            Block::ToolCall { .. } if self.tools == ToolsMode::None => {}
            Block::ToolCall { id, name, args } => {
                let args_text = match args {
                    Value::String(s) => s.clone(),
                    Value::Null => String::new(),
                    v if full => serde_json::to_string_pretty(v).unwrap_or_default(),
                    v => compact_json(v),
                };
                if full && args_text.contains('\n') {
                    out.push(format!("  → {name}\n{}", indent(&args_text, "    ")));
                } else if full {
                    out.push(format!("  → {name} {args_text}"));
                } else {
                    out.push(format!("  → {name} {}", snippet(&args_text, BRIEF)));
                }
                if let Some(child) = id.as_ref().and_then(|id| self.spawns.get(id)) {
                    out.push(format!("  → child {child}"));
                }
            }
            Block::ToolResult { text, .. } => {
                let summary =
                    format!("  ← {} lines, {} chars", text.lines().count(), human_chars(text.chars().count()));
                out.push(summary);
                if full {
                    if !text.is_empty() {
                        out.push(indent(text.trim_end(), "    "));
                    }
                } else if let Some(preview) = preview(text) {
                    out.push(indent(&preview, "    "));
                }
            }
            Block::Image { mime, bytes } => {
                out.push(format!("  [image {} {}]", mime.as_deref().unwrap_or("?"), human_bytes(*bytes)));
            }
            Block::Event { summary, payload } => {
                let label = if t.role == Role::Event { format!("· {}", t.kind) } else { "·".into() };
                if full {
                    let pretty = serde_json::to_string_pretty(payload).unwrap_or_default();
                    let head = if summary.is_empty() { label } else { format!("{label}: {summary}") };
                    out.push(format!("{head}\n{}", indent(&pretty, "    ")));
                } else {
                    let text =
                        if summary.is_empty() { compact_json(&without_boilerplate(payload)) } else { summary.clone() };
                    out.push(format!("{label}: {}", snippet(&text, BRIEF)));
                }
            }
        }
    }
}

/// Drop per-record bookkeeping fields from a one-line event payload.
fn without_boilerplate(v: &Value) -> Value {
    const KEYS: &[&str] = &[
        "type",
        "id",
        "parentId",
        "timestamp",
        "sessionId",
        "session_id",
        "uuid",
        "parentUuid",
        "cwd",
        "gitBranch",
        "version",
        "entrypoint",
        "userType",
        "isSidechain",
    ];
    match v {
        Value::Object(map) => Value::Object(
            map.iter().filter(|(k, _)| !KEYS.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect(),
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
            Block::Event { summary, payload } => visible.push(format!("{} {summary} {}", t.kind, leaves(payload))),
        }
    }
    SearchText { visible: visible.join("\n"), thinking: thinking.join("\n") }
}

/// String and number leaves of a JSON value, without keys or opaque blobs.
fn leaves(v: &Value) -> String {
    const OPAQUE: &[&str] =
        &["data", "encrypted_content", "signature", "thinkingSignature", "textSignature", "image_url"];
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.push(s.clone()),
            Value::Number(n) => out.push(n.to_string()),
            Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            Value::Object(map) => {
                map.iter().filter(|(k, _)| !OPAQUE.contains(&k.as_str())).for_each(|(_, v)| walk(v, out))
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, &mut out);
    out.join(" ")
}

/// One-line excerpt around byte range `start..end` of `text`.
pub fn excerpt(text: &str, start: usize, end: usize, context: usize) -> String {
    let before: String = text[..start].chars().rev().take(context).collect::<Vec<_>>().into_iter().rev().collect();
    let after: String = text[end..].chars().take(context).collect();
    let lead = if before.len() < start { "…" } else { "" };
    let tail = if after.len() < text.len() - end { "…" } else { "" };
    format!(
        "{lead}{}{}{}{tail}",
        one_line_keep_edges(&before),
        &one_line(&text[start..end]),
        one_line_keep_edges(&after)
    )
}

/// Like `one_line` but preserves a single leading/trailing space so the match stays separated.
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
        assert_eq!(excerpt(text, start, start + 6, 5), "…bbbb needle cccc…");
    }
}
