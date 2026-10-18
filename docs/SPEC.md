# codeagentlog — SPEC v0

A small, read-only Rust CLI that lets **any coding agent (or human)** find and read past
conversations from **pi, Codex, Claude Code, and Cursor**, organized by folder and agent.

## Motivation

Across ~40 sessions I've had to manually prompt agents with things like:

- "see previous conversations. where are we"
- "can you see the previous conversation i was having yesterday. what plan did we agree on"
- "see the other pi sessions in this folder. hone in on the session where we talk about …benchmark-variant…"
- "can you find the conversations i had with claude code for this folder?"
- "see @~/.pi/agent/sessions/--Users-…-eval-benchmarks--/ for the other session …"

Every time, the agent re-discovers storage layouts and writes throwaway `jq`/`rg` parsers over
multi-MB JSONL files full of tool output. An earlier pi-only `/attach` extension was abandoned
because it only worked inside pi.

## Principles

1. **Present, don't judge.** No noise filtering, no summarization, no heuristics about what
   "matters". Every record in a session is rendered. The *caller* narrows (`--role`, `--turns`,
   `--agent`, `--since`); the tool never narrows on its own.
2. **Formatting only.** The tool's job is to turn each agent's on-disk format into one readable
   transcript and one listing format, so agents stop writing parsers.
3. **Read-only.** Never writes into agent directories. Only writes its own cache, plus its own
   skill folder when the user approves `setup`.
4. **Agent-first UX.** Compact text output by default, `--json` everywhere, and every truncated
   output ends with the exact command to continue.
5. **Not boxed into cwd.** Folder tree and cross-folder listing are first-class.
6. **Survive format drift.** Lenient parsing; unknown records are rendered generically and
   counted, never fatal.
7. **Drop-in for anyone.** Zero config. Standard per-agent locations, overridable via each agent's
   own env var (`PI_CODING_AGENT_DIR`, `CLAUDE_CONFIG_DIR`, `CODEX_HOME`) or `--root agent=PATH`.
   Missing agents are skipped silently. Nothing tuned to one user's data; the counts in this spec
   are observations from one machine, not assumptions.

Scope: **pi, Codex, Claude Code, Cursor**. OpenCode / Gemini / others later.

---

## Commands

```
codeagentlog tree   [PATH] [--depth N] [--agent pi|claude|codex|cursor] [--since DUR]
codeagentlog ls     [PATH] [--here|--under|--near|--all] [--agent X] [--since DUR]
                    [--prompts N] [--recent N] [--group agent|folder|none] [--children] [--limit N]
codeagentlog show   <ID|PATH|@last[:agent]> [--turns A..B|N~K | --tail N] [--role R,..] [--chat]
                    [--tools brief|full|none] [--thinking] [--events]
                    [--branch ENTRY_ID | --all-branches] [--db] [--max-chars N]   # 0 = unlimited
codeagentlog grep   <PATTERN> [PATH] [scope flags] [--agent X] [--role user|assistant|tool|event]
                    [-i] [--regex] [--events] [--context N] [--limit N]
codeagentlog path   <ID>
codeagentlog doctor
codeagentlog setup  [--yes] [--remove]
codeagentlog guide
```

All commands accept `--json`, `--color auto|always|never`, `--no-pager`.

- `@last[:agent]`: newest root session under cwd, excluding the caller's own.
- `--turns N~K`: records N-K..N+K. `--tail N` counts records in the current view.
- `show --chat`: user/assistant records with text, text blocks only (caller-chosen view).
- Color/pager only on a TTY (`NO_COLOR`, `FORCE_COLOR`, `TERM=dumb` honored); piped text is identical
  to colored text minus escapes.

### Defaults

| Thing | Default |
|---|---|
| `tree` root | `~` |
| `tree` grouping | folder hierarchy, per-agent counts as columns |
| `ls` scope | `--under` (PATH, default cwd, and all descendants) |
| `ls` grouping | agent → folder (`--group` to change) |
| `ls` sort | most recently updated first |
| `ls` limit | 20 sessions total; footer gives the exact command for more |
| `grep` scope | `--under` cwd (same as `ls`) |
| `--since` | filters on **last updated** (`30m`, `2h`, `3d`, `2w`, or an ISO date) |
| times | local time zone |
| `show` tools | `brief` |
| `show` thinking | hidden (`--thinking` to show) |
| `show` pi branches | active path only, footer lists other branches |
| `show` Codex | `response_item` stream; `--events` adds `event_msg` stream |
| `show --max-chars` | 20000 |
| child sessions | nested/collapsed under parent in `ls`; always searchable by `grep` |

