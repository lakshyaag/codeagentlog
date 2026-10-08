//! Small string helpers shared by adapters and renderers.

use std::path::Path;
use std::sync::OnceLock;

use serde_json::Value;

/// Truncate to at most `max` chars, appending `…` when cut.
pub fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        None => s.to_string(),
        Some((cut, _)) => format!("{}…", &s[..cut]),
    }
}

/// Collapse all whitespace runs into single spaces.
pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One-lined and truncated.
pub fn snippet(s: &str, max: usize) -> String {
    truncate(&one_line(s), max)
}

pub fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { prefix.trim_end().to_string() } else { format!("{prefix}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn compact_json(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_default()
}

pub fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// Text of a content value: a plain string, or an array of blocks whose
/// `text` fields are joined with newlines.
pub fn content_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|b| match b {
                Value::String(s) => Some(s.as_str()),
                _ => str_field(b, "text"),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Approximate decoded size of base64 data.
pub fn base64_bytes(data: &str) -> usize {
    data.len() / 4 * 3
}

pub fn home_dir() -> Option<&'static Path> {
    static HOME: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    HOME.get_or_init(|| std::env::var_os("HOME").map(Into::into)).as_deref()
}

/// Display a path with the home directory abbreviated to `~`.
pub fn tilde(p: &Path) -> String {
    if let Some(home) = home_dir()
        && let Ok(rest) = p.strip_prefix(home)
    {
        return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
    }
    p.display().to_string()
}

/// Quote a value for a shell command line if needed.
pub fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+~".contains(c));
    if safe { s.to_string() } else { format!("'{}'", s.replace('\'', r"'\''")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_on_char_boundaries() {
        assert_eq!(truncate("héllo", 2), "hé…");
        assert_eq!(truncate("hi", 5), "hi");
    }

    #[test]
    fn extracts_content_text() {
        let v = serde_json::json!([{"type":"text","text":"a"},{"type":"image"},{"type":"input_text","text":"b"}]);
        assert_eq!(content_text(&v), "a\nb");
    }

    #[test]
    fn quotes_shell_args() {
        assert_eq!(shell_quote("pi:abc"), "pi:abc");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
