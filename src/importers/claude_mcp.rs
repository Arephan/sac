//! Importer `claude-mcp` (stub: to be implemented).

use super::Importer;
use crate::ctx::Ctx;
use crate::Doc;
use anyhow::Result;

pub struct ClaudeMcp;

impl Importer for ClaudeMcp {
    fn id(&self) -> &str {
        "claude-mcp"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["tools"]
    }
    fn description(&self) -> &str {
        "MCP server definitions (user, per-project, mcp.json). Env/header values never kept."
    }
    fn detect(&self, _ctx: &Ctx) -> bool {
        false
    }
    fn run(&self, _ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        anyhow::bail!("claude-mcp: not implemented")
    }
}
