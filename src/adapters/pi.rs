//! pi: `~/.pi/agent/sessions/<--encoded-cwd-->/<ts>_<uuid>.jsonl`.
//! Entries form a tree via `id`/`parentId`; the active path ends at the last entry.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;
use regex::Regex;
use serde_json::{Value, json};

use super::{Adapter, count_unknown, finalize, read_jsonl};
use crate::model::{Agent, Block, Node, Role, Session, Title};
use crate::text::{base64_bytes, content_text, snippet, str_field};
use crate::time::parse_ts;

pub struct Pi {
    pub home: PathBuf,
}

/// Record types rendered as plain events.
const EVENT_TYPES: &[&str] = &["thinking_level_change", "custom", "context_edit", "usage", "label", "branch_summary"];

impl Adapter for Pi {
    fn session_dirs(&self) -> Vec<PathBuf> {
        vec![self.home.join("sessions")]
    }

    fn is_session_file(&self, _path: &Path) -> bool {
        true
    }

    fn load(&self, path: &Path) -> Result<Session> {
        let mut s = Session::new(Agent::Pi, path.to_path_buf());
        let bad = read_jsonl(path, |rec| record(&mut s, rec))?;
        finalize(&mut s, bad);
        s.meta.child_kind = subagent_kind(s.meta.title.as_ref());
        Ok(s)
    }
}

fn record(s: &mut Session, rec: Value) {
    let ty = str_field(&rec, "type").unwrap_or("").to_string();
    let ts = parse_ts(rec.get("timestamp"));
    let node = str_field(&rec, "id")
        .filter(|_| ty != "session")
        .map(|id| Node { id: id.to_string(), parent: str_field(&rec, "parentId").map(String::from) });
    let pos = match ty.as_str() {
        "session" => {
            s.meta.id = str_field(&rec, "id").unwrap_or_default().to_string();
            s.meta.add_cwd(str_field(&rec, "cwd").unwrap_or_default());
            s.meta.forked_from = str_field(&rec, "parentSession").map(String::from);
            let summary = format!("pi session v{}", rec.get("version").map(|v| v.to_string()).unwrap_or_default());
            s.push(Role::Event, ts, ty, vec![Block::event(summary, rec)])
        }
        "message" => message(s, &rec, ts),
        "model_change" => {
            let model = str_field(&rec, "modelId").unwrap_or_default().to_string();
            let summary = format!("{}/{model}", str_field(&rec, "provider").unwrap_or("?"));
            s.meta.model = Some(model);
            s.push(Role::Event, ts, ty, vec![Block::event(summary, rec)])
        }
        "session_info" => {
            let name = str_field(&rec, "name").unwrap_or_default().to_string();
            if !name.is_empty() {
                s.meta.title = Some(Title { text: name.clone(), source: "session_info".into() });
            }
            s.push(Role::Event, ts, ty, vec![Block::event(name, rec)])
        }
        "compaction" => {
            let summary = snippet(str_field(&rec, "summary").unwrap_or_default(), 200);
            s.push(Role::Event, ts, ty, vec![Block::event(summary, rec)])
        }
        "custom_message" => {
            let custom = str_field(&rec, "customType").unwrap_or("?").to_string();
            let blocks = content_blocks(rec.get("content").unwrap_or(&Value::Null));
            s.push(Role::Custom(custom), ts, ty, blocks)
        }
        t if EVENT_TYPES.contains(&t) => s.push(Role::Event, ts, ty, vec![Block::event("", rec)]),
        _ => {
            count_unknown(&mut s.meta, &ty);
            s.push(Role::Event, ts, ty, vec![Block::event("", rec)])
        }
    };
    s.turns[pos].node = node;
}

fn message(s: &mut Session, rec: &Value, ts: Option<jiff::Timestamp>) -> usize {
    let msg = rec.get("message").unwrap_or(&Value::Null);
    let role = str_field(msg, "role").unwrap_or("?");
    let ts = parse_ts(msg.get("timestamp")).or(ts);
    let kind = format!("message/{role}");
    let content = msg.get("content").unwrap_or(&Value::Null);
    match role {
        "user" => s.push(Role::User, ts, kind, content_blocks(content)),
        "assistant" => {
            if let Some(model) = str_field(msg, "model") {
                s.meta.model = Some(model.to_string());
            }
            s.push(Role::Assistant, ts, kind, content_blocks(content))
        }
        "system" => s.push(Role::System, ts, kind, content_blocks(content)),
        "toolResult" => {
            let mut blocks = vec![Block::ToolResult {
                name: str_field(msg, "toolName").map(String::from),
                ok: msg.get("isError").and_then(Value::as_bool).map(|e| !e),
                text: content_text(content),
            }];
            blocks.extend(content_blocks(content).into_iter().filter(|b| matches!(b, Block::Image { .. })));
            s.push(Role::Tool, ts, kind, blocks)
        }
        "bashExecution" => {
            let exit = msg.get("exitCode").and_then(Value::as_i64);
            let cancelled = msg.get("cancelled").and_then(Value::as_bool).unwrap_or(false);
            let blocks = vec![
                Block::ToolCall {
                    id: None,
                    name: "bash".into(),
                    args: json!({ "command": str_field(msg, "command").unwrap_or_default() }),
                },
                Block::ToolResult {
                    name: Some("bash".into()),
                    ok: Some(exit == Some(0) && !cancelled),
                    text: str_field(msg, "output").unwrap_or_default().to_string(),
                },
            ];
            s.push(Role::Bash, ts, kind, blocks)
        }
        _ => {
            count_unknown(&mut s.meta, &kind);
            s.push(Role::Event, ts, kind, vec![Block::event("", msg.clone())])
        }
    }
}

/// pi content: a plain string or an array of typed blocks.
fn content_blocks(content: &Value) -> Vec<Block> {
    let Some(items) = content.as_array() else {
        let text = content_text(content);
        return if text.is_empty() { vec![] } else { vec![Block::text(text)] };
    };
    items
        .iter()
        .map(|b| match str_field(b, "type").unwrap_or("") {
            "text" => Block::text(str_field(b, "text").unwrap_or_default()),
            "thinking" => Block::Thinking { text: str_field(b, "thinking").unwrap_or_default().to_string() },
            "toolCall" => Block::ToolCall {
                id: str_field(b, "id").map(String::from),
                name: str_field(b, "name").unwrap_or("?").to_string(),
                args: b.get("arguments").cloned().unwrap_or(Value::Null),
            },
            "image" => Block::Image {
                mime: str_field(b, "mimeType").map(String::from),
                bytes: base64_bytes(str_field(b, "data").unwrap_or_default()),
            },
            other => Block::event(format!("content block {other}"), b.clone()),
        })
        .collect()
}

/// pi subagent sessions are named like `subagent-scout-<uuid>-1`.
fn subagent_kind(title: Option<&Title>) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^subagent-(.+?)-[0-9a-f]{8}-[0-9a-f]{4}-").unwrap());
    let title = title.filter(|t| t.source == "session_info")?;
    re.captures(&title.text).map(|c| c[1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_subagent_kind() {
        let t = Title {
            text: "subagent-context-builder-0c1d2e3f-aaaa-4bbb-8ccc-123456789abc-1".into(),
            source: "session_info".into(),
        };
        assert_eq!(subagent_kind(Some(&t)).as_deref(), Some("context-builder"));
    }
}
