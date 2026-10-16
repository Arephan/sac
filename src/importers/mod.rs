//! Importers discover one part of a setup from the machine and return a fragment shaped like
//! `setup.yaml`. Built-ins live in this module; executables in `<instance>/importers/` are
//! loaded through `external`.

use crate::ctx::Ctx;
use crate::Doc;
use anyhow::Result;
use std::path::Path;

pub mod claude_instructions;
pub mod claude_mcp;
pub mod claude_memory;
pub mod claude_plugins;
pub mod claude_settings;
pub mod claude_skills;
pub mod cron;
pub mod external;
pub mod file_state;
pub mod infra;
pub mod launchd;
pub mod rules;

pub trait Importer {
    /// Kebab-case, unique.
    fn id(&self) -> &str;
    /// Layers the fragment touches.
    fn layers(&self) -> Vec<&str>;
    fn description(&self) -> &str;
    /// False means the importer does not apply to this machine and is skipped.
    fn detect(&self, ctx: &Ctx) -> bool;
    /// Read the machine through `ctx` only. `opts` is `import.options.<id>` (plus `declared`
    /// for `file-state`). May include a `_sources` key, which the orchestrator strips.
    fn run(&self, ctx: &Ctx, opts: &Doc) -> Result<Doc>;
}

/// The ten built-in importers, in run order.
pub fn builtin() -> Vec<Box<dyn Importer>> {
    vec![
        Box::new(claude_settings::ClaudeSettings),
        Box::new(claude_mcp::ClaudeMcp),
        Box::new(claude_plugins::ClaudePlugins),
        Box::new(claude_skills::ClaudeSkills),
        Box::new(claude_instructions::ClaudeInstructions),
        Box::new(claude_memory::ClaudeMemory),
        Box::new(launchd::Launchd),
        Box::new(cron::Cron),
        Box::new(infra::Infra),
        Box::new(file_state::FileState),
    ]
}

/// Built-ins plus every executable in `<instance>/importers/` (sorted by name). An external
/// importer whose id equals a built-in replaces it.
pub fn load_importers(instance_dir: Option<&Path>) -> Result<Vec<Box<dyn Importer>>> {
    let mut list = builtin();
    if let Some(dir) = instance_dir.map(|d| d.join("importers")) {
        if dir.is_dir() {
            for imp in external::load_dir(&dir)? {
                match list.iter().position(|x| x.id() == imp.id()) {
                    Some(i) => list[i] = imp,
                    None => list.push(imp),
                }
            }
        }
    }
    Ok(list)
}
