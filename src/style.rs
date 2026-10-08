//! Terminal presentation: ANSI color and paging, for humans only.
//!
//! Color is on when stdout is a TTY, unless `NO_COLOR` is set or `TERM=dumb`;
//! `FORCE_COLOR` / `CLICOLOR_FORCE` or `--color always` force it on. Piped output
//! (what agents see) stays plain, so its text is identical with or without color.

use std::io::{IsTerminal, Write};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use crate::model::{Agent, Role};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

static ENABLED: OnceLock<bool> = OnceLock::new();

pub fn init(choice: ColorChoice) {
    let env = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    let on = match choice {
        ColorChoice::Always => true,
        ColorChoice::Never => false,
        ColorChoice::Auto if env("NO_COLOR") => false,
        ColorChoice::Auto if env("FORCE_COLOR") || env("CLICOLOR_FORCE") => true,
        ColorChoice::Auto => std::io::stdout().is_terminal() && std::env::var("TERM").ok().as_deref() != Some("dumb"),
    };
    let _ = ENABLED.set(on);
}

fn enabled() -> bool {
    *ENABLED.get().unwrap_or(&false)
}

fn paint(s: &str, code: &str) -> String {
    if enabled() && !s.is_empty() { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
}

pub fn bold(s: &str) -> String {
    paint(s, "1")
}

pub fn dim(s: &str) -> String {
    paint(s, "2")
}

pub fn heading(s: &str) -> String {
    paint(s, "1;4")
}

pub fn path(s: &str) -> String {
    paint(s, "34")
}

pub fn highlight(s: &str) -> String {
    paint(s, "1;31")
}

pub fn agent(a: Agent, s: &str) -> String {
    paint(
        s,
        match a {
            Agent::Pi => "1;35",
            Agent::Claude => "1;33",
            Agent::Codex => "1;36",
        },
    )
}

pub fn role(r: &Role, s: &str) -> String {
    paint(
        s,
        match r {
            Role::User => "1;32",
            Role::Assistant => "1;34",
            Role::Tool | Role::Bash => "33",
            Role::Event | Role::System | Role::Developer | Role::Custom(_) => "2",
        },
    )
}

/// `error:` label for stderr, colored when stderr is an interactive terminal.
pub fn error_label() -> &'static str {
    let tty = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    if tty { "\x1b[1;31merror:\x1b[0m" } else { "error:" }
}

/// `-- note. next: <command>` footer: dim note, bold command.
pub fn footer(note: &str, command: Option<&str>) -> String {
    match command {
        Some(cmd) => format!("{} {}", dim(&format!("-- {note}")), bold(cmd)),
        None => dim(&format!("-- {note}")),
    }
}

/// Color a plain transcript line by line. `show` budgets and cuts plain text,
/// so styling is applied afterwards based on each line's prefix.
pub fn transcript(text: &str) -> String {
    if !enabled() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    for line in text.split_inclusive('\n') {
        let (body, nl) = line.strip_suffix('\n').map_or((line, ""), |b| (b, "\n"));
        out.push_str(&transcript_line(body));
        out.push_str(nl);
    }
    out
}

fn transcript_line(line: &str) -> String {
    if let Some(rest) = line.strip_prefix("## [") {
        // `## [N] role · meta…`
        let (num, tail) = rest.split_once("] ").unwrap_or((rest, ""));
        let (role_name, meta) = tail.split_once(" · ").map_or((tail, None), |(r, m)| (r, Some(m)));
        let r = Role::parse(role_name).unwrap_or(Role::Event);
        let meta = meta.map(|m| dim(&format!(" · {m}"))).unwrap_or_default();
        return format!("{}{}{meta}", dim(&format!("## [{num}] ")), role(&r, role_name));
    }
    if line.starts_with("# ") {
        return dim(line);
    }
    if line.starts_with("-- ") {
        return match line.split_once(": codeagentlog ") {
            Some((note, cmd)) => format!("{}{}", dim(&format!("{note}: ")), bold(&format!("codeagentlog {cmd}"))),
            None => dim(line),
        };
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with("→ ") {
        return paint(line, "36");
    }
    if trimmed.starts_with("← ") || trimmed.starts_with("(last) ") || trimmed == "…" {
        return paint(line, "33");
    }
    if line.starts_with("· ")
        || line.starts_with("[thinking")
        || line.starts_with("[reasoning")
        || trimmed.starts_with("[image ")
    {
        return dim(line);
    }
    if trimmed.starts_with("… ") && trimmed.contains("chars omitted") {
        return dim(line);
    }
    line.to_string()
}

/// Print output: through a pager for interactive terminals, directly otherwise.
pub fn emit(text: &str, allow_pager: bool) {
    if allow_pager && std::io::stdout().is_terminal() && page(text) {
        return;
    }
    let mut stdout = std::io::stdout().lock();
    // Ignore broken pipes (`| head`).
    let _ = stdout.write_all(text.as_bytes()).and_then(|_| stdout.flush());
}

/// `$CODEAGENTLOG_PAGER`, `$PAGER`, or `less -FRX` (quits if one screen, keeps color).
fn page(text: &str) -> bool {
    let pager = std::env::var("CODEAGENTLOG_PAGER")
        .or_else(|_| std::env::var("PAGER"))
        .ok()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "less".into());
    if pager.trim() == "cat" {
        return false;
    }
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(&pager).stdin(Stdio::piped());
    if std::env::var_os("LESS").is_none() {
        cmd.env("LESS", "FRX");
    }
    let Ok(mut child) = cmd.spawn() else { return false };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    child.wait().is_ok()
}
