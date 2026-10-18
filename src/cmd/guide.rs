use anyhow::Result;

use super::{Ctx, to_json};

const GUIDE: &str = "\
## Past conversations
When I refer to a previous/other conversation or session (pi, Claude Code, Codex, or Cursor), use `codeagentlog`
instead of reading session files directly:
- `codeagentlog ls --recent 2`: sessions for this folder, newest first, with their last prompts
  (`--near` adds parent folders, `--all` everything, `--agent claude` one agent, `--since 2d` recent ones).
- `codeagentlog show @last --chat --tail 10`: where the previous session here left off.
- `codeagentlog grep <keyword>`: find the session and record; each hit prints the `show` command to open it.
- `codeagentlog show <id> --turns 57~5`: records 52..62; add `--tools full`, `--thinking`, `--events` for more.
- `codeagentlog tree`: which folders have sessions.
- Cursor transcripts lack tool results and thinking: add `--db` to `show` to read them from Cursor's database.
Ids look like `pi:01a1…`, `claude:d3d1…`, `codex:019f…`, `cursor:6089…`; any unique prefix works. Truncated
output ends with the exact command to continue.
";

pub fn run(ctx: &Ctx) -> Result<String> {
    if ctx.json {
        return to_json(&serde_json::json!({ "guide": GUIDE }));
    }
    Ok(GUIDE.to_string())
}
