//! Executable checks found in `<instance>/checks/` (stub: to be implemented).
//! Protocol: `<exe> describe` -> {id, doc}; `<exe> run` reads {setup, params, ctx} on stdin and
//! prints {status, message, evidence}.

use super::Check;
use anyhow::Result;
use std::path::Path;

/// Load every executable in `dir`, sorted by file name.
pub fn load_dir(_dir: &Path) -> Result<Vec<Box<dyn Check>>> {
    Ok(Vec::new())
}
