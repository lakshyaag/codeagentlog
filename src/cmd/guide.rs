use anyhow::Result;

use super::{Ctx, to_json};

const GUIDE: &str = "\
When I refer to a previous or other conversation/session (pi, Claude Code, or Codex), use `codeagentlog`:
`codeagentlog ls` (sessions for this folder; add --near or --all to widen), `codeagentlog tree` (find folders),
`codeagentlog grep <keyword>` (search transcripts), then `codeagentlog show <id> [--turns A..B | --tail N]`.
Truncated output ends with the exact command to continue. Don't parse session files directly.
";

pub fn run(ctx: &Ctx) -> Result<String> {
    if ctx.json {
        return to_json(&serde_json::json!({ "guide": GUIDE }));
    }
    Ok(GUIDE.to_string())
}
