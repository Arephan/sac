//! Executable importers found in `<instance>/importers/` (stub: to be implemented).
//! Protocol: `<exe> describe` -> {id, layers, description}; `<exe> detect` exit 0 = applies;
//! `<exe> run` reads options JSON on stdin, prints a fragment on stdout. Env: SAC_HOME, SAC_REAL_HOME.

use super::Importer;
use anyhow::Result;
use std::path::Path;

/// Load every executable in `dir`, sorted by file name.
pub fn load_dir(_dir: &Path) -> Result<Vec<Box<dyn Importer>>> {
    Ok(Vec::new())
}
