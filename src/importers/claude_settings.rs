//! Importer `claude-settings` (stub: to be implemented).

use super::Importer;
use crate::ctx::Ctx;
use crate::Doc;
use anyhow::Result;

pub struct ClaudeSettings;

impl Importer for ClaudeSettings {
    fn id(&self) -> &str {
        "claude-settings"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["brain", "tools"]
    }
    fn description(&self) -> &str {
        "Claude Code settings.json + settings.local.json (permissions, hooks, plugins, model)."
    }
    fn detect(&self, _ctx: &Ctx) -> bool {
        false
    }
    fn run(&self, _ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        anyhow::bail!("claude-settings: not implemented")
    }
}
