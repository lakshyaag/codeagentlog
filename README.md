# codeagentlog

A small, read-only CLI for finding and reading past **pi**, **Claude Code**, and **Codex**
conversations, organized by folder and agent. Built so coding agents (and humans) stop writing
throwaway `jq` parsers over multi-MB session logs.

## Install

Requires **Rust 1.85+** ([rustup](https://rustup.rs)).

From a clone of this repo:

```sh
cargo install --path . --locked
```

Once the GitHub remote exists, the same build can be installed without a local checkout:

```sh
cargo install --git https://github.com/<you>/codeagentlog --locked
```

`--locked` uses the committed `Cargo.lock` so the dependency set matches CI. Then tell
agents how to use it:

```sh
codeagentlog guide >> ~/.claude/CLAUDE.md   # or AGENTS.md
```

This crate is not on crates.io yet, so `cargo install codeagentlog` will not work until it
is published there.

## Usage

```sh
codeagentlog ls --recent 2                # sessions for cwd and below, newest first, with last 2 prompts
codeagentlog ls --near --since 2d         # also parent folders, only recent sessions
codeagentlog show @last --chat --tail 10  # where the previous session here left off
codeagentlog grep "benchmark-variant" -i  # each hit prints the show command to open it
codeagentlog show pi:01a11a9a --turns 57~5            # records 52..62
codeagentlog show codex:019f --turns 40..80 --tools full --thinking --events
codeagentlog show claude:d3d1 --role user             # only your prompts
codeagentlog tree ~/Projects --depth 2
codeagentlog path claude:d3d1             # transcript file
codeagentlog doctor                       # dirs, counts, parse errors, unknown record types
```

Session references: `agent:prefix`, any unique prefix, a file path, or `@last` / `@last:<agent>` (newest
session under cwd, excluding the one you're in).

All commands accept `--json`. Truncated output always ends with the exact command to continue.

Output is plain text when piped (what agents see). On a terminal it's colored and paged
(`less -FRX`); `--color always|never`, `NO_COLOR`, `FORCE_COLOR`, `--no-pager`, and
`$CODEAGENTLOG_PAGER` / `$PAGER` adjust that.

- **Records are numbered once, in file order**, across every stream and branch. Flags like
  `--events`, `--tools none`, or `--branch` hide records but never renumber, so `[N]` from
  `grep`, `ls`, or a footer always points at the same record.
- **Nothing is filtered by judgment.** Every record renders; you narrow with `--role`, `--turns`,
  `--agent`, `--since`, `--tools`, `--events`.
- **Hidden-by-default content** (thinking, Codex `event_msg`, inactive pi branches) is still
  searched by `grep`, and each such hit carries the `show` flags needed to see it.
- **Child sessions** (Claude subagents, Codex guardian/subagent threads) nest under their parent
  in `ls` (`--children` expands them) and are always searched by `grep`.

## Where data comes from

| Agent | Default location | Override |
|---|---|---|
| pi | `~/.pi/agent/sessions/` | `PI_CODING_AGENT_DIR` |
| Claude Code | `~/.claude/projects/` | `CLAUDE_CONFIG_DIR` |
| Codex | `~/.codex/sessions/`, `~/.codex/archived_sessions/` | `CODEX_HOME` |

Or pass `--root agent=PATH` (the agent's home dir, e.g. `--root codex=/tmp/codex`). Missing agents
are skipped. The tool never writes to agent directories; its only state is a metadata cache at
`$XDG_CACHE_HOME/codeagentlog/meta.sqlite` (default `~/.cache/…`), safe to delete anytime
(`--no-cache` bypasses it).

The caller's own session is marked `(current)` via `PI_SESSION_ID`, `CLAUDE_CODE_SESSION_ID`, or
`CODEX_THREAD_ID`.

## Development

```sh
cargo test                      # unit + end-to-end tests over tests/fixtures
UPDATE_GOLDEN=1 cargo test      # regenerate tests/golden/*.txt after intended output changes
```

Layout: `src/adapters/` parses each agent's format into the shared model (`src/model.rs`);
`src/catalog.rs` discovers, caches, and links sessions; `src/render.rs` turns records into text;
`src/cmd/` holds one file per command. See [docs/SPEC.md](docs/SPEC.md) for the design.

## License

MIT.
