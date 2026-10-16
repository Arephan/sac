//! Scaffold a new instance from `templates/` (stub: to be implemented).

use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct InitResult {
    pub dir: PathBuf,
    pub instance: String,
    pub files: Vec<String>,
}

/// Write setup.yaml, policies.yaml, overrides.yaml and lint-denylist.txt into `dir`.
/// Refuses protected directories under `home` and existing files unless `force`.
pub fn init(_dir: &Path, _name: Option<&str>, _force: bool, _home: &Path) -> Result<InitResult> {
    anyhow::bail!("init: not implemented")
}
