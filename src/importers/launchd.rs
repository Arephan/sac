//! Importer `launchd` (stub: to be implemented).

use super::Importer;
use crate::ctx::Ctx;
use crate::Doc;
use anyhow::Result;

pub struct Launchd;

impl Importer for Launchd {
    fn id(&self) -> &str {
        "launchd"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["loops"]
    }
    fn description(&self) -> &str {
        "launchd user agents incl. disabled/parked/removed ones (read-only)."
    }
    fn detect(&self, _ctx: &Ctx) -> bool {
        false
    }
    fn run(&self, _ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        anyhow::bail!("launchd: not implemented")
    }
}
