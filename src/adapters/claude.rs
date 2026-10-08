//! Claude Code: `~/.claude/projects/<-encoded-cwd>/<sessionId>.jsonl`, with
//! child sessions in `<sessionId>/subagents/**/agent-<agentId>.jsonl` (+ `.meta.json`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::{Adapter, count_unknown, finalize, read_jsonl};
use crate::model::{Agent, Block, Role, Session, Title};
use crate::text::{base64_bytes, content_text, snippet, str_field};
use crate::time::parse_ts;

pub struct Claude {
    pub home: PathBuf,
}

/// Record types rendered as plain events.
const EVENT_TYPES: &[&str] = &[
    "last-prompt",
    "mode",
    "permission-mode",
    "atis-latch",
    "file-history-snapshot",
    "file-history-delta",
    "queue-operation",
    "agent-name",
    "cost-state",
    "pr-link",
    "agent-setting",
    "tag",
];

/// Title precedence, best first.
const TITLE_SOURCES: &[&str] = &["custom-title", "ai-title", "summary", "description"];

impl Adapter for Claude {
    fn session_dirs(&self) -> Vec<PathBuf> {
        vec![self.home.join("projects")]
    }

    fn is_session_file(&self, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(self.home.join("projects")) else { return false };
        let depth = rel.components().count();
        let is_agent = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("agent-"));
        depth == 2 || (is_subagent(path) && is_agent)
    }

    fn load(&self, path: &Path) -> Result<Session> {
        let mut p = Parser {
            s: Session::new(Agent::Claude, path.to_path_buf()),
            messages: HashMap::new(),
            tool_names: HashMap::new(),
            titles: HashMap::new(),
        };
        if is_subagent(path) {
            p.child_meta(path);
        }
        let bad = read_jsonl(path, |rec| p.record(rec))?;
        p.apply_title();
        finalize(&mut p.s, bad);
        Ok(p.s)
    }
}

fn is_subagent(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "subagents")
}

struct Parser {
    s: Session,
    /// `message.id` → turn position, to merge the one-line-per-block assistant records.
    messages: HashMap<String, usize>,
    /// `tool_use.id` → tool name, to label tool results.
    tool_names: HashMap<String, String>,
    titles: HashMap<&'static str, String>,
}

impl Parser {
    /// Child id, parent link and kind from the path and the sibling `.meta.json`.
    fn child_meta(&mut self, path: &Path) {
        let meta = &mut self.s.meta;
        let parent = path
            .ancestors()
            .skip_while(|a| a.file_name().is_none_or(|n| n != "subagents"))
            .nth(1)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let agent_id = stem.strip_prefix("agent-").unwrap_or(&stem);
        if let Some(parent) = parent {
            meta.id = format!("{parent}/{agent_id}");
            meta.parent_id = Some(parent);
        }
        let sidecar = path.with_extension("meta.json");
        let info: Value = std::fs::read(sidecar).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        meta.child_kind = Some(str_field(&info, "agentType").unwrap_or("subagent").to_string());
        meta.spawn_ref = str_field(&info, "toolUseId").map(String::from);
        if let Some(desc) = str_field(&info, "description") {
            self.titles.insert("description", desc.to_string());
        }
    }

    fn record(&mut self, rec: Value) {
        let meta = &mut self.s.meta;
        if let Some(cwd) = str_field(&rec, "cwd") {
            meta.add_cwd(cwd);
        }
        if let Some(branch) = str_field(&rec, "gitBranch").filter(|b| !b.is_empty()) {
            meta.git_branch = Some(branch.to_string());
        }
        // The file name is the session id: resumed sessions copy lines carrying the old `sessionId`.

        let ty = str_field(&rec, "type").unwrap_or("").to_string();
        let ts = parse_ts(rec.get("timestamp"));
        match ty.as_str() {
            "assistant" => self.assistant(&rec, ts),
            "user" => self.user(&rec, ts),
            "system" => {
                let kind = format!("system/{}", str_field(&rec, "subtype").unwrap_or("?"));
                let blocks = match rec.get("content") {
                    Some(c) => vec![Block::text(content_text(c))],
                    None => vec![Block::event("", rec.clone())],
                };
                self.s.push(Role::System, ts, kind, blocks);
            }
            "attachment" => {
                let att = rec.get("attachment").unwrap_or(&Value::Null);
                let kind = str_field(att, "type").unwrap_or("?");
                let rendered = rec.get("rendered").map(rendered_text).unwrap_or_default();
                let summary = format!("{kind}: {}", snippet(&rendered, 200));
                self.s.push(Role::Event, ts, ty, vec![Block::event(summary, rec)]);
            }
            "ai-title" | "custom-title" | "summary" => {
                let key = if ty == "ai-title" {
                    "aiTitle"
                } else if ty == "custom-title" {
                    "customTitle"
                } else {
                    "summary"
                };
                let title = str_field(&rec, key).unwrap_or_default().to_string();
                let source = TITLE_SOURCES.iter().find(|s| **s == ty).copied().unwrap_or("summary");
                self.titles.insert(source, title.clone());
                self.s.push(Role::Event, ts, ty, vec![Block::event(title, rec)]);
            }
            "continued-in" => {
                let next = str_field(&rec, "continuedInSessionId").unwrap_or_default().to_string();
                self.s.meta.continued_in = Some(next.clone());
                self.s.push(Role::Event, ts, ty, vec![Block::event(format!("continued in claude:{next}"), rec)]);
            }
            t if EVENT_TYPES.contains(&t) => {
                self.s.push(Role::Event, ts, ty, vec![Block::event("", rec)]);
            }
            _ => {
                count_unknown(&mut self.s.meta, &ty);
                self.s.push(Role::Event, ts, ty, vec![Block::event("", rec)]);
            }
        }
    }