### Scope flags

- `--here` — sessions whose cwd == PATH
- `--under` — cwd == PATH or descendant (default)
- `--near` — `--under` plus ancestors of PATH (e.g. `sft-eval` also sees `post-training`)
- `--all` — everything

Path canonicalization: resolve symlinks, strip `/System/Volumes/Data` prefix, strip trailing `/`.
If the path no longer exists, normalize lexically (no symlink resolution) — never drop the session.

Which cwd a session belongs to:
- **Display cwd** = the session's starting cwd.
- **Scope match** = true if *any* cwd seen during the session matches (Claude records cwd per line,
  Codex per `turn_context`). The cache stores the full set.
- **Child sessions** are scoped by their parent's cwd and always appear nested under the parent.
  `grep` still searches them everywhere.

### IDs

- Namespaced, **always displayed as the full native ID**:
  `pi:01a11a9a-5f59-71aa-82e2-71538deca7f8`, `claude:d3d1cb7b-…`, `codex:019ff0bf-…`.
  (pi/Codex use UUIDv7, whose leading characters are just a timestamp, so short prefixes collide
  between a parent and the children it spawns seconds later.)
- On input, any unique prefix within a namespace resolves (the namespace is optional if the prefix
  is unique across all agents). Ambiguous prefix → list candidates, exit non-zero.
- A raw file path is also accepted anywhere an ID is.
- Child sessions get their own IDs (`codex:<thread id>`, `claude:<session>/<agentId>`,
  `cursor:<session>/<childId>`, `pi:<id>`).

### Current session marker

Listings mark the caller's own session `(current)`; it is **not** hidden.
Detection order:
1. pi: `PI_SESSION_ID` / `PI_SESSION_FILE` (confirmed present).
2. Claude Code: `CLAUDE_CODE_SESSION_ID`; Codex: `CODEX_THREAD_ID`; Cursor: `CURSOR_CONVERSATION_ID`
   (all found in the binaries).
3. Fallback, only when an agent marker env var is set (`PI_CODING_AGENT`, `CLAUDECODE`,
   `CODEX_SANDBOX`/`CODEX_CI`, `CURSOR_AGENT`): that agent's newest session in cwd updated < 60s ago.

---

## Output formats

### `tree`

```
~/Projects                                         pi  claude  codex   last
├─ post-training                                    3       0      2   2d
│  ├─ sft-eval                                     12       4      6   now
│  └─ teacher-distillation                         18       9      3   1d
├─ eval-benchmarks                                 41       3     12   4d
└─ gtm-agent-v2                                     9       0      0   3w
```
Counts are root sessions; child count shown as `+N` suffix if any (e.g. `6+31`).

### `ls`

```
pi
  ~/Projects/post-training/sft-eval
    pi:01a11a9a-5f59-71aa-82e2-71538deca7f8  2026-10-08 08:21 → 09:40  12 prompts  412 records  (current)
      "i have had multiple cases where i have had to manually prompt…"   (first-user)
      forked from pi:01a1166d-d52c-7776-bf84-bc8a7de1ecd7
claude
  ~/Projects/post-training/sft-eval
    claude:d3d1cb7b-…  2026-10-07 12:45 → 14:02  9 prompts  188 records  main
      "Dataset and k8s sync flow"                                         (ai-title)
codex
  ~/Projects/post-training/sft-eval
    codex:019ff0bf-…  2026-10-05 10:02 → 11:15  7 prompts  264 records  +12 children (guardian×10, subagent×2)
      "Reward design for simpleqa grader"                                 (thread_name)

-- showing 3 of 57. more: codeagentlog ls --limit 40
```

