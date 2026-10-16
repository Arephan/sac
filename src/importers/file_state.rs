//! Importer `file-state` (stub: to be implemented).

use super::Importer;
use crate::ctx::Ctx;
use crate::Doc;
use anyhow::Result;

pub struct FileState;

impl Importer for FileState {
    fn id(&self) -> &str {
        "file-state"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["data"]
    }
    fn description(&self) -> &str {
        "Existence check for declared state files (contents never read)."
    }
    fn detect(&self, _ctx: &Ctx) -> bool {
        false
    }
    fn run(&self, _ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        anyhow::bail!("file-state: not implemented")
    }
}
