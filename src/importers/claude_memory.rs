//! Importer `claude-memory` (stub: to be implemented).

use super::Importer;
use crate::ctx::Ctx;
use crate::Doc;
use anyhow::Result;

pub struct ClaudeMemory;

impl Importer for ClaudeMemory {
    fn id(&self) -> &str {
        "claude-memory"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["instructions", "projects", "data", "identity"]
    }
    fn description(&self) -> &str {
        "Auto-memory files: feedback=rules, project=projects, reference=references."
    }
    fn detect(&self, _ctx: &Ctx) -> bool {
        false
    }
    fn run(&self, _ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        anyhow::bail!("claude-memory: not implemented")
    }
}
