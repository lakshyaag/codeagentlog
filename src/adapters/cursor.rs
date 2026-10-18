//! Cursor: `~/.cursor/projects/<slug>/agent-transcripts/<id>/<id>.jsonl`, with
//! subagents in `<id>/subagents/<childId>.jsonl`. Records are `{role, message: {content}}`
//! plus `{type: "turn_ended", status, error?}`.
//!
//! The transcript has no tool results, no timestamps (beyond a `<timestamp>` line in most
//! prompts) and no cwd. The cwd comes from `<slug>`, the workspace path with every
//! non-alphanumeric char replaced by `-`. Cursor's database keeps the full conversation under
//! the same id; `show --db` reads it from there (see `cursor_db`).

use std::path::{Path, PathBuf};

use anyhow::Result;
use jiff::{Timestamp, civil, tz::Offset};
use serde_json::Value;

use super::{Adapter, count_unknown, finalize, jsonl_files, read_jsonl};
use crate::model::{Agent, Block, Role, Session};
use crate::text::str_field;

pub struct Cursor {
    pub home: PathBuf,
}

impl Adapter for Cursor {
    fn session_dirs(&self) -> Vec<PathBuf> {
        vec![self.home.join("projects")]
    }

    /// Only `*/agent-transcripts`: project dirs also hold large canvas, terminal and MCP trees.
    fn discover(&self) -> Vec<PathBuf> {
        let Ok(projects) = std::fs::read_dir(self.home.join("projects")) else { return Vec::new() };
        projects
            .flatten()
            .map(|p| p.path().join("agent-transcripts"))
            .filter(|d| d.is_dir())
            .flat_map(|d| jsonl_files(&d, 3).collect::<Vec<_>>())
            .filter(|p| self.is_session_file(p))
            .collect()
    }

    fn is_session_file(&self, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(self.home.join("projects")) else { return false };
        let parts: Vec<_> = rel.components().map(|c| c.as_os_str()).collect();
        match parts.as_slice() {
            [_, dir, id, file] => *dir == "agent-transcripts" && Path::new(file).file_stem() == Some(*id),
            [_, dir, _, sub, _] => *dir == "agent-transcripts" && *sub == "subagents",
            _ => false,
        }
    }

    fn load(&self, path: &Path) -> Result<Session> {
        let mut p =
            Parser { s: Session::new(Agent::Cursor, path.to_path_buf()), slug: slug(path), workdirs: Vec::new() };
        if is_subagent(path) {
            p.child_meta(path);
        }
        let bad = read_jsonl(path, |rec| p.record(rec))?;
        p.fallback_cwd();
        finalize(&mut p.s, bad);
        // Prompts carry the only timestamps; the file is appended as the agent works, so its
        // mtime marks the last activity.
        if let Ok(mtime) = std::fs::metadata(path).and_then(|m| m.modified())
            && let Ok(mtime) = Timestamp::try_from(mtime)
        {
            p.s.meta.updated = p.s.meta.updated.max(mtime);
        }
        Ok(p.s)
    }
}

fn is_subagent(path: &Path) -> bool {
    path.parent().and_then(Path::file_name).is_some_and(|n| n == "subagents")
}

/// The `<slug>` project dir above `agent-transcripts`.
fn slug(path: &Path) -> Option<String> {
    let transcripts = path.ancestors().find(|a| a.file_name().is_some_and(|n| n == "agent-transcripts"))?;
    Some(transcripts.parent()?.file_name()?.to_string_lossy().into_owned())
}

