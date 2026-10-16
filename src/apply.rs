//! Render config files from the desired state into an output root (stub: to be implemented).

use crate::Doc;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct ApplyOut {
    pub out_dir: PathBuf,
    pub written: Vec<String>,
    pub n_plist: usize,
    pub lock: Doc,
}

/// Create the out root and refuse one that resolves into live config. Returns the real path.
pub fn assert_safe_out_root(out_root: &Path, _home: &Path) -> Result<PathBuf> {
    Ok(out_root.to_path_buf())
}

/// Write claude/settings.json, claude/mcp-servers.json, claude/RULES.md, launchd plists,
/// setup.lock.json, setup.resolved.yaml and report.md under `<out_root>/<instance>/`.
pub fn render(
    _desired: &Doc,
    _out_root: &Path,
    _home: &Path,
    _policy_text: Option<&str>,
    _plan_text: Option<&str>,
) -> Result<ApplyOut> {
    anyhow::bail!("apply: not implemented")
}

pub fn claude_settings_doc(_desired: &Doc) -> Doc {
    serde_json::json!({})
}

pub fn mcp_servers_doc(_desired: &Doc) -> Doc {
    serde_json::json!({ "mcpServers": {} })
}

pub fn rules_md(_desired: &Doc) -> String {
    String::new()
}

pub fn plist_xml(_loop: &Doc, _home: &Path) -> String {
    String::new()
}
