//! codeagentlog: find and read past pi / Claude Code / Codex / Cursor conversations.

mod adapters;
mod branches;
mod cache;
mod catalog;
mod cmd;
mod current;
mod model;
mod render;
mod scope;
mod style;
mod text;
mod time;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::model::Agent;
use crate::render::ToolsMode;
use crate::scope::Scope;
use crate::style::ColorChoice;

const WORKFLOW: &str = "\
Workflow:
  codeagentlog ls                          sessions for this folder (newest first; --near/--all to widen)
  codeagentlog show @last --chat --tail 10 where the previous session left off
  codeagentlog grep <keyword>              find the session/record; each hit prints a show command
  codeagentlog show <id> --turns 57~5      records 52..62 around a hit
  codeagentlog tree                        which folders have sessions

Session ids: `agent:prefix` (e.g. pi:01a1), any unique prefix, a file path,
or @last / @last:<agent> (newest session here, excluding the current one).
Truncated output always ends with the exact command to continue.";

#[derive(Parser)]
#[command(
    name = "codeagentlog",
    version,
    about = "Find and read past coding-agent conversations (pi, Claude Code, Codex, Cursor).",
    after_help = WORKFLOW
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Output JSON instead of text.
    #[arg(long, global = true)]
    json: bool,

    /// Color output (auto: only on a terminal, honoring NO_COLOR).
    #[arg(long, global = true, value_enum, default_value = "auto")]
    color: ColorChoice,

    /// Don't page long output on a terminal.
    #[arg(long, global = true)]
    no_pager: bool,

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
    /// List sessions for a folder (default: cwd and below), newest first.
    #[command(
        after_help = "Examples:\n  codeagentlog ls --recent 2\n  codeagentlog ls --near --since 2d\n  codeagentlog ls --all --agent claude --limit 50"
    )]
    Ls(LsArgs),
    /// Print a session transcript.
    #[command(
        after_help = "Examples:\n  codeagentlog show @last --chat --tail 10\n  codeagentlog show pi:01a1 --turns 40..80 --tools full\n  codeagentlog show codex:019f --turns 57~5 --thinking --events\n  codeagentlog show cursor:6089 --db --tools full"
    )]
    Show(ShowArgs),
    /// Search transcripts (rendered text, not raw JSON).
    #[command(
        after_help = "Examples:\n  codeagentlog grep 'benchmark-variant' -i\n  codeagentlog grep 'k8s sync' --all --role user,assistant\n  codeagentlog grep 'reward (model|grader)' --regex --since 1w"
    )]
    Grep(GrepArgs),
    /// Print the transcript file path of a session.
    Path {
        /// Session id (`agent:prefix`, unique prefix, or @last[:agent]).
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
    /// Also print the last N user messages of each session (where it left off).
    #[arg(long, default_value_t = 0, value_name = "N")]
    recent: usize,
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
    /// Session id (`agent:prefix`, unique prefix, @last[:agent]) or transcript file path.
    id: String,
    /// Record range: `A..B`, `A..`, `..B`, `N`, or `N~K` (N±K). Inclusive.
    #[arg(long, value_name = "A..B", conflicts_with = "tail")]
    turns: Option<String>,
    /// Only the last N records of this view.
    #[arg(long, value_name = "N")]
    tail: Option<usize>,
    /// Only these roles: user, assistant, tool, event, system, developer, bash, custom:<type>.
    #[arg(long, value_delimiter = ',')]
    role: Vec<String>,
    /// Conversation only: user and assistant text (no tools, thinking, or events).
    #[arg(long)]
    chat: bool,
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
    /// Cursor only: read the conversation from Cursor's database instead of the transcript,
    /// adding timestamps, thinking and tool results. Records are numbered in database order.
    #[arg(long, conflicts_with_all = ["branch", "all_branches"])]
    db: bool,
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
    style::init(if cli.json { ColorChoice::Never } else { cli.color });
    let paged =
        !cli.json && !cli.no_pager && matches!(cli.command, Command::Show(_) | Command::Grep(_) | Command::Ls(_));
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
            style::emit(&out, paged);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("codeagentlog {} {e:#}", style::error_label());
            ExitCode::FAILURE
        }
    }
}
