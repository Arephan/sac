//! Shape-only copy of a setup (stub: to be implemented).

use crate::Doc;

/// Keep counts, kinds, severities, scopes, dates, schedules and flags; replace every name,
/// path, handle and text.
pub fn redact(_setup: &Doc) -> Doc {
    serde_json::json!({ "version": 1, "meta": { "instance": "redacted" } })
}
