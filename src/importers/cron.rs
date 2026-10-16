//! Importer `cron` (stub: to be implemented).

use super::Importer;
use crate::ctx::Ctx;
use crate::Doc;
use anyhow::Result;

pub struct Cron;

impl Importer for Cron {
    fn id(&self) -> &str {
        "cron"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["loops"]
    }
    fn description(&self) -> &str {
        "User crontab entries."
    }
    fn detect(&self, _ctx: &Ctx) -> bool {
        false
    }
    fn run(&self, _ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        anyhow::bail!("cron: not implemented")
    }
}