- One header line + one title line per session (+ optional link line). Title source shown in parens.
- `prompts` = count of user-role records, verbatim (includes injected context such as Codex
  `<environment_context>` or Claude `<task-notification>` — the tool doesn't judge).
  `records` = numbered records (see `show`).
- `--prompts N` appends the first N user messages (verbatim, each truncated to ~200 chars).
- Sessions with zero user messages are listed (labelled `0 user`).
- `--children` expands child sessions inline, indented under the parent.

### `show`

```
# claude:d3d1cb7b-5e0c-4c8a-9a51-0f2b7d3c1e44 · ~/Projects/post-training/sft-eval · main
# 2026-10-07 12:45 → 14:02 · 9 prompts · 88 records · model claude-sonnet-… (latest; changes listed inline)
# file ~/.claude/projects/-Users-…-sft-eval/d3d1cb7b-….jsonl
# children: claude:d3d1cb7b-…/a7d1314a… (general-purpose: "Verify Weak RL papers batch 1")

## [1] user · 12:45:25
can you see the previous conversation i had with you where we built the dataset and k8 sync flow

## [2] assistant · 12:45:31
I'll look for it.
  → Bash {"command":"ls -t ~/.claude/projects/…"}

## [3] tool · Bash · ok
  ← 14 lines, 1.2k chars
    d6efd97b-….jsonl
    ca59d1eb-….jsonl
    …
    (last) f6a2cecf-….jsonl

## [4] user · 12:47:02
<task-notification>…rendered verbatim…</task-notification>

## [5] event · 12:50:10
· system/informational: agents-md: no CLAUDE.md found; AGENTS.md loaded: …

-- truncated at 20000 chars (record 31/88). next: codeagentlog show claude:d3d1cb7b-5e0c-4c8a-9a51-0f2b7d3c1e44 --turns 31..
```

Rules:
- **Numbering:** every record in the file gets a number once, in file order, across all streams
  (Codex `event_msg` included) and all branches (pi). Claude assistant lines sharing a `message.id`
  are one record. Flags (`--events`, `--tools none`, `--branch`, `--thinking`) only hide/show records
  and **never renumber** — gaps are normal. So `[N]` from `grep`, `ls`, or a footer always means
  the same record.
- **Oversized record:** if one record alone exceeds `--max-chars`, it's cut in place:
  `… 48213 chars omitted; full: codeagentlog show <id> --turns 31 --tools full --max-chars 0`.
  The range footer then continues from the next record, so pagination never loops.
- Roles: `user`, `assistant`, `tool`, `event`, `system`, plus agent-specific passthroughs
  (`custom:<type>` for pi custom messages, `developer` for Codex, `bash` for pi `!` commands).
- `--tools brief`:
  - tool call → one line: `→ <name> <args json>` truncated to ~200 chars.
  - tool result → `← <N lines, M chars>` + first ~200 chars + `(last) <final line>` (errors/exit codes
    tend to be at the end). `error` instead of `ok` when the record says so.
- `--tools full`: complete args and results. `--tools none`: omit tool turns entirely
  (turn numbers unchanged — gaps are fine).
- Events (model changes, compaction, titles, settings, etc.) render as one `·` line in brief mode,
  full JSON-ish payload in full mode.
- Images → `[image <mime> <bytes>]` placeholder; never inline base64.
- Thinking → `[thinking, N chars]` unless `--thinking`.
- Footer always present when truncated, with the exact next command.
- pi with branches: footer `3 other branches: --branch <entryId> … | --all-branches`.

### `grep`

```
pi:01a0f3c2-…  ~/Projects/eval-benchmarks  2026-08-28  6 prompts  141 records
  [12] user: …hone in on the session where we talk about getting a benchmark-variant service…
  [15] assistant: …the benchmark-variant service would mirror search-eval-variant…
codex:01a1062c  …
```
- Grouped by session, most recent first; each hit shows turn number + role + snippet.
- Matches **rendered text** of turns (all roles, including tool calls/results and events),
  never raw JSON keys or base64. `--role` narrows if the caller asks.
- Searched by default: thinking, full tool args/results (not the brief rendering), all pi branches,
  child sessions (labelled `child of <parent id>`). Not by default: Codex `event_msg` stream
  (duplicates response items) — `--events` adds it.
- A hit in content `show` hides by default carries the flags needed to see it,
  e.g. `[57 thinking] … → show <id> --turns 57 --thinking`, `[80, branch 3f2a] … → --branch 3f2a`.
- Raw-byte prefilter is skipped (full parse instead) when the pattern can't be matched against JSON
  bytes: contains `"`, `\`, control chars, or non-ASCII; `-i` with non-ASCII; regex with anchors.

### `doctor`

Per agent: root dir, file count, session count, child count, parse errors, unknown record
types with counts, newest/oldest session. This is how format drift gets noticed.

### `setup`

Installs the agent skill, `skills/codeagentlog/SKILL.md` (embedded in the binary), as
`<claude home>/skills/codeagentlog/SKILL.md` (Claude Code, Cursor) and
`~/.agents/skills/codeagentlog/SKILL.md` (Codex, pi, Cursor), for agents whose home dir exists.
Each target is `missing`, `outdated` (content differs from this binary's skill) or `current`.

Writes only with explicit approval: `--yes`, or `y` at a prompt when stdin and stderr are a
terminal. Otherwise (an agent running it, `--json`) it prints the plan, writes nothing, and tells
the agent to ask the user before `--yes`. Re-running is safe; `--remove` deletes only the files it
installed (same approval rule). `doctor` reports each target's state.

Release rule: the skill ships inside the binary, so it changes only with a release, and `setup`
after an upgrade refreshes it.

### `guide`

Prints the skill's body (no frontmatter): the same instructions, for agents without skills support
to paste into `AGENTS.md` / `CLAUDE.md`.

---

## Data model

```rust
enum Agent { Pi, Claude, Codex }

struct SessionMeta {
    agent: Agent,
    id: String,                  // native id; display as "<agent>:<short>"
    path: PathBuf,
    cwd: PathBuf,                // starting cwd, canonicalized (display)
    cwds: Vec<PathBuf>,          // all cwds seen (scope matching)
    git_branch: Option<String>,
    title: Option<(String, TitleSource)>,
    started: Timestamp,
    updated: Timestamp,
    record_count: usize,
    prompt_count: usize,         // user-role records, verbatim
    parent: Option<SessionRef>,  // for child sessions
    forked_from: Option<SessionRef>, // pi `parentSession`, Claude `continued-in` (inverse) — still a root
    child_kind: Option<String>,  // "guardian", "subagent", "general-purpose", …
}

struct Turn { idx: usize, role: Role, ts: Option<Timestamp>, blocks: Vec<Block>, raw_type: String }

enum Block {
    Text(String),
    Thinking(String),
    ToolCall { name: String, args: serde_json::Value },
    ToolResult { name: Option<String>, ok: Option<bool>, text: String },
    Image { mime: Option<String>, bytes: usize },
    Event { kind: String, payload: serde_json::Value },
}

trait Adapter {
    fn discover(&self) -> Vec<SourceRef>;                 // stat only
    fn meta(&self, src: &SourceRef) -> Result<SessionMeta>; // header / light scan
    fn load(&self, src: &SourceRef, opts: &LoadOpts) -> Result<(SessionMeta, Vec<Turn>)>;
}
```

Parsing is done into `serde_json::Value` with field fallbacks (not strict structs). Unknown
record type → `Turn { role: Event, raw_type, blocks: [Event{..}] }` + counted for `doctor`.
Truncated final line (live session) → skipped silently.

---

## Adapter notes (verified against my data, 2026-10-08)

### pi — `~/.pi/agent/sessions/<--encoded-cwd-->/<ts>_<uuid>.jsonl`  (438 sessions, 633M)

Record types observed (counts across all files):

| type | count | render as |
|---|---|---|
| `message/toolResult` | 33129 | `tool` (`toolName`, `isError`, `content`) |
| `message/assistant` | 28918 | `assistant`; blocks `text`, `thinking`, `toolCall`, `image` |
| `message/user` | 3924 | `user`; content is blocks or plain string (62 cases) |
| `thinking_level_change` | 958 | event |
| `model_change` | 723 | event (`provider`, `modelId`) |
| `custom` | 664 | event (`customType`, `data`) — extension state, not in LLM context |
| `session` | 438 | header: `id`, `cwd`, `timestamp`, `version` |
| `message/system` | 48 | `system` |
| `compaction` | 43 | event; `summary` shown (brief: first ~200 chars) |
| `custom_message` | 37 | `custom:<customType>` (`content`, `display`) |
| `message/bashExecution` | 14 | `bash` (`command`, `output`, `exitCode`) |
| `context_edit` | 4 | event |
| `usage` | 1 | event |
| `session_info` | 1+ | event; `name` → title |

- Every entry has `id` + `parentId` → **tree**. Active path = walk `parentId` from the last entry.
  Other leaves = branches.
- Header may carry `parentSession` (path to another session file; seen on ~5% of headers, likely
  forks) → `forked_from`; shown in `ls`/`show` headers, session stays a root.
- Title: latest `session_info.name`, else first user message.
- Child sessions: pi subagents write `session_info.name` like `subagent-scout-<uuid>-1`,
  `subagent-reviewer-…`, `subagent-context-builder-…`. **TODO:** figure out how to link to the parent
  session (uuid in name? session dir? subagent extension metadata?). Until linked, treat as roots
  with `child_kind` parsed from the name.
- Current session: `PI_SESSION_ID`, `PI_SESSION_FILE`.

### Claude Code — `~/.claude/projects/<-encoded-cwd>/<sessionId>.jsonl`  (79 files, 107M)

| type | count | render as |
|---|---|---|
| `assistant` | 6463 | `assistant`; **multiple lines per `message.id`** (one per block) → merge consecutive lines with same `message.id` into one turn |
| `attachment` | 4635 | event |
| `user` | 3423 | `user` if content is string/text blocks; `tool_result` blocks → `tool` turn (`tool_use_id`, `is_error`) |
| `last-prompt` | 1166 | event |
| `mode`, `permission-mode`, `atis-latch` | ~3400 | event |
| `ai-title` | 1120 | event; latest `aiTitle` → title |
| `system` | 573 | `system` (`subtype`, `content`) |
| `file-history-snapshot` / `-delta` | ~650 | event |
| `queue-operation` | 333 | event |
| `agent-name`, `cost-state`, `pr-link`, `continued-in`, `agent-setting` | small | event |

- Content block types: `text`, `thinking`, `tool_use`, `tool_result`, `image`, or a plain string.
- Per-line fields: `cwd`, `sessionId`, `gitBranch`, `uuid`, `parentUuid`, `timestamp`, `isSidechain`,
  `version`, `entrypoint`.
- Tool name for a `tool_result` comes from the matching `tool_use.id` earlier in the file.
- **Children:** `<sessionId>/subagents/agent-<agentId>.jsonl` + `agent-<agentId>.meta.json`
  (`agentType`, `description`, `toolUseId`, `spawnDepth`, …); also nested
  `<sessionId>/subagents/workflows/<wf_id>/agent-*.jsonl`. Lines have `isSidechain: true`, `agentId`.
  Link: parent = `sessionId`; spawn point = `toolUseId` (show in parent transcript as
  `→ child claude:<sid>/<agentId>`).
- `<project>/memory/` dirs exist — not sessions, ignore (maybe surface later).
- Title: latest `ai-title.aiTitle`, else first user message.
- Continuation: `continued-in` record links to the next session — show in header/footer.

### Codex — `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl` + `~/.codex/archived_sessions/`  (473 files, ~900M)

| type/payload.type | count | render as |
|---|---|---|
| `event_msg/item_completed` | 28963 | `--events` only (items: AgentMessage, UserMessage, FileChange, McpToolCall, WebSearch, Plan, ContextCompaction, Reasoning) |
| `event_msg/token_count` | 18313 | `--events` only |
| `response_item/reasoning` | 14900 | thinking (often encrypted/empty → `[reasoning, encrypted]`) |
| `response_item/function_call` + `_output` | ~22000 | tool call / tool result |
| `response_item/custom_tool_call` + `_output` | ~15000 | tool call / tool result |
| `response_item/message/assistant` | 7749 | `assistant` |
| `token_usage_record` | 5925 | event |
| `response_item/message/user` | 4087 | `user` (verbatim — includes injected AGENTS.md / `<environment_context>` blocks) |
| `turn_context` | 3351 | event |
| `event_msg/task_started`, `task_complete`, `turn_aborted`, `thread_*` | ~5500 | `--events` only |
| `response_item/message/developer` | 1145 | `developer` |
| `world_state` | 512 | event |
| `session_meta` | 501 | header |
| `response_item/web_search_call` | 332 | tool call |
| `response_item/agent_message`, `inter_agent_communication_metadata` | 291 each | `assistant` (agent msg) / event |
| `compacted`, `response_item/compaction` | 148 | event |
| `response_item/tool_search_call` + `_output` | 72 | tool call / result |
| `realtime_item/*` | 3 | event |

- **No `event_msg/user_message` in this Codex version** (older docs suggest it exists — handle both).
- `session_meta.payload` keys: `id`, `session_id`, `cwd`, `timestamp`, `originator`, `source`,
  `thread_source`, `parent_thread_id`, `cli_version`, `model_provider`, `base_instructions`,
  `runtime_workspace_roots`, …
- Default transcript = `response_item` + non-`event_msg` records (session_meta, turn_context, compacted,
  world_state, token_usage_record as events). `--events` adds the `event_msg` stream (it largely
  duplicates response items).
- **Title:** `~/.codex/session_index.jsonl` (`{id, thread_name, updated_at}`, 205 entries) → else first
  `item_completed` with `item.type == UserMessage` → else first `response_item` user message.
  (Verify that UserMessage holds the typed prompt for root sessions.)
- **Children are ~half of all files.** Observed `(originator | source | thread_source | parent?)`:
  - `{"subagent":{"other":"guardian"}}` / `guardian_review` — approval reviewer threads (~210)
  - `{"subagent":{"thread_spawn":{"parent_thread_id":…}}}` — spawned subagents (~64)
  - `{"subagent":"review"}` — review mode
  - roots: `vscode`, `cli`, `exec`
  Link via `payload.parent_thread_id` or `source.subagent.thread_spawn.parent_thread_id`. Some guardian
  threads have no parent id → treat as roots with `child_kind = guardian` (still shown, never hidden).
- cwd may be outside projects (Codex Desktop scratch dirs like `~/Documents/Codex/2026-10-04/see-x20`).
  `tree` must show these.
- Other Codex state (`history.jsonl`, `memories_1.sqlite`, `goals_1.sqlite`, `logs_2.sqlite`) — out of scope v0.

### Cursor — `~/.cursor/projects/<slug>/agent-transcripts/<id>/<id>.jsonl` + `<id>/subagents/<childId>.jsonl`  (1145 files)

| record | count | render as |
|---|---|---|
| `{role: assistant, message.content}` | 37716 | `assistant` (`text`, `tool_use` blocks; one record per line) |
| `{role: user, message.content}` | 5800 | `user` |
| `{type: turn_ended, status, error?}` | 3564 | event (`success` / `error: …` / `aborted`) |

- **The transcript is thin:** no tool results, tool-call ids, model, thinking, title or cwd, and
  timestamps only inside prompts: `<timestamp>Monday, Jul 13, 2026, 2:21 PM (UTC-4)</timestamp>`
  (~85% of prompts). That line becomes the record time and the `<user_query>` wrapper is dropped;
  other injected tags (`<attached_files>`, …) stay verbatim. `updated` = max(last prompt, file mtime).
- **cwd:** `<slug>` is the workspace path with every non-alphanumeric char replaced by `-` (lossy).
  Recovered, in order: an absolute path in tool arguments with an ancestor that slugifies to `<slug>`
  (~77%); a filesystem walk matching the slug (longest name first); Shell `working_directory` values.
  Multi-root workspaces (`…-Cursor-Workspaces-<n>-workspace-json`) usually stay unknown.
- **Children:** `subagents/<childId>.jsonl`, id `cursor:<parent>/<childId>`, kind `subagent`. No
  link back to the spawning `Task`/`Subagent` call (no tool ids).
- **`show --db`:** Cursor's IDE database (`~/Library/Application Support/Cursor/User/globalStorage/state.vscdb`
  on macOS, `~/.config/Cursor/…` on Linux, `%APPDATA%/Cursor/…` on Windows; override with
  `CODEAGENTLOG_CURSOR_DB`) keys the same conversations by the same id (98% overlap observed):
  `composerData:<id>` (title `name`, `modelConfig`, ordered `fullConversationHeadersOnly`) and
  `bubbleId:<id>:<bubbleId>` (type 1 user / 2 assistant, `createdAt`, `text`, `thinking.text`,
  `toolFormerData {name, params, result, status, toolCallId}`). `--db` renders the session from there
  instead: per-message timestamps, thinking, and each tool call followed by a `tool` record with its
  result. Records are numbered in database order, so `[N]` differs from the transcript view; footers
  carry `--db`. Content-free placeholder bubbles are `--events` only. Opened read-only with
  `immutable=1` (never locks or writes Cursor's file). Listing, `grep` and the cache use the transcript only.

---

## Performance

- No search index in v0.
- **Metadata cache:** `$XDG_CACHE_HOME/codeagentlog/meta.sqlite` (default `~/.cache/codeagentlog`,
  on macOS too), key `(path, mtime, size)` → `SessionMeta`. Safe to delete anytime.
  `tree`/`ls` = walk + stat + re-read only changed files.
- **grep:** stage 1 = parallel raw-byte prefilter (`memchr::memmem`, case-folded for `-i`; `regex`
  bytes for `--regex`) over all in-scope files with `rayon`. Stage 2 = full parse only of hit files,
  match against rendered turn text.
- Target: `ls`/`tree` < 100ms warm; `grep` over ~1.6GB < 2s cold-ish on a laptop.

## Crates

`clap` (derive), `serde`, `serde_json`, `rusqlite` (bundled), `rayon`, `walkdir`, `memchr`, `regex`,
`jiff`, `directories`, `anyhow`. Nothing else without a reason.

## Layout

```
codeagentlog/
  Cargo.toml
  docs/SPEC.md                this design
  src/main.rs                 clap wiring
  src/model.rs                Agent, SessionMeta, Turn, Block, Role, SessionRef
  src/scope.rs                canonicalize, here/under/near/all
  src/ids.rs                  namespaced id parse/resolve/shorten
  src/cache.rs                meta.sqlite
  src/render/{text,json}.rs   tree/ls/show/grep renderers, truncation + footers
  src/cmd/{tree,ls,show,grep,path,doctor,setup,guide}.rs
  skills/codeagentlog/SKILL.md the agent skill `setup` installs and `guide` prints
  src/adapters/{mod,pi,claude,codex,cursor,cursor_db}.rs
  tests/fixtures/{pi,claude,codex,cursor}/*.jsonl   small synthetic samples
  tests/*.rs                  golden-output tests per adapter + per command
```

## Milestones

Status: v0.1–v0.4 implemented (all commands, cache, child linking, `--json`, golden tests).

1. **v0.1** — adapters (pi, claude, codex) + `ls` + `show` + `path`. No cache. Golden tests from fixtures.
2. **v0.2** — `tree`, scope flags, child linking/nesting, metadata cache, `doctor`.
3. **v0.3** — `grep`, `guide`, `(current)` marker, `--json` on all commands.
4. **v0.4** — Cursor: agent-transcripts adapter + `show --db` from the IDE `state.vscdb`.
5. **later** — cursor-agent `store.db`, OpenCode (`opencode.db`), Gemini, Amp, Factory; Homebrew tap, crates.io.

## Open TODOs

- pi subagent → parent linking (see pi notes).
- Verify `CLAUDE_CODE_SESSION_ID` / `CODEX_THREAD_ID` are exported to tool shells in practice.
- Confirm Codex `item_completed/UserMessage` = typed prompt for root sessions.
- Decide whether `tree` counts should include children by default (current plan: `roots+children`).
- Claude `parentUuid` branches (rewind/edit): v0 renders file order linearly; revisit if confusing.
- Confirm what pi writes `parentSession` for (fork vs other).
- Cursor: link subagents to their spawning call.