/// How Cursor names project dirs: the path with every non-alphanumeric char replaced by `-`.
fn slugify(path: &str) -> String {
    path.trim_matches('/').chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

struct Parser {
    s: Session,
    /// Unresolved project slug; cleared once the cwd is found.
    slug: Option<String>,
    /// Shell `working_directory` arguments, the cwd of last resort.
    workdirs: Vec<String>,
}

impl Parser {
    fn child_meta(&mut self, path: &Path) {
        let meta = &mut self.s.meta;
        let parent = path.parent().and_then(Path::parent).and_then(Path::file_name);
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        if let Some(parent) = parent {
            let parent = parent.to_string_lossy().into_owned();
            meta.id = format!("{parent}/{stem}");
            meta.parent_id = Some(parent);
        }
        meta.child_kind = Some("subagent".into());
    }

    fn record(&mut self, rec: Value) {
        if let Some(role) = str_field(&rec, "role") {
            let content = rec.pointer("/message/content").unwrap_or(&Value::Null);
            match role {
                "user" => {
                    let (ts, blocks) = self.prompt(content);
                    self.s.push(Role::User, ts, "user", blocks);
                }
                "assistant" => {
                    let blocks = self.blocks(content);
                    self.s.push(Role::Assistant, None, "assistant", blocks);
                }
                other => {
                    let kind = format!("role/{other}");
                    count_unknown(&mut self.s.meta, &kind);
                    self.s.push(Role::Event, None, kind, vec![Block::event("", rec)]);
                }
            }
            return;
        }
        let ty = str_field(&rec, "type").unwrap_or("?").to_string();
        if ty == "turn_ended" {
            let status = str_field(&rec, "status").unwrap_or("?");
            let summary = match str_field(&rec, "error") {
                Some(err) => format!("{status}: {err}"),
                None => status.to_string(),
            };
            self.s.push(Role::Event, None, ty, vec![Block::event(summary, rec)]);
        } else {
            count_unknown(&mut self.s.meta, &ty);
            self.s.push(Role::Event, None, ty, vec![Block::event("", rec)]);
        }
    }

    /// User text with the `<timestamp>` line turned into the record time and the
    /// `<user_query>` wrapper dropped. Other injected context stays verbatim.
    fn prompt(&mut self, content: &Value) -> (Option<Timestamp>, Vec<Block>) {
        let mut ts = None;
        let blocks = self
            .blocks(content)
            .into_iter()
            .map(|b| match b {
                Block::Text { text } => {
                    let (stamp, text) = unwrap_prompt(&text);
                    ts = ts.or(stamp);
                    Block::Text { text }
                }
                other => other,
            })
            .collect();
        (ts, blocks)
    }

    fn blocks(&mut self, content: &Value) -> Vec<Block> {
        let Some(items) = content.as_array() else {
            return vec![Block::text(crate::text::content_text(content))];
        };
        let mut out = Vec::new();
        for b in items {
            match str_field(b, "type").unwrap_or("") {
                "text" => out.push(Block::text(str_field(b, "text").unwrap_or_default())),
                "tool_use" => {
                    let name = str_field(b, "name").unwrap_or("?").to_string();
                    let args = b.get("input").cloned().unwrap_or(Value::Null);
                    self.cwd_evidence(&name, &args);
                    out.push(Block::ToolCall { id: None, name, args });
                }
                other => out.push(Block::event(format!("content block {other}"), b.clone())),
            }
        }
        out
    }

    /// Any absolute path in tool arguments whose ancestor slugifies to the project slug
    /// names the workspace exactly.
    fn cwd_evidence(&mut self, tool: &str, args: &Value) {
        if tool == "Shell"
            && let Some(dir) = str_field(args, "working_directory")
        {
            self.workdirs.push(dir.to_string());
        }
        let Some(slug) = &self.slug else { return };
        let mut found = None;
        each_string(args, &mut |s| {
            if found.is_none() && s.starts_with('/') {
                found =
                    Path::new(s).ancestors().find(|a| slugify(&a.to_string_lossy()) == *slug).map(Path::to_path_buf);
            }
        });
        if let Some(cwd) = found {
            self.s.meta.add_cwd(&cwd.to_string_lossy());
            self.slug = None;
        }
    }

    /// No path in the transcript matched: rebuild it from the filesystem, else use
    /// the directories shell commands ran in.
    fn fallback_cwd(&mut self) {
        let Some(slug) = self.slug.take() else { return };
        if let Some(cwd) = probe(Path::new("/"), &slug) {
            self.s.meta.add_cwd(&cwd.to_string_lossy());
            return;
        }
        for dir in std::mem::take(&mut self.workdirs) {
            self.s.meta.add_cwd(&dir);
        }
    }
}

fn each_string(v: &Value, f: &mut impl FnMut(&str)) {
    match v {
        Value::String(s) => f(s),
        Value::Array(items) => items.iter().for_each(|i| each_string(i, f)),
        Value::Object(map) => map.values().for_each(|i| each_string(i, f)),
        _ => {}
    }
}

/// Rebuild a directory from its slug by walking the filesystem, longest matching name first
/// (`a-b` could be the dir `a-b` or `a/b`).
fn probe(dir: &Path, rest: &str) -> Option<PathBuf> {
    if rest.is_empty() {
        return dir.is_dir().then(|| dir.to_path_buf());
    }
    let mut names: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| (slugify(&e.file_name().to_string_lossy()), e.path()))
        .filter(|(k, _)| !k.is_empty())
        .collect();
    names.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    names.into_iter().find_map(|(k, path)| {
        let tail = rest.strip_prefix(k.as_str())?;
        let tail = if tail.is_empty() { tail } else { tail.strip_prefix('-')? };
        probe(&path, tail)
    })
}

