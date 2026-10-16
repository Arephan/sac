//! `apply --live`: merge the desired state into a real home with backups (stub: to be implemented).

use crate::Doc;
use anyhow::Result;
use std::path::PathBuf;

pub struct LiveOptions {
    pub home: PathBuf,
    pub yes: bool,
    pub force: bool,
}

pub struct LiveReport {
    pub backup_dir: Option<PathBuf>,
    pub changed: Vec<String>,
    pub skipped: Vec<String>,
    pub commands: Vec<String>,
}

pub fn apply_live(_desired: &Doc, _opts: &LiveOptions) -> Result<LiveReport> {
    anyhow::bail!("apply --live: not implemented")
}
