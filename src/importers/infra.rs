//! Importer `infra` (stub: to be implemented).

use super::Importer;
use crate::ctx::Ctx;
use crate::Doc;
use anyhow::Result;

pub struct Infra;

impl Importer for Infra {
    fn id(&self) -> &str {
        "infra"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["infra"]
    }
    fn description(&self) -> &str {
        "OS, installed CLIs, git repos under the home dir."
    }
    fn detect(&self, _ctx: &Ctx) -> bool {
        false
    }
    fn run(&self, _ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        anyhow::bail!("infra: not implemented")
    }
}
