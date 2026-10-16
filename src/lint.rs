//! Secret and personal-data lint over the shareable tree (stub: to be implemented).

use indexmap::IndexMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize)]
pub struct LintFinding {
    pub at: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct LintReport {
    pub files: usize,
    pub terms: usize,
    pub allowed: Vec<String>,
    pub findings: Vec<LintFinding>,
}

/// Terms derived from every `instance/<name>/` (denylist, account principals, handles, home users) -> origin.
pub fn instance_terms(_instance_root: &Path) -> IndexMap<String, String> {
    IndexMap::new()
}

/// Phrases allowed through, from every `instance/<name>/lint-allow.txt`.
pub fn instance_allow(_instance_root: &Path) -> Vec<String> {
    Vec::new()
}

/// Everything under `root` except target/, node_modules/, out/, .git/, instance/, .superpowers/, .worktrees/.
pub fn default_targets(_root: &Path) -> Vec<PathBuf> {
    Vec::new()
}

/// Scan the targets.
pub fn lint(
    _root: &Path,
    _targets: &[PathBuf],
    _terms: &IndexMap<String, String>,
    _allow: &[String],
) -> LintReport {
    LintReport::default()
}

pub fn format_lint(_r: &LintReport) -> String {
    String::new()
}
