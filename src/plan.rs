//! Desired vs live diff by layer collection. Pure: reads two documents, writes nothing.
//! Ported from the Node prototype (tag v0.2.0): keyed collections diff by id, plain
//! lists diff as string sets, scalar objects diff field by field.

use crate::layers::LAYERS;
use crate::util::{get_path, is_truthy, stable_string};
use crate::Doc;
use indexmap::{IndexMap, IndexSet};
use serde_json::{Map, Value};
use std::collections::HashSet;

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

/// Fields that change on their own and say nothing about configuration.
const VOLATILE: [&str; 2] = ["generated_at", "last_commit"];

/// Changes listed per layer by `format_plan` unless verbose.
const LIMIT: usize = 12;

/// `status` values that mean the same as another (a running launchd job is also loaded).
fn status_equiv(v: &Doc) -> Option<Doc> {
    match v.as_str() {
        Some("running") => Some(Value::String("loaded".to_string())),
        _ => None,
    }
}

/// Drop volatile keys and map equivalent statuses, at every depth.
fn clean(v: &Doc) -> Doc {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(clean).collect()),
        Value::Object(o) => {
            let mut out = Map::new();
            for (k, x) in o {
                if VOLATILE.contains(&k.as_str()) {
                    continue;
                }
                let x = if k == "status" {
                    status_equiv(x).unwrap_or_else(|| clean(x))
                } else {
                    clean(x)
                };
                out.insert(k.clone(), x);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// A JavaScript array index ("0", "7", ... below 2^32 - 1). Such keys enumerate first.
fn array_index(k: &str) -> Option<usize> {
    if k == "0" {
        return Some(0);
    }
    if k.is_empty() || k.starts_with('0') || !k.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    k.parse::<u64>()
        .ok()
        .filter(|n| *n < u64::from(u32::MAX))
        .map(|n| n as usize)
}

/// Property names in JavaScript enumeration order (`Object.keys`).
fn js_keys(v: &Doc) -> Vec<String> {
    match v {
        Value::Object(o) => {
            let mut idx: Vec<(usize, &String)> = o
                .keys()
                .filter_map(|k| array_index(k).map(|n| (n, k)))
                .collect();
            idx.sort_unstable();
            let mut keys: Vec<String> = idx.into_iter().map(|(_, k)| k.clone()).collect();
            keys.extend(o.keys().filter(|k| array_index(k).is_none()).cloned());
            keys
        }
        Value::Array(a) => (0..a.len()).map(|i| i.to_string()).collect(),
        Value::String(s) => (0..s.chars().count()).map(|i| i.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// `stableStringify(v[k])`, or `None` where JavaScript would read `undefined`.
fn field_repr(v: &Doc, k: &str) -> Option<String> {
    match v {
        Value::Object(o) => o.get(k).map(stable_string),
        Value::Array(a) => array_index(k).and_then(|i| a.get(i)).map(stable_string),
        Value::String(s) => array_index(k)
            .and_then(|i| s.chars().nth(i))
            .map(|c| stable_string(&Value::String(c.to_string()))),
        _ => None,
    }
}

/// Names of the fields whose values differ once both sides are cleaned. The reference runs
/// `clean` on each field's value only, which never reaches an item's own `last_commit` or
/// `status`; cleaning the whole item first makes those top-level fields count as volatile and
/// equivalent too, which is what the VOLATILE and STATUS_EQUIV tables are for.
fn changed_fields(a: &Doc, b: &Doc) -> Vec<String> {
    let (a, b) = (clean(a), clean(b));
    let mut keys: IndexSet<String> = js_keys(&a).into_iter().collect();
    keys.extend(js_keys(&b));
    keys.into_iter()
        .filter(|k| field_repr(&a, k) != field_repr(&b, k))
        .collect()
}

/// `String(v)` in JavaScript: how an id is shown when it is not a plain string.
pub(crate) fn js_display(v: Option<&Doc>) -> String {
    match v {
        None => "undefined".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => match n.as_f64() {
            Some(f) if n.is_f64() && f == 0.0 => "0".to_string(),
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e21 => format!("{f:.0}"),
            _ => n.to_string(),
        },
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::Null => String::new(),
                other => js_display(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_string(),
    }
}

/// `getPath(doc, path) || []`; `None` when the value is set but is not a list.
fn list_at<'a>(doc: &'a Doc, path: &str) -> Option<&'a [Doc]> {
    match get_path(doc, path) {
        v if !is_truthy(v) => Some(&[]),
        Some(Value::Array(a)) => Some(a.as_slice()),
        _ => None,
    }
}

fn change(op: char, path: &str, id: String, fields: Vec<String>) -> Change {
    Change {
        op,
        path: path.to_string(),
        id,
        fields,
    }
}

/// Lists of strings: set membership, every occurrence reported.
fn diff_plain(path: &str, d: &[Doc], l: &[Doc], out: &mut Vec<Change>) {
    let ds: HashSet<String> = d.iter().map(stable_string).collect();
    let ls: HashSet<String> = l.iter().map(stable_string).collect();
    for x in d {
        if !ls.contains(&stable_string(x)) {
            out.push(change('+', path, js_display(Some(x)), Vec::new()));
        }
    }
    for x in l {
        if !ds.contains(&stable_string(x)) {
            out.push(change('-', path, js_display(Some(x)), Vec::new()));
        }
    }
}

/// Items by key, like a JavaScript `Map`: a repeated key keeps its first position and its
/// last item. The map key tells `1` from `"1"`; the display string is how the id prints.
fn by_key<'a>(list: &'a [Doc], key: &str) -> IndexMap<String, (String, &'a Doc)> {
    let mut m = IndexMap::new();
    for x in list {
        let id = x.get(key);
        let k = id
            .map(stable_string)
            .unwrap_or_else(|| "\u{0}undefined".to_string());
        m.insert(k, (js_display(id), x));
    }
    m
}

/// Lists of objects keyed by id: added, changed (with field names), untracked.
fn diff_keyed(path: &str, key: &str, d: &[Doc], l: &[Doc], out: &mut Vec<Change>) {
    let dm = by_key(d, key);
    let lm = by_key(l, key);
    for (k, (id, x)) in &dm {
        match lm.get(k) {
            None => out.push(change('+', path, id.clone(), Vec::new())),
            Some((_, y)) => {
                let f = changed_fields(x, y);
                if !f.is_empty() {
                    out.push(change('~', path, id.clone(), f));
                }
            }
        }
    }
    for (k, (id, _)) in &lm {
        if !dm.contains_key(k) {
            out.push(change('-', path, id.clone(), Vec::new()));
        }
    }
}

/// Diff `desired` against `live`.
pub fn plan(desired: &Doc, live: &Doc) -> Plan {
    let empty = Value::Object(Map::new());
    let scalar = |doc: &'_ Doc, path: &str| -> Doc {
        match get_path(doc, path) {
            Some(v) if is_truthy(Some(v)) => v.clone(),
            _ => empty.clone(),
        }
    };
    let mut layers = Vec::new();
    for layer in LAYERS.iter() {
        let mut changes = Vec::new();
        for c in layer.collections {
            let (Some(d), Some(l)) = (list_at(desired, c.path), list_at(live, c.path)) else {
                continue;
            };
            match c.key {
                None => diff_plain(c.path, d, l, &mut changes),
                Some(key) => diff_keyed(c.path, key, d, l, &mut changes),
            }
        }
        for s in layer.scalars {
            let f = changed_fields(&scalar(desired, s), &scalar(live, s));
            if !f.is_empty() {
                let id = s.rsplit('.').next().unwrap_or(s).to_string();
                changes.push(change('~', s, id, f));
            }
        }
        layers.push(LayerPlan {
            layer: layer.id.to_string(),
            changes,
        });
    }
    let mut totals = Totals::default();
    for c in layers.iter().flat_map(|l| &l.changes) {
        match c.op {
            '+' => totals.add += 1,
            '~' => totals.change += 1,
            '-' => totals.untracked += 1,
            _ => {}
        }
    }
    Plan { layers, totals }
}

/// Text block mirroring the reference's `formatPlan` (12 changes per layer unless verbose).
pub fn format_plan(p: &Plan, verbose: bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    for lp in &p.layers {
        if lp.changes.is_empty() {
            lines.push(format!("  {:<13} no changes", lp.layer));
            continue;
        }
        let n = |op: char| lp.changes.iter().filter(|c| c.op == op).count();
        lines.push(format!(
            "  {:<13} +{} ~{} -{}",
            lp.layer,
            n('+'),
            n('~'),
            n('-')
        ));
        let shown = if verbose {
            &lp.changes[..]
        } else {
            &lp.changes[..lp.changes.len().min(LIMIT)]
        };
        for c in shown {
            let fields = if c.fields.is_empty() {
                String::new()
            } else {
                format!(" ({})", c.fields.join(", "))
            };
            lines.push(format!("      {} {} {}{}", c.op, c.path, c.id, fields));
        }
        if shown.len() < lp.changes.len() {
            lines.push(format!(
                "      … {} more (use --verbose)",
                lp.changes.len() - shown.len()
            ));
        }
    }
    lines.push(String::new());
    lines.push(
        "  + = in desired, not on the machine   ~ = differs   - = on the machine, not in desired"
            .to_string(),
    );
    lines.push(format!(
        "  Plan: {} to add, {} to change, {} untracked. Nothing was written.",
        p.totals.add, p.totals.change, p.totals.untracked
    ));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::LAYERS;
    use serde_json::json;

    fn layer<'a>(p: &'a Plan, id: &str) -> &'a [Change] {
        &p.layers
            .iter()
            .find(|l| l.layer == id)
            .expect("layer present")
            .changes
    }

    fn show(cs: &[Change]) -> Vec<String> {
        cs.iter()
            .map(|c| {
                let f = if c.fields.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", c.fields.join(", "))
                };
                format!("{} {} {}{}", c.op, c.path, c.id, f)
            })
            .collect()
    }

    #[test]
    fn plan_keyed_collections_add_change_untracked() {
        let d = json!({ "identity": { "accounts": [
            { "id": "bot", "kind": "chat_bot", "access": "can_post" },
            { "id": "work", "kind": "chat_user", "access": "forbidden" }
        ] } });
        let l = json!({ "identity": { "accounts": [
            { "id": "bot", "kind": "other", "access": "can_post", "note": "n" },
            { "id": "extra", "kind": "x", "access": "unknown" }
        ] } });
        let p = plan(&d, &l);
        assert_eq!(
            (p.totals.add, p.totals.change, p.totals.untracked),
            (1, 1, 1)
        );
        assert_eq!(
            show(layer(&p, "identity")),
            vec![
                "~ identity.accounts bot (kind, note)",
                "+ identity.accounts work",
                "- identity.accounts extra",
            ]
        );
    }

    #[test]
    fn plan_unkeyed_lists_are_string_sets() {
        let d = json!({ "tools": {
            "permissions": { "allow": ["Read", "Bash(git status*)"], "deny": ["Bash(rm*)"] },
            "denied_mcp_servers": ["work-chat"]
        } });
        let l = json!({ "tools": { "permissions": { "allow": ["Read", "Write"], "deny": ["Bash(rm*)"] } } });
        let p = plan(&d, &l);
        assert_eq!(
            show(layer(&p, "tools")),
            vec![
                "+ tools.permissions.allow Bash(git status*)",
                "- tools.permissions.allow Write",
                "+ tools.denied_mcp_servers work-chat",
            ]
        );
        assert_eq!(
            (p.totals.add, p.totals.change, p.totals.untracked),
            (2, 0, 1)
        );
    }

    #[test]
    fn plan_scalars_report_changed_field_names() {
        let d = json!({ "brain": { "claude_code": { "model": "opus", "effort": "high" } } });
        let l = json!({ "brain": { "claude_code": { "model": "sonnet", "effort": "high", "theme": "dark" } } });
        let p = plan(&d, &l);
        assert_eq!(
            show(layer(&p, "brain")),
            vec!["~ brain.claude_code claude_code (model, theme)"]
        );
        let p2 = plan(
            &json!({}),
            &json!({ "infra": { "machine": { "os": "macos" } } }),
        );
        assert_eq!(
            show(layer(&p2, "infra")),
            vec!["~ infra.machine machine (os)"]
        );
        assert_eq!(p2.totals.change, 1);
    }

    #[test]
    fn plan_ignores_volatile_fields_and_running_vs_loaded() {
        let d = json!({
            "infra": { "repos": [{ "id": "r", "path": "~/r", "last_commit": "2026-01-01 init" }] },
            "loops": [{ "id": "l", "runner": "launchd", "enabled": true, "status": "loaded",
                        "schedule": { "every_seconds": 60, "generated_at": "a" } }],
            "data": { "memory": { "files": 3, "generated_at": "x" } }
        });
        let l = json!({
            "infra": { "repos": [{ "id": "r", "path": "~/r" }] },
            "loops": [{ "id": "l", "runner": "launchd", "enabled": true, "status": "running",
                        "schedule": { "every_seconds": 60, "generated_at": "b" } }],
            "data": { "memory": { "files": 3, "generated_at": "y" } }
        });
        let p = plan(&d, &l);
        let all: Vec<String> = p.layers.iter().flat_map(|x| show(&x.changes)).collect();
        assert!(all.is_empty(), "{all:?}");
        // Only running -> loaded is equivalent; other status changes still count.
        let l2 = json!({ "loops": [{ "id": "l", "runner": "launchd", "enabled": true, "status": "installed:not-loaded",
                                     "schedule": { "every_seconds": 60 } }] });
        let p2 = plan(&json!({ "loops": d["loops"].clone() }), &l2);
        assert_eq!(show(layer(&p2, "loops")), vec!["~ loops l (status)"]);
    }

    #[test]
    fn plan_lists_every_layer_in_order() {
        let p = plan(&json!({}), &json!({}));
        let ids: Vec<&str> = p.layers.iter().map(|l| l.layer.as_str()).collect();
        let want: Vec<&str> = LAYERS.iter().map(|l| l.id).collect();
        assert_eq!(ids, want);
        assert_eq!(
            (p.totals.add, p.totals.change, p.totals.untracked),
            (0, 0, 0)
        );
    }

    #[test]
    fn plan_skips_collections_that_are_not_lists_and_shows_odd_ids() {
        let d =
            json!({ "loops": { "not": "a list" }, "projects": [{ "id": 7 }, { "name": "no id" }] });
        let p = plan(&d, &json!({ "loops": [{ "id": "x" }] }));
        assert!(layer(&p, "loops").is_empty());
        assert_eq!(
            show(layer(&p, "projects")),
            vec!["+ projects 7", "+ projects undefined"]
        );
    }

    #[test]
    fn plan_format_layout_limit_and_verbose() {
        let allow: Vec<String> = (1..=13).map(|i| format!("Tool{i:02}")).collect();
        let d = json!({
            "tools": { "permissions": { "allow": allow } },
            "loops": [{ "id": "l", "runner": "cron", "enabled": true }]
        });
        let l = json!({ "loops": [{ "id": "l", "runner": "cron", "enabled": false }] });
        let p = plan(&d, &l);
        let text = format_plan(&p, false);
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines[0], "  identity      no changes");
        assert_eq!(lines[3], "  tools         +13 ~0 -0");
        assert_eq!(lines[4], "      + tools.permissions.allow Tool01");
        assert_eq!(lines[15], "      + tools.permissions.allow Tool12");
        assert_eq!(lines[16], "      … 1 more (use --verbose)");
        assert_eq!(lines[17], "  loops         +0 ~1 -0");
        assert_eq!(lines[18], "      ~ loops l (enabled)");
        assert!(!text.contains("Tool13"));
        assert!(text.ends_with(
            "  safety        no changes\n\n  + = in desired, not on the machine   ~ = differs   - = on the machine, not in desired\n  Plan: 13 to add, 1 to change, 0 untracked. Nothing was written."
        ), "{text}");
        let v = format_plan(&p, true);
        assert!(v.contains("      + tools.permissions.allow Tool13\n"));
        assert!(!v.contains("more (use --verbose)"));
    }

    #[test]
    fn plan_json_has_fields_only_for_changes() {
        let p = plan(
            &json!({ "projects": [{ "id": "a", "status": "x" }, { "id": "b" }] }),
            &json!({ "projects": [{ "id": "a", "status": "y" }] }),
        );
        let v = serde_json::to_value(layer(&p, "projects")).unwrap();
        assert_eq!(
            v,
            json!([
                { "op": "~", "path": "projects", "id": "a", "fields": ["status"] },
                { "op": "+", "path": "projects", "id": "b" }
            ])
        );
    }
}
