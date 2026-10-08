//! Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl` and
//! `~/.codex/archived_sessions/`. Records are `{timestamp, type, payload}`;
//! the `event_msg` stream is secondary (shown with `--events`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::{Adapter, count_unknown, finalize, read_jsonl};
use crate::model::{Agent, Block, Role, Session, SessionMeta, Title};
use crate::text::{base64_bytes, content_text, snippet, str_field};
use crate::time::parse_ts;

pub struct Codex {
    pub home: PathBuf,
}

/// Top-level record types rendered as plain events.
const EVENT_TYPES: &[&str] =
    &["compacted", "world_state", "token_usage_record", "inter_agent_communication_metadata", "realtime_item"];

/// `response_item` payload types that older Codex versions wrote at top level.
const LEGACY_ITEM_TYPES: &[&str] =
    &["message", "reasoning", "function_call", "function_call_output", "local_shell_call"];

impl Adapter for Codex {
    fn session_dirs(&self) -> Vec<PathBuf> {
        vec![self.home.join("sessions"), self.home.join("archived_sessions")]
    }

    fn is_session_file(&self, _path: &Path) -> bool {
        true
    }

    fn load(&self, path: &Path) -> Result<Session> {
        let mut p = Parser {
            s: Session::new(Agent::Codex, path.to_path_buf()),
            calls: HashMap::new(),
            user_item: None,
            seen_meta: false,
        };
        let bad = read_jsonl(path, |rec| p.record(rec))?;
        if p.s.meta.title.is_none() {
            p.s.meta.title = p.user_item.map(|t| Title { text: snippet(&t, 120), source: "user-message".into() });
        }
        finalize(&mut p.s, bad);
        Ok(p.s)
    }

    /// Titles from `session_index.jsonl` (`{id, thread_name}`, later lines win).
    fn decorate(&self, metas: &mut [SessionMeta]) {
        let Ok(index) = std::fs::read_to_string(self.home.join("session_index.jsonl")) else { return };
        let names: HashMap<String, String> = index
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| Some((str_field(&v, "id")?.to_string(), str_field(&v, "thread_name")?.to_string())))
            .filter(|(_, name)| !name.is_empty())
            .collect();
        for m in metas {
            if let Some(name) = names.get(&m.id) {
                m.title = Some(Title { text: name.clone(), source: "thread_name".into() });
            }
        }
    }
}

struct Parser {
    s: Session,
    /// `call_id` → tool name, to label outputs.
    calls: HashMap<String, String>,
    /// First `item_completed` UserMessage: the typed prompt, used as title fallback.
    user_item: Option<String>,
    seen_meta: bool,
}

impl Parser {
    fn record(&mut self, rec: Value) {
        let ts = parse_ts(rec.get("timestamp"));
        let ty = str_field(&rec, "type").unwrap_or("").to_string();
        let payload = rec.get("payload").cloned().unwrap_or(Value::Null);
        match ty.as_str() {
            "session_meta" => self.session_meta(payload, ts),
            "response_item" => self.response_item(payload, ts),
            "event_msg" => self.event_msg(payload, ts),
            "turn_context" => {
                self.s.meta.add_cwd(str_field(&payload, "cwd").unwrap_or_default());
                if let Some(model) = str_field(&payload, "model") {
                    self.s.meta.model = Some(model.to_string());
                }
                let summary = format!(
                    "{} · {}",
                    str_field(&payload, "model").unwrap_or("?"),
                    str_field(&payload, "cwd").unwrap_or("?")
                );
                self.s.push(Role::Event, ts, ty, vec![Block::event(summary, payload)]);
            }
            t if EVENT_TYPES.contains(&t) => {
                let summary = str_field(&payload, "message").map(|m| snippet(m, 200)).unwrap_or_default();
                self.s.push(Role::Event, ts, ty, vec![Block::event(summary, payload)]);
            }
            t if LEGACY_ITEM_TYPES.contains(&t) => self.response_item(rec, ts),
            "" if rec.get("id").is_some() && !self.seen_meta => self.session_meta(rec, ts),
            _ => {
                count_unknown(&mut self.s.meta, &ty);
                self.s.push(Role::Event, ts, ty, vec![Block::event("", rec)]);
            }
        }
    }

