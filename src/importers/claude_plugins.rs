//! Importer `claude-plugins` (stub: to be implemented).

use super::Importer;
use crate::ctx::Ctx;
use crate::Doc;
use anyhow::Result;

pub struct ClaudePlugins;

impl Importer for ClaudePlugins {
    fn id(&self) -> &str {
        "claude-plugins"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["tools"]
    }
    fn description(&self) -> &str {
        "Installed plugins and marketplaces under ~/.claude/plugins."
    }
    fn detect(&self, _ctx: &Ctx) -> bool {
        false
    }
    fn run(&self, _ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        anyhow::bail!("claude-plugins: not implemented")
    }
}
