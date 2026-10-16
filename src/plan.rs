//! Desired vs live diff by layer collection (stub: to be implemented).

use crate::Doc;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Change {
    pub op: char,
    pub path: String,
    pub id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct LayerPlan {
    pub layer: String,
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Totals {
    pub add: usize,
    pub change: usize,
    pub untracked: usize,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Plan {
    pub layers: Vec<LayerPlan>,
    pub totals: Totals,
}

/// Diff `desired` against `live`.
pub fn plan(_desired: &Doc, _live: &Doc) -> Plan {
    Plan::default()
}

/// Text block mirroring the reference's `formatPlan` (12 changes per layer unless verbose).
pub fn format_plan(_p: &Plan, _verbose: bool) -> String {
    String::new()
}
