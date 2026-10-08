//! End-to-end tests over small synthetic fixtures shaped like real pi / Claude Code / Codex logs.
//!
//! Golden outputs live in `tests/golden/`; regenerate with `UPDATE_GOLDEN=1 cargo test`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The binary pointed at the fixtures, with a clean, deterministic environment.
fn command() -> Command {
    let fx = fixtures();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_codeagentlog"));
    for var in [
        "PI_SESSION_ID",
        "PI_SESSION_FILE",
        "PI_CODING_AGENT",
        "PI_CODING_AGENT_DIR",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDECODE",
        "CLAUDE_CONFIG_DIR",
        "CODEX_THREAD_ID",
        "CODEX_SANDBOX",
        "CODEX_CI",
        "CODEX_HOME",
    ] {
        cmd.env_remove(var);
    }
    cmd.env("TZ", "UTC")
        .env("HOME", &fx)
        .arg("--root")
        .arg(format!("pi={}", fx.join("pi").display()))
        .arg("--root")
        .arg(format!("claude={}", fx.join("claude").display()))
        .arg("--root")
        .arg(format!("codex={}", fx.join("codex").display()));
    cmd
}

fn run_env(args: &[&str], env: &[(&str, &str)]) -> (String, String, bool) {
    let mut cmd = command();
    cmd.arg("--no-cache").args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

fn run(args: &[&str]) -> String {
    let (stdout, stderr, ok) = run_env(args, &[]);
    assert!(ok, "codeagentlog {args:?} failed: {stderr}");
    stdout
}

/// Golden files compare text with relative ages ("(3d ago)") normalized, since they depend on now.
fn golden(name: &str, actual: &str) {
    let ages = regex::Regex::new(r"\((?:now|\d+(?:m|h|d|w|mo|y) ago)\)").unwrap();
    let actual = &*ages.replace_all(actual, "(AGE)");
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(format!("{name}.txt"));
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing {}; run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(actual, expected, "golden mismatch for {name}");
}

#[test]
fn ls_groups_by_agent_with_titles_and_children() {
    let out = run(&["ls", "/work/proj", "--prompts", "2"]);
    golden("ls", &out);
    assert!(out.contains("\"Benchmark variant planning\"  (session_info)"));
    assert!(out.contains("\"Dataset sync flow\"  (ai-title)"));
    assert!(out.contains("\"Reward grader design\"  (thread_name)"));
    assert!(out.contains("+1 children (general-purpose×1)"));
    assert!(out.contains("+1 children (guardian×1)"));
}

#[test]
fn ls_scopes() {
    assert!(!run(&["ls", "/work/proj", "--here"]).contains("codex:"));
    assert!(run(&["ls", "/work/proj/sub", "--here"]).contains("codex:0199cccc"));
    assert!(run(&["ls", "/work/proj/sub/deeper", "--near"]).contains("pi:0199aaaa"));
    assert!(run(&["ls", "/elsewhere"]).starts_with("no sessions in /elsewhere"));
}

#[test]
fn ls_limit_footer_and_current_marker() {
    let (out, _, ok) =
        run_env(&["ls", "/work", "--limit", "1"], &[("PI_SESSION_ID", "0199aaaa-0000-7000-8000-000000000001")]);
    assert!(ok);
    assert!(out.contains("-- showing 1 of 3. more: codeagentlog ls /work --limit 21"));
    let (out, _, _) = run_env(&["ls", "/work"], &[("PI_SESSION_ID", "0199aaaa-0000-7000-8000-000000000001")]);
    assert!(out.contains("(current)"));
}

#[test]
fn show_pi_follows_active_branch() {
    let out = run(&["show", "pi:0199aaaa"]);
    golden("show_pi", &out);
    assert!(!out.contains("walruses"), "off-branch record hidden by default");
    assert!(out.contains("## [7] assistant"), "numbering stays stable across hidden records");
    assert!(out.contains("-- 1 other branches: --branch e5 | --all-branches"));
    assert!(out.contains("[thinking, 24 chars]"));

    let all = run(&["show", "pi:0199aaaa", "--all-branches", "--thinking"]);
    assert!(all.contains("## [6] assistant · 10:00:04"));
    assert!(all.contains("## [7] assistant · 10:00:05 · branches from [5]"));
    assert!(all.contains("secret plan about zebras"));

    let branch = run(&["show", "pi:0199aaaa", "--branch", "e5"]);
    assert!(branch.contains("walruses") && !branch.contains("Two files found"));
}

#[test]
fn show_claude_merges_split_messages_and_links_children() {
    let out = run(&["show", "claude:1111"]);
    golden("show_claude", &out);
    assert!(out.contains("## [3] assistant"));
    assert!(out.contains("  → Bash {\"command\":\"ls\"}\n  → Agent"), "blocks of msg_1 merged into one record");
    assert!(out.contains("→ child claude:11111111-2222-4333-8444-555555555555/abc123"));
    assert!(out.contains("## [4] tool · Bash · ok"));
}

#[test]
fn show_codex_hides_event_stream_by_default() {
    let out = run(&["show", "codex:0199cccc-0000-7000-8000-000000000001"]);
    golden("show_codex", &out);
    assert!(!out.contains("penguins"));
    assert!(out.contains("# title \"Reward grader design\" (thread_name)"));
    assert!(out.contains("[reasoning, encrypted]"));
    let events = run(&["show", "codex:0199cccc-0000-7000-8000-000000000001", "--events"]);
    assert!(events.contains("## [10] event"));
    assert!(events.contains("penguins"));
}

#[test]
fn show_paginates_and_cuts_oversized_records() {
    let out = run(&["show", "claude:1111", "--max-chars", "300"]);
    assert!(out.contains("-- truncated at 300 chars"));
    let next = out.lines().find_map(|l| l.split("next: ").nth(1)).expect("footer");
    assert!(next.starts_with("codeagentlog show claude:11111111-2222-4333-8444-555555555555 --turns "));
    assert!(next.ends_with("--max-chars 300"));

    let tiny = run(&["show", "claude:1111", "--turns", "3", "--max-chars", "40"]);
    assert!(tiny.contains("chars omitted; full: codeagentlog show claude:11111111-2222-4333-8444-555555555555 --turns 3 --tools full --max-chars 0"));
}

#[test]
fn show_tools_modes() {
    let none = run(&["show", "pi:0199aaaa", "--tools", "none"]);
    assert!(!none.contains("## [5] tool") && !none.contains("→ bash"));
    let full = run(&["show", "pi:0199aaaa", "--tools", "full"]);
    assert!(full.contains("    a.txt\n    b.txt"));
}

#[test]
fn show_accepts_raw_paths() {
    let path = fixtures()
        .join("codex/sessions/2026/10/03/rollout-2026-10-03T08-00-05-0199cccc-0000-7000-8000-000000000002.jsonl");
    let out = run(&["show", path.to_str().unwrap()]);
    assert!(out.contains("# guardian of codex:0199cccc-0000-7000-8000-000000000001"));
}

#[test]
fn grep_reports_hidden_hits_with_reveal_commands() {
    let out = run(&["grep", "zebras", "/work"]);
    assert!(out.contains("[4] assistant thinking:"));
    assert!(out.contains("→ codeagentlog show pi:0199aaaa-0000-7000-8000-000000000001 --turns 4 --thinking"));

    let out = run(&["grep", "walrus", "/work"]);
    golden("grep_walrus", &out);
    assert!(out.contains("--turns 6 --branch e5"));
    assert!(out.contains("child of claude:11111111-2222-4333-8444-555555555555"));

    assert!(run(&["grep", "penguins", "/work"]).starts_with("no matches"));
    assert!(run(&["grep", "penguins", "/work", "--events"]).contains("--turns 10 --events"));
}

#[test]
fn grep_options() {
    assert!(run(&["grep", "ZEBRAS", "/work"]).starts_with("no matches"));
    assert!(run(&["grep", "ZEBRAS", "/work", "-i"]).contains("zebras"));
    assert!(run(&["grep", r"grad(er|e)\b", "/work", "--regex"]).contains("codex:"));
    let roles = run(&["grep", "sync", "/work", "--role", "assistant"]);
    assert!(roles.contains("[7] assistant") && !roles.contains("[2] user"));
}

#[test]
fn ids_resolve_by_prefix_and_report_ambiguity() {
    let out = run(&["path", "0199aaaa"]);
    assert!(out.trim_end().ends_with("0199aaaa-0000-7000-8000-000000000001.jsonl"));
    let (_, err, ok) = run_env(&["path", "codex:0199cccc"], &[]);
    assert!(!ok && err.contains("ambiguous (2 matches)"));
    let (_, err, ok) = run_env(&["path", "nope"], &[]);
    assert!(!ok && err.contains("no session matches"));
}

#[test]
fn tree_counts_roots_and_children() {
    let out = run(&["tree", "/work"]);
    // The `last` column is relative to now; keep the golden file stable.
    let stable: String = out
        .lines()
        .map(|l| match l.trim_end().rsplit_once(' ') {
            Some((head, _)) if !l.starts_with("--") => format!("{}\n", head.trim_end()),
            _ => format!("{l}\n"),
        })
        .collect();
    golden("tree", &stable);
    assert!(out.contains("1+1"));
}

#[test]
fn doctor_counts_unknown_records() {
    let out = run(&["doctor"]);
    assert!(out.contains("unknown record types: mystery_record×1"));
    assert!(out.contains("files 2  sessions 1  children 1"));
}

#[test]
fn json_output_parses() {
    for args in [
        vec!["ls", "/work", "--json"],
        vec!["show", "pi:0199aaaa", "--json"],
        vec!["grep", "sync", "/work", "--json"],
        vec!["tree", "/work", "--json"],
        vec!["doctor", "--json"],
        vec!["path", "pi:0199aaaa", "--json"],
        vec!["guide", "--json"],
    ] {
        let out = run(&args);
        serde_json::from_str::<serde_json::Value>(&out).unwrap_or_else(|e| panic!("{args:?}: {e}\n{out}"));
    }
}

#[test]
fn cache_round_trip_matches_fresh_parse() {
    let dir = std::env::temp_dir().join(format!("codeagentlog-test-{}", std::process::id()));
    let cached = || {
        let out = command().env("XDG_CACHE_HOME", &dir).args(["ls", "/work"]).output().unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    let first = cached();
    let second = cached();
    assert!(dir.join("codeagentlog/meta.sqlite").exists());
    assert_eq!(first, second);
    assert_eq!(first, run(&["ls", "/work"]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn show_chat_role_and_around() {
    let chat = run(&["show", "claude:1111", "--chat"]);
    assert!(chat.contains("## [2] user") && chat.contains("## [7] assistant"));
    assert!(!chat.contains("## [3]") && !chat.contains("## [4] tool") && !chat.contains("· attachment"));

    let roles = run(&["show", "pi:0199aaaa", "--role", "user,bash"]);
    assert!(roles.contains("## [3] user") && roles.contains("## [10] bash") && !roles.contains("## [4] assistant"));

    // --tail counts records in the current view, not raw records.
    let tail = run(&["show", "claude:1111", "--chat", "--tail", "1"]);
    assert!(tail.contains("## [7] assistant") && !tail.contains("## [2] user"));

    let around = run(&["show", "pi:0199aaaa", "--turns", "4~1"]);
    assert!(around.contains("## [3]") && around.contains("## [5]") && !around.contains("## [7]"));
}

#[test]
fn last_shortcut_skips_current_session() {
    // Build two pi sessions whose cwd is a real temp folder, then run from that folder.
    let tmp = std::env::temp_dir().join(format!("codeagentlog-last-{}", std::process::id()));
    let project = tmp.join("project");
    let sessions = tmp.join("pi/sessions/proj");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&sessions).unwrap();
    let project = project.canonicalize().unwrap();
    for (id, minute) in [("older", 1), ("newer", 2)] {
        let header = serde_json::json!({"type": "session", "id": id, "cwd": project, "timestamp": format!("2026-10-01T10:0{minute}:00Z")});
        std::fs::write(sessions.join(format!("{id}.jsonl")), format!("{header}\n")).unwrap();
    }
    let path = |env: &[(&str, &str)]| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_codeagentlog"));
        cmd.current_dir(&project)
            .env_remove("PI_SESSION_ID")
            .env_remove("PI_SESSION_FILE")
            .env_remove("PI_CODING_AGENT");
        let root = |agent: &str| format!("{agent}={}", tmp.join(agent).display());
        cmd.arg("--no-cache");
        for agent in ["pi", "claude", "codex"] {
            cmd.args(["--root", &root(agent)]);
        }
        cmd.args(["path", "@last:pi"]);
        cmd.envs(env.iter().copied());
        String::from_utf8(cmd.output().unwrap().stdout).unwrap()
    };
    assert!(path(&[]).contains("newer.jsonl"));
    assert!(path(&[("PI_SESSION_ID", "newer")]).contains("older.jsonl"), "the current session is skipped");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn ls_recent_prompts() {
    let out = run(&["ls", "/work/proj", "--recent", "1", "--agent", "codex"]);
    assert!(out.contains("last: design the reward grader"));
    assert!(!out.contains("AGENTS.md"));
}

#[test]
fn grep_suggests_show_command_and_marks_current() {
    let (out, _, _) =
        run_env(&["grep", "sync", "/work"], &[("CLAUDE_CODE_SESSION_ID", "11111111-2222-4333-8444-555555555555")]);
    assert!(out.contains("(current)"));
    assert!(out.contains("→ codeagentlog show claude:11111111-2222-4333-8444-555555555555 --turns 2~5"));
}

#[test]
fn grep_ignores_bookkeeping_ids() {
    assert!(run(&["grep", "11111111-2222", "/work"]).starts_with("no matches"));
}

#[test]
fn color_only_when_asked_or_on_a_terminal() {
    assert!(!run(&["ls", "/work"]).contains('\x1b'), "piped output stays plain");
    assert!(run(&["ls", "/work", "--color", "always"]).contains("\x1b["));
    assert!(!run(&["ls", "/work", "--color", "always", "--json"]).contains('\x1b'));
}