/// Split Cursor's prompt wrapper: `<timestamp>Monday, Jul 13, 2026, 2:21 PM (UTC-4)</timestamp>`
/// and `<user_query>…</user_query>`.
fn unwrap_prompt(text: &str) -> (Option<Timestamp>, String) {
    let mut ts = None;
    let mut out = text.to_string();
    if let Some((before, after)) = text.split_once("<timestamp>")
        && let Some((stamp, rest)) = after.split_once("</timestamp>")
        && let Some(parsed) = parse_stamp(stamp)
    {
        ts = Some(parsed);
        out = format!("{before}{rest}");
    }
    const OPEN: &str = "<user_query>";
    const CLOSE: &str = "</user_query>";
    if let (Some(a), Some(b)) = (out.find(OPEN), out.rfind(CLOSE))
        && a < b
    {
        out = format!("{}{}{}", &out[..a], out[a + OPEN.len()..b].trim(), &out[b + CLOSE.len()..]);
    }
    (ts, out.trim().to_string())
}

/// `Monday, Jul 13, 2026, 2:21 PM (UTC-4)`; the offset may be `UTC`, `UTC+2` or `UTC+5:30`.
fn parse_stamp(s: &str) -> Option<Timestamp> {
    let (_weekday, rest) = s.trim().split_once(", ")?;
    let (local, zone) = rest.rsplit_once(" (UTC")?;
    let zone = zone.strip_suffix(')')?;
    let dt = civil::DateTime::strptime("%b %d, %Y, %I:%M %p", local).ok()?;
    let secs = match zone.split_at_checked(1) {
        None => 0,
        Some((sign, hm)) => {
            let (h, m) = hm.split_once(':').unwrap_or((hm, "0"));
            let secs = h.parse::<i32>().ok()? * 3600 + m.parse::<i32>().ok()? * 60;
            if sign == "-" { -secs } else { secs }
        }
    };
    Offset::from_seconds(secs).ok()?.to_timestamp(dt).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_prompt_timestamps() {
        let ts = |s: &str| parse_stamp(s).map(|t| t.to_string());
        assert_eq!(ts("Monday, Jul 13, 2026, 2:21 PM (UTC-4)").as_deref(), Some("2026-07-13T18:21:00Z"));
        assert_eq!(ts("Friday, Aug 7, 2026, 9:05 AM (UTC+5:30)").as_deref(), Some("2026-08-07T03:35:00Z"));
        assert_eq!(ts("Friday, Aug 7, 2026, 12:05 AM (UTC)").as_deref(), Some("2026-08-07T00:05:00Z"));
        assert_eq!(ts("yesterday"), None);
    }

    #[test]
    fn unwraps_prompts() {
        let (ts, text) = unwrap_prompt(
            "<timestamp>Monday, Jul 13, 2026, 2:21 PM (UTC-4)</timestamp>\n<user_query>\nhi\n</user_query>",
        );
        assert!(ts.is_some());
        assert_eq!(text, "hi");
        let (ts, text) = unwrap_prompt("<attached_files>a.rs</attached_files>\n<user_query>\nfix it\n</user_query>");
        assert!(ts.is_none());
        assert_eq!(text, "<attached_files>a.rs</attached_files>\nfix it");
    }

    #[test]
    fn slugifies_like_cursor() {
        assert_eq!(slugify("/Users/me/Application Support/x.json"), "Users-me-Application-Support-x-json");
    }
}