    fn session_meta(&mut self, p: Value, ts: Option<jiff::Timestamp>) {
        if !self.seen_meta {
            self.seen_meta = true;
            let meta = &mut self.s.meta;
            meta.id = str_field(&p, "id").unwrap_or_default().to_string();
            meta.add_cwd(str_field(&p, "cwd").unwrap_or_default());
            meta.git_branch = p.pointer("/git/branch").and_then(Value::as_str).map(String::from);
            let subagent = p.pointer("/source/subagent");
            meta.child_kind = subagent.map(|s| match s {
                Value::String(k) => k.clone(),
                Value::Object(o) if o.contains_key("thread_spawn") => "subagent".into(),
                Value::Object(o) => o.get("other").and_then(Value::as_str).unwrap_or("subagent").to_string(),
                _ => "subagent".into(),
            });
            meta.parent_id = str_field(&p, "parent_thread_id")
                .or_else(|| p.pointer("/source/subagent/thread_spawn/parent_thread_id").and_then(Value::as_str))
                .map(String::from);
        }
        let source = match p.get("source") {
            Some(Value::String(s)) => s.clone(),
            Some(v) => crate::text::compact_json(v),
            None => "?".into(),
        };
        let summary = format!(
            "{} · source {} · cli {}",
            str_field(&p, "originator").unwrap_or("?"),
            snippet(&source, 120),
            str_field(&p, "cli_version").unwrap_or("?")
        );
        self.s.push(Role::Event, ts, "session_meta", vec![Block::event(summary, p)]);
    }

    fn response_item(&mut self, p: Value, ts: Option<jiff::Timestamp>) {
        let ty = str_field(&p, "type").unwrap_or("?").to_string();
        let kind = format!("response_item/{ty}");
        let (role, blocks) = match ty.as_str() {
            "message" => {
                let role = match str_field(&p, "role").unwrap_or("") {
                    "user" => Role::User,
                    "assistant" => Role::Assistant,
                    "developer" => Role::Developer,
                    "system" => Role::System,
                    other => Role::Custom(other.to_string()),
                };
                (role, message_blocks(p.get("content").unwrap_or(&Value::Null)))
            }
            "agent_message" => {
                let header = format!(
                    "{} → {}",
                    str_field(&p, "author").unwrap_or("?"),
                    str_field(&p, "recipient").unwrap_or("?")
                );
                let body = message_blocks(p.get("content").unwrap_or(&Value::Null));
                (Role::Assistant, std::iter::once(Block::text(header)).chain(body).collect())
            }
            "reasoning" => {
                let parts: Vec<String> = ["summary", "content"]
                    .iter()
                    .filter_map(|k| p.get(*k))
                    .map(content_text)
                    .filter(|t| !t.is_empty())
                    .collect();
                (Role::Assistant, vec![Block::Thinking { text: parts.join("\n") }])
            }
            "function_call" | "custom_tool_call" | "local_shell_call" | "web_search_call" | "tool_search_call" => {
                let name = match (str_field(&p, "namespace"), str_field(&p, "name")) {
                    (Some(ns), Some(n)) => format!("{ns}.{n}"),
                    (None, Some(n)) => n.to_string(),
                    _ => ty.trim_end_matches("_call").to_string(),
                };
                let args = match ty.as_str() {
                    "function_call" => parse_json_string(p.get("arguments")),
                    "custom_tool_call" => p.get("input").cloned().unwrap_or(Value::Null),
                    _ => p.get("action").or_else(|| p.get("arguments")).cloned().unwrap_or(Value::Null),
                };
                let id = str_field(&p, "call_id").map(String::from);
                if let Some(id) = &id {
                    self.calls.insert(id.clone(), name.clone());
                }
                (Role::Assistant, vec![Block::ToolCall { id, name, args }])
            }
            "function_call_output" | "custom_tool_call_output" | "tool_search_output" | "local_shell_call_output" => {
                let name = str_field(&p, "call_id").and_then(|id| self.calls.get(id)).cloned();
                let output = p.get("output").or_else(|| p.get("tools")).unwrap_or(&Value::Null);
                (Role::Tool, output_blocks(name, output))
            }
            _ => {
                if ty != "compaction" {
                    count_unknown(&mut self.s.meta, &kind);
                }
                (Role::Event, vec![Block::event("", p)])
            }
        };
        self.s.push(role, ts, kind, blocks);
    }

