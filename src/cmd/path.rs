use anyhow::Result;

use super::{Ctx, to_json};

pub fn run(ctx: &Ctx, id: &str) -> Result<String> {
    let catalog = ctx.catalog(&[]);
    let m = &catalog.entries[ctx.resolve(&catalog, id)?].meta;
    if ctx.json {
        return to_json(&serde_json::json!({ "id": m.display_id(), "path": m.path }));
    }
    Ok(format!("{}\n", m.path.display()))
}
