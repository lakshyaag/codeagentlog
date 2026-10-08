//! codeagentlog: find and read past pi / Claude Code / Codex conversations.

mod adapters;
mod branches;
mod cache;
mod catalog;
mod cmd;
mod current;
mod model;
mod render;
mod scope;
mod text;
mod time;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::model::Agent;
use crate::render::ToolsMode;
use crate::scope::Scope;

#[derive(Parser)]
#[command(
    name = "codeagentlog",
    version,
    about = "Find and read past coding-agent conversations (pi, Claude Code, Codex)."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Output JSON instead of text.
    #[arg(long, global = true)]
    json: bool,

    /// Override an agent's data dir, e.g. `--root codex=/tmp/codex-home`. Repeatable.
    #[arg(long = "root", global = true, value_name = "AGENT=PATH")]
    roots: Vec<String>,

    /// Don't read or write the metadata cache.
    #[arg(long, global = true)]
    no_cache: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Folder tree with per-agent session counts.
    Tree(TreeArgs),
    /// List sessions for a folder (default: cwd and below).
    Ls(LsArgs),
    /// Print a session transcript.
    Show(ShowArgs),
    /// Search transcripts (rendered text, not raw JSON).
    Grep(GrepArgs),
    /// Print the transcript file path of a session.
    Path {
        /// Session id (`agent:prefix` or unique prefix).
        id: String,
    },
    /// Per-agent health: dirs, counts, parse errors, unknown record types.
    Doctor,
    /// A paragraph to paste into AGENTS.md / CLAUDE.md.
    Guide,
}

#[derive(Args, Clone)]
pub struct Filters {
    /// Only these agents (repeatable or comma-separated).
    #[arg(long, value_enum, value_delimiter = ',')]
    agent: Vec<Agent>,

    /// Only sessions updated since: 30m, 12h, 3d, 2w, or 2026-10-01.
    #[arg(long, value_name = "DUR|DATE")]
    since: Option<String>,
}

#[derive(Args, Clone, Default)]
#[group(multiple = false)]
pub struct ScopeArgs {
    /// Sessions whose cwd is exactly PATH.
    #[arg(long)]
    here: bool,
    /// PATH and all descendants (default).
    #[arg(long)]
    under: bool,
    /// --under plus ancestors of PATH.
    #[arg(long)]
    near: bool,
    /// Everything, regardless of folder.
    #[arg(long)]
    all: bool,
}

impl ScopeArgs {
    fn scope(&self) -> Scope {
        match (self.here, self.near, self.all) {
            (true, _, _) => Scope::Here,
            (_, true, _) => Scope::Near,
            (_, _, true) => Scope::All,
            _ => Scope::Under,
        }
    }
}

#[derive(Args)]
pub struct TreeArgs {
    /// Root folder (default: ~).
    path: Option<PathBuf>,
    /// Max folder depth to expand below the root.
    #[arg(long)]
    depth: Option<usize>,
    #[command(flatten)]
    filters: Filters,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Group {
    Agent,
    Folder,
    None,
}

#[derive(Args)]
pub struct LsArgs {
    /// Folder (default: cwd).
    path: Option<PathBuf>,
    #[command(flatten)]
    scope: ScopeArgs,
    #[command(flatten)]
    filters: Filters,
    /// Also print the first N user messages of each session.
    #[arg(long, default_value_t = 0, value_name = "N")]
    prompts: usize,
    #[arg(long, value_enum, default_value = "agent")]
    group: Group,
    /// Expand child sessions (subagents, reviewers) under their parent.
    #[arg(long)]
    children: bool,
    /// Max sessions to list.
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Args)]
pub struct ShowArgs {
    /// Session id (`agent:prefix` or unique prefix) or transcript file path.
    id: String,
    /// Record range: `A..B`, `A..`, `..B`, or `N` (inclusive).
    #[arg(long, value_name = "A..B", conflicts_with = "tail")]
    turns: Option<String>,
    /// Only the last N visible records.
    #[arg(long, value_name = "N")]
    tail: Option<usize>,
    #[arg(long, value_enum, default_value = "brief")]
    tools: ToolsMode,
    /// Show thinking / reasoning text.
    #[arg(long)]
    thinking: bool,
    /// Include secondary event streams (Codex `event_msg`).
    #[arg(long)]
    events: bool,
    /// Show the branch ending at this entry id (pi).
    #[arg(long, value_name = "ENTRY_ID", conflicts_with = "all_branches")]
    branch: Option<String>,
    /// Show records from every branch, in file order (pi).
    #[arg(long)]
    all_branches: bool,
    /// Output budget in chars; 0 = unlimited.
    #[arg(long, default_value_t = 20000)]
    max_chars: usize,
}

#[derive(Args)]
pub struct GrepArgs {
    pattern: String,
    /// Folder (default: cwd).
    path: Option<PathBuf>,
    #[command(flatten)]
    scope: ScopeArgs,
    #[command(flatten)]
    filters: Filters,
    /// Only records with these roles: user, assistant, tool, event, system, developer, bash.
    #[arg(long, value_delimiter = ',')]
    role: Vec<String>,
    /// Case-insensitive.
    #[arg(short = 'i', long)]
    ignore_case: bool,
    /// Treat PATTERN as a regular expression.
    #[arg(long)]
    regex: bool,
    /// Also search secondary event streams (Codex `event_msg`).
    #[arg(long)]
    events: bool,
    /// Chars of context around each match.
    #[arg(long, default_value_t = 80)]
    context: usize,
    /// Max sessions to report.
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = cmd::Ctx::new(&cli.roots, cli.json, !cli.no_cache).and_then(|ctx| match cli.command {
        Command::Tree(a) => cmd::tree::run(&ctx, a),
        Command::Ls(a) => cmd::ls::run(&ctx, a),
        Command::Show(a) => cmd::show::run(&ctx, a),
        Command::Grep(a) => cmd::grep::run(&ctx, a),
        Command::Path { id } => cmd::path::run(&ctx, &id),
        Command::Doctor => cmd::doctor::run(&ctx),
        Command::Guide => cmd::guide::run(&ctx),
    });
    match result {
        Ok(out) => {
            let mut stdout = std::io::stdout().lock();
            // Ignore broken pipes (`| head`).
            let _ = stdout.write_all(out.as_bytes()).and_then(|_| stdout.flush());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("codeagentlog: {e:#}");
            ExitCode::FAILURE
        }
    }
}