    fn event_msg(&mut self, p: Value, ts: Option<jiff::Timestamp>) {
        let ty = str_field(&p, "type").unwrap_or("?").to_string();
        let summary = match ty.as_str() {
            "item_completed" => {
                let item = p.get("item").unwrap_or(&Value::Null);
                let item_ty = str_field(item, "type").unwrap_or("?");
                let detail = match item_ty {
                    "UserMessage" | "AgentMessage" => content_text(item.get("content").unwrap_or(&Value::Null)),
                    "CommandExecution" => str_field(item, "command").unwrap_or_default().to_string(),
                    "McpToolCall" => format!(
                        "{}.{}",
                        str_field(item, "server").unwrap_or("?"),
                        str_field(item, "tool").unwrap_or("?")
                    ),
                    _ => String::new(),
                };
                if item_ty == "UserMessage" && self.user_item.is_none() && !detail.trim().is_empty() {
                    self.user_item = Some(detail.clone());
                }
                format!("{item_ty}: {}", snippet(&detail, 200))
            }
            "user_message" | "agent_message" => snippet(str_field(&p, "message").unwrap_or_default(), 200),
            _ => String::new(),
        };
        let pos = self.s.push(Role::Event, ts, format!("event_msg/{ty}"), vec![Block::event(summary, p)]);
        self.s.turns[pos].aux = true;
    }
}

fn message_blocks(content: &Value) -> Vec<Block> {
    let Some(items) = content.as_array() else {
        return vec![Block::text(content_text(content))];
    };
    items
        .iter()
        .map(|b| match str_field(b, "type").unwrap_or("") {
            "input_text" | "output_text" | "text" => Block::text(str_field(b, "text").unwrap_or_default()),
            "input_image" => data_url_image(str_field(b, "image_url").unwrap_or_default()),
            "encrypted_content" => Block::Thinking { text: String::new() },
            other => Block::event(format!("content block {other}"), b.clone()),
        })
        .collect()
}

/// Tool output: a plain string (sometimes JSON with `output` + `metadata.exit_code`)
/// or an array of `input_text` / `input_image` blocks.
fn output_blocks(name: Option<String>, output: &Value) -> Vec<Block> {
    if let Value::String(s) = output
        && let Ok(v) = serde_json::from_str::<Value>(s)
        && let Some(text) = str_field(&v, "output")
    {
        let ok = v.pointer("/metadata/exit_code").and_then(Value::as_i64).map(|c| c == 0);
        return vec![Block::ToolResult { name, ok, text: text.to_string() }];
    }
    let mut blocks = vec![Block::ToolResult { name, ok: None, text: content_text(output) }];
    blocks.extend(
        output
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| str_field(b, "type") == Some("input_image"))
            .map(|b| data_url_image(str_field(b, "image_url").unwrap_or_default())),
    );
    blocks
}

fn data_url_image(url: &str) -> Block {
    let (head, data) = url.split_once(',').unwrap_or(("", url));
    let mime = head.strip_prefix("data:").and_then(|h| h.split(';').next()).map(String::from);
    Block::Image { mime, bytes: base64_bytes(data) }
}

/// Function-call arguments are a JSON-encoded string; decode when possible.
fn parse_json_string(v: Option<&Value>) -> Value {
    match v {
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.clone())),
        Some(other) => other.clone(),
        None => Value::Null,
    }
}
