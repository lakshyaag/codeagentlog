use anyhow::Result;

use super::{Ctx, to_json};

/// The agent skill `setup` installs, shipped in the repo at `skills/codeagentlog/SKILL.md`.
pub const SKILL: &str = include_str!("../../skills/codeagentlog/SKILL.md");

/// The skill without its frontmatter: the same instructions, for AGENTS.md / CLAUDE.md.
pub fn guide() -> &'static str {
    SKILL.strip_prefix("---\n").and_then(|s| s.split_once("\n---\n")).map_or(SKILL, |(_, body)| body).trim_start()
}

pub fn run(ctx: &Ctx) -> Result<String> {
    if ctx.json {
        return to_json(&serde_json::json!({ "guide": guide() }));
    }
    Ok(guide().to_string())
}