    fn assistant(&mut self, rec: &Value, ts: Option<jiff::Timestamp>) {
        let msg = rec.get("message").unwrap_or(&Value::Null);
        if let Some(model) = str_field(msg, "model").filter(|m| !m.starts_with('<')) {
            self.s.meta.model = Some(model.to_string());
        }
        let blocks = self.blocks(msg.get("content").unwrap_or(&Value::Null));
        let mid = str_field(msg, "id").map(String::from);
        if let Some(&pos) = mid.as_ref().and_then(|m| self.messages.get(m)) {
            self.s.turns[pos].blocks.extend(blocks);
            return;
        }
        let pos = self.s.push(Role::Assistant, ts, "assistant", blocks);
        if let Some(mid) = mid {
            self.messages.insert(mid, pos);
        }
    }

    fn user(&mut self, rec: &Value, ts: Option<jiff::Timestamp>) {
        let content = rec.pointer("/message/content").unwrap_or(&Value::Null);
        let blocks = self.blocks(content);
        let is_tool = blocks.iter().any(|b| matches!(b, Block::ToolResult { .. }));
        let (role, kind) = if is_tool { (Role::Tool, "user/tool_result") } else { (Role::User, "user") };
        self.s.push(role, ts, kind, blocks);
    }

    fn blocks(&mut self, content: &Value) -> Vec<Block> {
        let Some(items) = content.as_array() else {
            return vec![Block::text(content_text(content))];
        };
        let mut out = Vec::new();
        for b in items {
            match str_field(b, "type").unwrap_or("") {
                "text" => out.push(Block::text(str_field(b, "text").unwrap_or_default())),
                "thinking" => {
                    out.push(Block::Thinking { text: str_field(b, "thinking").unwrap_or_default().to_string() })
                }
                "redacted_thinking" => out.push(Block::Thinking { text: String::new() }),
                "tool_use" | "server_tool_use" => {
                    let id = str_field(b, "id").map(String::from);
                    let name = str_field(b, "name").unwrap_or("?").to_string();
                    if let Some(id) = &id {
                        self.tool_names.insert(id.clone(), name.clone());
                    }
                    out.push(Block::ToolCall { id, name, args: b.get("input").cloned().unwrap_or(Value::Null) });
                }
                "tool_result" => {
                    let inner = b.get("content").unwrap_or(&Value::Null);
                    let name = str_field(b, "tool_use_id").and_then(|id| self.tool_names.get(id)).cloned();
                    let ok = b.get("is_error").and_then(Value::as_bool).map(|e| !e);
                    out.push(Block::ToolResult { name, ok, text: content_text(inner) });
                    out.extend(inner.as_array().into_iter().flatten().filter_map(image));
                }
                "image" => out.extend(image(b)),
                other => out.push(Block::event(format!("content block {other}"), b.clone())),
            }
        }
        out
    }

    fn apply_title(&mut self) {
        self.s.meta.title = TITLE_SOURCES.iter().find_map(|src| {
            self.titles.get(src).filter(|t| !t.is_empty()).map(|t| Title { text: t.clone(), source: src.to_string() })
        });
    }
}

fn image(b: &Value) -> Option<Block> {
    (str_field(b, "type") == Some("image")).then(|| Block::Image {
        mime: b.pointer("/source/media_type").and_then(Value::as_str).map(String::from),
        bytes: base64_bytes(b.pointer("/source/data").and_then(Value::as_str).unwrap_or_default()),
    })
}

/// Attachment `rendered` is a string or a list of `{content}` items.
fn rendered_text(v: &Value) -> String {
    match v {
        Value::Array(items) => {
            items.iter().map(|i| i.get("content").map(content_text).unwrap_or_default()).collect::<Vec<_>>().join("\n")
        }
        other => content_text(other),
    }
}
