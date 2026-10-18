# codeagentlog

[![CI](https://github.com/lakshyaag/codeagentlog/actions/workflows/ci.yml/badge.svg)](https://github.com/lakshyaag/codeagentlog/actions/workflows/ci.yml)

Find and read your past coding-agent conversations from the terminal:
[pi](https://github.com/badlogic/pi-mono), [Claude Code](https://www.anthropic.com/claude-code),
[Codex](https://github.com/openai/codex), and [Cursor](https://cursor.com), all in one place,
organized by the folder you worked in.

Every coding agent saves its history to disk, each in its own format, usually as multi-MB JSONL
files. Whenever I wanted "what did we try in this repo last week?" — or worse, whenever an agent
wanted to know — the answer was another throwaway `jq` parser. This is the tool I wanted instead:
read-only, works offline, meant to be read by agents as much as by people.

```console
$ codeagentlog grep "rate limit" --since 7d
claude:d3d1f0a2-5c1e-4b7a-9e0f-2a6c8b1d4e37  ~/code/api  2026-10-14  6 prompts  212 records
  [57] user: why do the retries keep hitting the rate limit?
  [61] assistant: The backoff resets on every request, so the client never actually waits…
  → codeagentlog show claude:d3d1f0a2-5c1e-4b7a-9e0f-2a6c8b1d4e37 --turns 57~5
```

Every hit ends with the exact command to open it.

## Install

macOS and Linux:

```sh
curl -fsSL https://github.com/lakshyaag/codeagentlog/releases/latest/download/codeagentlog-installer.sh | sh
```

Windows, in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/lakshyaag/codeagentlog/releases/latest/download/codeagentlog-installer.ps1 | iex"
```

Or grab a binary from [Releases](https://github.com/lakshyaag/codeagentlog/releases), or build from
source with Rust 1.85+:

```sh
cargo install --git https://github.com/lakshyaag/codeagentlog --locked
```

<details>
<summary>Or have your agent do it</summary>

Paste this into Claude Code, Codex, pi, or Cursor:

```text
Install codeagentlog (https://github.com/lakshyaag/codeagentlog), a CLI for reading past coding-agent sessions.
1. Run: curl -fsSL https://github.com/lakshyaag/codeagentlog/releases/latest/download/codeagentlog-installer.sh | sh
   On Windows instead: powershell -ExecutionPolicy Bypass -c "irm https://github.com/lakshyaag/codeagentlog/releases/latest/download/codeagentlog-installer.ps1 | iex"
   It installs to ~/.cargo/bin; use that path if the command isn't found yet.
2. Run `codeagentlog setup`. It writes nothing; it lists the skill files it would add. Show me the list and
   wait for my OK before running it with --yes.
3. Run `codeagentlog doctor` and tell me which agents' sessions it found.
```

</details>

## Quick start

```sh
codeagentlog ls                          # sessions in this folder, newest first
codeagentlog show @last --chat --tail 10 # where the last session here left off
codeagentlog grep "flaky test"           # search everything, every agent
codeagentlog tree ~/code                 # which folders have sessions, per agent
```

## Let your agents use it

This is the part I built it for. `codeagentlog setup` installs a skill file for each agent you
have — `~/.claude/skills/` for Claude Code, `~/.agents/skills/` for Codex, pi, and Cursor (Cursor
reads both) — so they reach for the tool on their own. It shows you the files first and only
writes with `--yes`. `--remove` takes them back out, and re-running it after an upgrade refreshes
the skill (`doctor` says when it's stale). For an agent without skills support,
`codeagentlog guide >> AGENTS.md` writes the same instructions into your AGENTS.md.

After that, "pick up where the last session left off" and "how did we fix this in the other repo?"
just work. When an agent runs it, output is plain text — no colors, no pager — and long output is
cut off with the exact command to see the rest. The agent's own session is marked `(current)` so
it doesn't read itself back.

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

A session can be named `agent:prefix`, any unique prefix, a file path, or `@last` /
`@last:<agent>` — the newest session under the current folder, not counting the one you're in.

### Everything else

- `--json` works on every command.
- `doctor` shows which agent dirs were found, how many sessions each has, and anything that
  failed to parse.
- On a terminal, output is colored and paged with `less -FRX`. `--color always|never`, `NO_COLOR`,
  `FORCE_COLOR`, `--no-pager`, and `$CODEAGENTLOG_PAGER` / `$PAGER` change that.

## How it behaves

- Records are numbered once, in file order. `--events`, `--tools none`, and `--branch` hide
  records but never renumber them, so a `[57]` from `grep` or `ls` always means the same record.
- Nothing is filtered by judgment. Every record is there; narrow it down yourself with `--role`,
  `--turns`, `--agent`, `--since`, `--tools`, and `--events`.
- Hidden doesn't mean unsearchable: thinking, Codex `event_msg` records, and abandoned pi branches
  are hidden by default but still searched by `grep`, and each hit says which flags reveal them.
- Subagent threads (Claude, Codex, Cursor) nest under the session that started them in `ls`
  (`--children` expands them), and `grep` always searches them.

## Where the data comes from

| Agent | Default location | Override |
|---|---|---|
| pi | `~/.pi/agent/sessions/` | `PI_CODING_AGENT_DIR` |
| Claude Code | `~/.claude/projects/` | `CLAUDE_CONFIG_DIR` |
| Codex | `~/.codex/sessions/`, `~/.codex/archived_sessions/` | `CODEX_HOME` |
| Cursor | `~/.cursor/projects/*/agent-transcripts/` | `--root cursor=PATH` |

`--root agent=PATH` points any agent at a different home dir (e.g. `--root codex=/tmp/codex`).
Agents that aren't installed are skipped.

**Cursor:** transcript files leave out tool output, thinking, and most timestamps. `show <id>
--db` reads the same conversation from Cursor's own database (`state.vscdb` in its app-data dir,
or `$CODEAGENTLOG_CURSOR_DB`). The database view has extra records, so record numbers don't match
the plain view, and `grep` only searches the transcripts.

**Privacy:** everything stays local — no network requests, no writes to agent files (Cursor's
database is opened read-only, without locking it). The only writes are the skill folder you
approve in `setup` and a metadata cache at `$XDG_CACHE_HOME/codeagentlog/meta.sqlite`, safe to
delete anytime (`--no-cache` skips it).

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
