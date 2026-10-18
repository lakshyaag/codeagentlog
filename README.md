# codeagentlog

[![CI](https://github.com/lakshyaag/codeagentlog/actions/workflows/ci.yml/badge.svg)](https://github.com/lakshyaag/codeagentlog/actions/workflows/ci.yml)

Find and read your past coding-agent conversations from the terminal:
[pi](https://github.com/badlogic/pi-mono), [Claude Code](https://www.anthropic.com/claude-code),
[Codex](https://github.com/openai/codex), and [Cursor](https://cursor.com), all in one place,
organized by the folder you worked in.

Every coding agent saves its history to disk, each in its own format, usually as multi-MB JSONL
files. Whenever I wanted to know "what did we try in this repo last week?", or worse, whenever an
agent wanted to know, the answer was another throwaway `jq` parser. This is the tool I wanted
instead. It's read-only, works offline, and its output is meant to be read by agents as much as by
people.

```console
$ codeagentlog grep "rate limit" --since 7d
claude:d3d1f0a2-5c1e-4b7a-9e0f-2a6c8b1d4e37  ~/code/api  2026-10-14  6 prompts  212 records
  [57] user: why do the retries keep hitting the rate limit?
  [61] assistant: The backoff resets on every request, so the client never actually waits…
  → codeagentlog show claude:d3d1f0a2-5c1e-4b7a-9e0f-2a6c8b1d4e37 --turns 57~5
```

Every hit ends with the exact command to open it.

## Install

Paste this into a terminal (macOS and Linux):

```sh
curl -fsSL https://github.com/lakshyaag/codeagentlog/releases/latest/download/codeagentlog-installer.sh | sh
```

On Windows, in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/lakshyaag/codeagentlog/releases/latest/download/codeagentlog-installer.ps1 | iex"
```

Or grab a binary from [Releases](https://github.com/lakshyaag/codeagentlog/releases), or build from
source with Rust 1.85+:

```sh
cargo install --git https://github.com/lakshyaag/codeagentlog --locked
```

### Or have your agent do it

Paste this into Claude Code, Codex, pi, or Cursor. The agent installs the tool, then shows you what
the skill install would write and waits for your OK:

```text
Install codeagentlog (https://github.com/lakshyaag/codeagentlog), a CLI for reading past coding-agent sessions.
1. Run: curl -fsSL https://github.com/lakshyaag/codeagentlog/releases/latest/download/codeagentlog-installer.sh | sh
   On Windows, run instead: powershell -ExecutionPolicy Bypass -c "irm https://github.com/lakshyaag/codeagentlog/releases/latest/download/codeagentlog-installer.ps1 | iex"
   It installs to ~/.cargo/bin; use that full path if the command isn't found yet.
2. Run `codeagentlog setup`. It writes nothing; it lists the skill files it would add. Show me that list and
   explain that `codeagentlog setup --yes` writes exactly those files. Only run it if I say yes.
3. Run `codeagentlog doctor` and tell me which agents' sessions it found.
```

## Quick start

```sh
codeagentlog ls                          # sessions in this folder and below, newest first
codeagentlog show @last --chat --tail 10 # where the last session here left off
codeagentlog grep "flaky test"           # search everything you've done with any agent
codeagentlog tree ~/code                 # which folders have sessions, per agent
```

## Let your agents use it

This is where it's most useful. Install the codeagentlog skill and your agents learn to use it on
their own:

```sh
codeagentlog setup
```

It lists the files it will add and asks before writing anything: one `SKILL.md` in
`~/.claude/skills/` (Claude Code and Cursor) and one in `~/.agents/skills/` (Codex, pi, and Cursor),
only for the agents you have. Run it again after upgrading to refresh the skill (`doctor` tells you
when it's out of date), and `codeagentlog setup --remove` takes it back out. When an agent runs
`setup` it can't answer the prompt, so it only gets the list and has to ask you before passing
`--yes`.

For an agent without skills support, paste the same instructions into its instructions file:

```sh
codeagentlog guide >> AGENTS.md
```

Now you can say "pick up where the last session left off" or "how did we fix this in the other
repo?" and the agent looks it up itself. Agents get plain text with no colors or pager, long output
is cut off with the exact command to see the rest, and the agent's own session is marked
`(current)` so it doesn't read itself back.

## Usage

### Find sessions

```sh
codeagentlog ls --recent 2               # also show the last 2 prompts of each session
codeagentlog ls --near --since 2d        # include parent folders, only the last 2 days
codeagentlog ls --all --agent codex      # every Codex session, anywhere
codeagentlog tree ~/Projects --depth 2
```

### Read one

```sh
codeagentlog show claude:d3d1                        # any unique id prefix works
codeagentlog show @last:codex                        # newest Codex session here
codeagentlog show pi:01a1 --turns 57~5               # records 52 to 62
codeagentlog show codex:019f --turns 40..80 --tools full --thinking --events
codeagentlog show claude:d3d1 --role user            # only your prompts
codeagentlog show cursor:6089 --db --tools full      # Cursor, with tool output (see below)
codeagentlog path claude:d3d1                        # where the transcript file lives
```

A session can be named by `agent:prefix`, any unique prefix, a file path, or `@last` /
`@last:<agent>` (the newest session under the current folder, not counting the one you're in).

### Everything else

- `--json` works on every command.
- `codeagentlog doctor` shows which agent dirs were found, how many sessions each has, and anything
  that failed to parse.
- On a terminal, output is colored and paged with `less -FRX`. `--color always|never`, `NO_COLOR`,
  `FORCE_COLOR`, `--no-pager`, and `$CODEAGENTLOG_PAGER` / `$PAGER` change that.

## How it behaves

- **Records are numbered once, in file order.** Flags like `--events`, `--tools none`, or
  `--branch` hide records but never renumber them, so a `[57]` from `grep` or `ls` always points
  at the same record.
- **Nothing is filtered by judgment.** Every record is shown. You narrow it down yourself with
  `--role`, `--turns`, `--agent`, `--since`, `--tools`, and `--events`.
- **Hidden doesn't mean unsearchable.** Thinking, Codex `event_msg` records, and abandoned pi
  branches are hidden by default but still searched by `grep`, and each hit includes the flags
  needed to see it.
- **Subagents stay with their parent.** Claude subagents, Codex guardian/subagent threads, and
  Cursor subagents nest under the session that started them in `ls` (`--children` expands them),
  and `grep` always searches them.

## Where the data comes from

| Agent | Default location | Override |
|---|---|---|
| pi | `~/.pi/agent/sessions/` | `PI_CODING_AGENT_DIR` |
| Claude Code | `~/.claude/projects/` | `CLAUDE_CONFIG_DIR` |
| Codex | `~/.codex/sessions/`, `~/.codex/archived_sessions/` | `CODEX_HOME` |
| Cursor | `~/.cursor/projects/*/agent-transcripts/` | `--root cursor=PATH` |

`--root agent=PATH` points any agent at a different home dir (e.g. `--root codex=/tmp/codex`).
Agents that aren't installed are skipped.

**Cursor:** its transcript files leave out tool output, thinking, and most timestamps. `show <id>
--db` reads the same conversation from Cursor's own database (`state.vscdb` in Cursor's app-data
dir, or `$CODEAGENTLOG_CURSOR_DB`). The database view has extra records, so its record numbers
don't match the plain view. `grep` only searches the transcripts.

**Privacy:** everything stays on your machine. codeagentlog never writes to any agent's files
(Cursor's database is opened read-only, without locking it) and makes no network requests. The one
exception is `setup`, which adds its own skill folder only after you approve it. Its only other
state is a metadata cache at `$XDG_CACHE_HOME/codeagentlog/meta.sqlite` (default `~/.cache/…`),
which is safe to delete anytime. `--no-cache` skips it.

## Development

```sh
cargo test                      # unit + end-to-end tests over tests/fixtures
UPDATE_GOLDEN=1 cargo test      # regenerate tests/golden/*.txt after intended output changes
```

Each agent's format is parsed into a shared model in `src/adapters/` (`src/model.rs`).
`src/catalog.rs` finds, caches, and links sessions, `src/render.rs` turns records into text, and
`src/cmd/` has one file per command. The design is in [docs/SPEC.md](docs/SPEC.md).

Adding another agent means writing one adapter. Issues and PRs are welcome.

## License

[MIT](LICENSE)
