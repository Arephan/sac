//! Importer `claude-instructions` (stub: to be implemented).

use super::Importer;
use crate::ctx::Ctx;
use crate::Doc;
use anyhow::Result;

pub struct ClaudeInstructions;

impl Importer for ClaudeInstructions {
    fn id(&self) -> &str {
        "claude-instructions"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["instructions"]
    }
    fn description(&self) -> &str {
        "Global CLAUDE.md sections as rules; @imports as sources."
    }
    fn detect(&self, _ctx: &Ctx) -> bool {
        false
    }
    fn run(&self, _ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        anyhow::bail!("claude-instructions: not implemented")
    }
}
