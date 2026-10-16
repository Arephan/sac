//! Manifest validation: the JSON schema, duplicate ids inside keyed collections, and
//! cross-references (lane loops, policy rules, channel identities) that become warnings.

use crate::layers::LAYERS;
use crate::util::{arr, get_path, s};
use crate::{Doc, SCHEMA_JSON};
use std::collections::HashSet;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Validation {
    pub ok: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

fn validator() -> &'static jsonschema::Validator {
    static V: OnceLock<jsonschema::Validator> = OnceLock::new();
    V.get_or_init(|| {
        let schema: serde_json::Value = serde_json::from_str(SCHEMA_JSON).expect("schema parses");
        jsonschema::validator_for(&schema).expect("schema compiles")
    })
}

/// Validate a whole document. `ok` is false only for schema errors and duplicate ids;
/// dangling cross-references are warnings.
pub fn validate(doc: &Doc) -> Validation {
    let mut errors: Vec<String> = validator()
        .iter_errors(doc)
        .map(|e| {
            let p = e.instance_path().to_string();
            let p = if p.is_empty() { "/".to_string() } else { p };
            format!("{p} {e}")
        })
        .collect();

    for layer in LAYERS.iter() {
        for c in layer.collections {
            let Some(key) = c.key else { continue };
            let mut seen = HashSet::new();
            for it in arr(get_path(doc, c.path)) {
                let id = s(it.get(key)).to_string();
                if !seen.insert(id.clone()) {
                    errors.push(format!("{}: duplicate id \"{}\"", c.path, id));
                }
            }
        }
    }

    let mut warnings = Vec::new();
    let ids = |path: &str| -> HashSet<String> {
        arr(get_path(doc, path))
            .iter()
            .map(|x| s(x.get("id")).to_string())
            .collect()
    };
    let loop_ids = ids("loops");
    let rule_ids = ids("instructions.rules");
    let acc_ids = ids("identity.accounts");
    for lane in arr(get_path(doc, "business.lanes")) {
        for l in arr(lane.get("loops")) {
            let l = s(Some(l));
            if !loop_ids.contains(l) {
                warnings.push(format!(
                    "business.lanes.{}: loop \"{}\" not in loops",
                    s(lane.get("id")),
                    l
                ));
            }
        }
    }
    for p in arr(get_path(doc, "safety.policies")) {
        if let Some(r) = p.get("rule").and_then(|x| x.as_str()) {
            if !rule_ids.contains(r) {
                warnings.push(format!(
                    "safety.policies.{}: rule \"{}\" not in instructions.rules",
                    s(p.get("id")),
                    r
                ));
            }
        }
    }
    if let Some(ch) = doc.get("channels").and_then(|x| x.as_object()) {
        for (kind, list) in ch {
            for c in arr(Some(list)) {
                let ident = s(c.get("identity"));
                if !ident.is_empty() && ident != "none" && !acc_ids.contains(ident) {
                    warnings.push(format!(
                        "channels.{}.{}: identity \"{}\" not in identity.accounts",
                        kind,
                        s(c.get("id")),
                        ident
                    ));
                }
            }
        }
    }

    Validation {
        ok: errors.is_empty(),
        errors,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn minimal_valid_and_schema_errors() {
        let v = validate(&json!({ "version": 1, "meta": { "instance": "x" } }));
        assert!(v.ok, "{:?}", v.errors);
        let bad = validate(
            &json!({ "version": 1, "meta": { "instance": "x" }, "instructions": { "rules": [{ "id": "r", "scope": "s", "severity": "loud", "date": null, "reason": "r" }] } }),
        );
        assert!(!bad.ok);
        assert!(
            bad.errors.iter().any(|e| e.contains("severity")),
            "{:?}",
            bad.errors
        );
    }

    #[test]
    fn duplicate_ids_and_cross_reference_warnings() {
        let d = json!({
            "version": 1, "meta": { "instance": "x" },
            "loops": [{ "id": "a", "runner": "manual", "enabled": false }, { "id": "a", "runner": "manual", "enabled": false }],
            "business": { "lanes": [{ "id": "l", "status": "active", "loops": ["missing"] }] },
            "safety": { "policies": [{ "id": "p", "check": "c", "severity": "hard", "rule": "nope" }] },
            "channels": { "slack": [{ "id": "home", "identity": "ghost" }] }
        });
        let v = validate(&d);
        assert!(v.errors.iter().any(|e| e.contains("duplicate id \"a\"")));
        assert_eq!(v.warnings.len(), 3, "{:?}", v.warnings);
    }
}
