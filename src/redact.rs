//! Shape-only copy of a setup: layers, counts, kinds, severities, scopes, schedules and on/off
//! flags survive; every free-text value is dropped. Safe by construction: only allowlisted
//! enum-like fields are copied and ids become placeholders. Ported from the reference
//! `framework/lib/redact.mjs`.

use crate::layers::LAYERS;
use crate::plan::js_display;
use crate::util::{arr, get_path, is_truthy, set_path};
use crate::Doc;
use serde_json::{json, Map, Value};

const DESCRIPTION: &str = "Shape-only redaction of a real setup: counts, kinds, severities and schedules kept; every name, path and text replaced.";

/// Fields copied verbatim per keyed collection (anything not listed is dropped).
const KEEP: [(&str, &[&str]); 20] = [
    ("identity.accounts", &["kind", "access"]),
    ("brain.runtimes", &[]),
    ("brain.roster", &[]),
    ("brain.gates", &["threshold"]),
    ("instructions.sources", &["kind"]),
    (
        "instructions.rules",
        &["scope", "severity", "date", "status"],
    ),
    ("tools.mcp_servers", &["transport", "enabled"]),
    ("tools.skills", &[]),
    ("tools.plugins", &["enabled", "installed"]),
    ("tools.hooks", &["event"]),
    ("tools.commands", &[]),
    ("loops", &["runner", "enabled", "managed", "schedule"]),
    ("projects", &["status"]),
    ("business.lanes", &["status"]),
    ("data.state", &["kind"]),
    ("data.references", &[]),
    ("infra.clis", &["present"]),
    ("infra.repos", &[]),
    ("infra.services", &[]),
    ("safety.policies", &["check", "severity"]),
];

/// Placeholder id prefix per keyed collection; `item` for the rest.
const PREFIX: [(&str, &str); 19] = [
    ("identity.accounts", "account"),
    ("instructions.rules", "rule"),
    ("instructions.sources", "source"),
    ("tools.mcp_servers", "mcp"),
    ("tools.skills", "skill"),
    ("tools.plugins", "plugin"),
    ("tools.hooks", "hook"),
    ("tools.commands", "command"),
    ("loops", "loop"),
    ("projects", "project"),
    ("business.lanes", "lane"),
    ("data.state", "state"),
    ("data.references", "ref"),
    ("infra.repos", "repo"),
    ("infra.services", "service"),
    ("brain.runtimes", "runtime"),
    ("brain.roster", "worker"),
    ("brain.gates", "gate"),
    ("safety.policies", "policy"),
];

/// CLI names common enough to keep as they are.
const GENERIC_CLIS: [&str; 20] = [
    "claude",
    "node",
    "npm",
    "python3",
    "git",
    "gh",
    "brew",
    "docker",
    "codex",
    "gemini",
    "wrangler",
    "gcloud",
    "firebase",
    "eas",
    "xcodebuild",
    "ffmpeg",
    "rtk",
    "uv",
    "bun",
    "deno",
];

/// Status words a free-text status is reduced to; the first one contained wins.
const STATUS_WORDS: [&str; 17] = [
    "live",
    "active",
    "halted",
    "killed",
    "blocked",
    "superseded",
    "paused",
    "planned",
    "installed",
    "running",
    "loaded",
    "parked",
    "removed",
    "not-loaded",
    "backup-only",
    "dead",
    "frozen",
];

/// The `business.lanes[].status` enum.
const LANE_STATUSES: [&str; 6] = ["active", "paused", "halted", "killed", "blocked", "planned"];

fn keep_for(path: &str) -> &'static [&'static str] {
    KEEP.iter()
        .find(|(p, _)| *p == path)
        .map(|(_, k)| *k)
        .unwrap_or(&[])
}

fn prefix_for(path: &str) -> &'static str {
    PREFIX
        .iter()
        .find(|(p, _)| *p == path)
        .map(|(_, x)| *x)
        .unwrap_or("item")
}

/// The first status word inside the text, `other` for any other non-empty value, `None`
/// for an empty one.
fn clean_status(s: Option<&Doc>) -> Option<String> {
    let t = if is_truthy(s) {
        js_display(s).to_lowercase()
    } else {
        String::new()
    };
    STATUS_WORDS
        .iter()
        .find(|w| t.contains(*w))
        .map(|w| w.to_string())
        .or_else(|| is_truthy(s).then(|| "other".to_string()))
}

fn redact_schedule(s: &Doc) -> Doc {
    let Value::Object(o) = s else {
        return json!({});
    };
    let mut r = Map::new();
    if is_truthy(o.get("every_seconds")) {
        r.insert("every_seconds".into(), o["every_seconds"].clone());
    }
    if is_truthy(o.get("calendar")) {
        r.insert("calendar".into(), o["calendar"].clone());
    }
    if is_truthy(o.get("keep_alive")) {
        r.insert("keep_alive".into(), json!(true));
    }
    if is_truthy(o.get("run_at_load")) {
        r.insert("run_at_load".into(), json!(true));
    }
    if is_truthy(o.get("cron")) {
        r.insert("cron".into(), o["cron"].clone());
    }
    Value::Object(r)
}

/// JavaScript `r[k] ??= v`: set when missing or null, keeping an existing key's position.
fn default_to(r: &mut Map<String, Value>, k: &str, v: Value) {
    if r.get(k).map_or(true, Value::is_null) {
        r.insert(k.to_string(), v);
    }
}

fn redact_item(path: &str, it: &Doc, i: usize) -> Doc {
    let field = |k: &str| it.get(k);
    let mut r = Map::new();
    let id = match field("id") {
        Some(Value::String(s)) if path == "infra.clis" && GENERIC_CLIS.contains(&s.as_str()) => {
            json!(s)
        }
        _ => json!(format!("{}-{:03}", prefix_for(path), i + 1)),
    };
    r.insert("id".into(), id);
    for k in keep_for(path) {
        if let Some(v) = field(k) {
            let v = if *k == "schedule" {
                redact_schedule(v)
            } else {
                v.clone()
            };
            r.insert((*k).to_string(), v);
        }
    }
    if field("status").is_some() && path != "instructions.rules" {
        match clean_status(field("status")) {
            Some(w) => {
                r.insert("status".into(), json!(w));
            }
            // JavaScript sets `undefined`, which never reaches the output.
            None => {
                r.shift_remove("status");
            }
        }
    }
    // Fill required fields with neutral placeholders so the example validates.
    match path {
        "instructions.rules" => {
            r.insert("reason".into(), json!("redacted"));
            default_to(&mut r, "scope", json!("global"));
            default_to(&mut r, "severity", json!("standing"));
            if !r.contains_key("date") {
                r.insert("date".into(), Value::Null);
            }
        }
        "identity.accounts" => {
            default_to(&mut r, "kind", json!("unknown"));
            default_to(&mut r, "access", json!("unknown"));
        }
        "tools.hooks" => {
            r.insert("command".into(), json!("redacted"));
            default_to(&mut r, "event", json!("PreToolUse"));
        }
        "brain.gates" => {
            let n = arr(field("applies_to")).len();
            r.insert("applies_to".into(), json!(vec!["channel-kind"; n]));
        }
        "data.state" => {
            r.insert("path".into(), json!("~/redacted"));
            default_to(&mut r, "kind", json!("file"));
        }
        "instructions.sources" => {
            r.insert("path".into(), json!("~/redacted"));
        }
        "business.lanes" => {
            let st = field("status")
                .and_then(Value::as_str)
                .filter(|s| LANE_STATUSES.contains(s))
                .unwrap_or("active");
            r.insert("status".into(), json!(st));
        }
        "safety.policies" => {
            default_to(&mut r, "check", json!("custom"));
            default_to(&mut r, "severity", json!("standing"));
        }
        "loops" => {
            default_to(&mut r, "runner", json!("manual"));
            default_to(&mut r, "enabled", json!(false));
        }
        _ => {}
    }
    Value::Object(r)
}

/// Keep counts, kinds, severities, scopes, dates, schedules and flags; replace every name,
/// path, handle and text.
pub fn redact(setup: &Doc) -> Doc {
    let mut out =
        json!({ "version": 1, "meta": { "instance": "redacted", "description": DESCRIPTION } });
    for layer in LAYERS.iter() {
        for c in layer.collections {
            let Some(Value::Array(list)) = get_path(setup, c.path) else {
                continue;
            };
            if list.is_empty() {
                continue;
            }
            let items: Vec<Doc> = match c.key {
                None => {
                    let last = c.path.rsplit('.').next().unwrap_or(c.path);
                    (0..list.len())
                        .map(|i| json!(format!("{last}-entry-{:03}", i + 1)))
                        .collect()
                }
                Some(_) => list
                    .iter()
                    .enumerate()
                    .map(|(i, it)| redact_item(c.path, it, i))
                    .collect(),
            };
            set_path(&mut out, c.path, Value::Array(items));
        }
    }
    // Channels: kind and a status word only, under ids made from the kind.
    if let Some(Value::Object(chs)) = setup.get("channels") {
        for (kind, list) in chs {
            let Value::Array(list) = list else { continue };
            if list.is_empty() {
                continue;
            }
            let items: Vec<Doc> = list
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let mut m = Map::new();
                    m.insert("id".into(), json!(format!("{kind}-{:02}", i + 1)));
                    if is_truthy(c.get("status")) {
                        if let Some(w) = clean_status(c.get("status")) {
                            m.insert("status".into(), json!(w));
                        }
                    }
                    Value::Object(m)
                })
                .collect();
            let o = out.as_object_mut().expect("out is an object");
            let channels = o.entry("channels").or_insert_with(|| json!({}));
            if let Value::Object(ch) = channels {
                ch.insert(kind.clone(), Value::Array(items));
            }
        }
    }
    if let Some(mem) = setup.get("data").and_then(|d| d.get("memory")) {
        if is_truthy(Some(mem)) {
            let mut m = Map::new();
            for k in ["files", "by_type"] {
                if let Some(v) = mem.get(k) {
                    m.insert(k.to_string(), v.clone());
                }
            }
            let o = out.as_object_mut().expect("out is an object");
            let data = o.entry("data").or_insert_with(|| json!({}));
            if let Value::Object(d) = data {
                d.insert("memory".into(), Value::Object(m));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{desired_state, load_instance, Instance};
    use crate::layers::count_layers;
    use crate::schema::validate;
    use serde_json::{json, Value};
    use std::collections::HashSet;
    use std::path::Path;

    fn ids(v: &Doc, out: &mut HashSet<String>) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    if k == "id" {
                        if let Some(s) = x.as_str() {
                            out.insert(s.to_string());
                        }
                    }
                    ids(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| ids(x, out)),
            _ => {}
        }
    }

    #[test]
    fn redact_minimal_example_validates_and_drops_every_original_id() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/minimal");
        let inst = Instance {
            file: dir.join("setup.yaml"),
            dir,
        };
        let desired = desired_state(&load_instance(&inst).unwrap());
        let r = redact(&desired);
        let v = validate(&r);
        assert!(v.ok, "{:?}", v.errors);
        assert_eq!(r["meta"]["instance"], json!("redacted"));

        let mut orig = HashSet::new();
        ids(&desired, &mut orig);
        let mut red = HashSet::new();
        ids(&r, &mut red);
        assert!(orig.len() > 20, "{orig:?}");
        let survived: Vec<&String> = red.intersection(&orig).collect();
        assert!(survived.is_empty(), "original ids survived: {survived:?}");

        let text = serde_json::to_string(&r).unwrap();
        for leak in [
            "assistant-bot",
            "never-spend-money",
            "com.example",
            "example.com",
            "~/assistant",
            "Money only moves",
            "personal-workspace",
            "chat:write",
            "MAIL_APP_PASSWORD",
            "mcp__work-chat__",
        ] {
            assert!(!text.contains(leak), "{leak} leaked: {text}");
        }

        // Shape survives: the same number of items in every layer.
        let (a, b) = (count_layers(&desired), count_layers(&r));
        for (layer, c) in &a {
            assert_eq!(c.total, b[layer].total, "layer {layer}");
        }
        assert_eq!(
            r["loops"][0]["schedule"],
            json!({ "calendar": ["hour=8,minute=0"] })
        );
        assert_eq!(r["instructions"]["rules"][0]["severity"], json!("hard"));
    }

    #[test]
    fn redact_keeps_enum_like_fields_and_fills_required_ones() {
        let s = json!({
            "identity": { "accounts": [{ "id": "alex-mail", "principal": "alex@example.com", "status": "Live since May" }] },
            "brain": { "gates": [{ "id": "g", "threshold": "< 3/10", "applies_to": ["email", "social"], "model": "m" }] },
            "instructions": {
                "rules": [
                    { "id": "r1", "scope": "money", "severity": "hard", "date": "2026-01-01", "status": "active", "reason": "private reason" },
                    { "id": "r2", "statement": "private text" }
                ],
                "sources": [{ "id": "src", "path": "~/private/CLAUDE.md", "kind": "global" }]
            },
            "tools": {
                "hooks": [{ "id": "h", "event": "Stop", "matcher": "Bash", "command": "~/private/hook.sh" }, { "id": "h2" }],
                "permissions": { "allow": ["Read", "Write"], "ask": [] }
            },
            "loops": [{ "id": "com.example.job", "program": ["/bin/sh"], "managed": true, "status": "not-loaded",
                        "schedule": { "every_seconds": 60, "keep_alive": "yes", "run_at_load": false, "cron": "0 8 * * *", "path": "~/x" } }],
            "projects": [{ "id": "p", "status": null }, { "id": "q", "status": "Paused for now" }, { "id": "z", "status": "who knows" }],
            "business": { "lanes": [{ "id": "l1", "status": "halted", "goal": "g" }, { "id": "l2", "status": "on fire" }] },
            "data": {
                "state": [{ "id": "st", "path": "~/private/state.json" }],
                "memory": { "files": 12, "by_type": { "user": 2 }, "path": "~/private/memory" }
            },
            "infra": { "clis": [{ "id": "git", "present": true }, { "id": "alex-private-cli", "present": false }] },
            "safety": { "policies": [{ "id": "pol", "params": { "x": 1 } }] },
            "channels": { "slack": [{ "id": "C-home", "status": "HALTED 2026" }, { "id": "other" }], "sms": [{ "id": "x" }] }
        });
        let r = redact(&s);
        let item = |p: &str, i: usize| {
            serde_json::to_string(&crate::util::get_path(&r, p).unwrap()[i]).unwrap()
        };
        assert_eq!(
            item("identity.accounts", 0),
            r#"{"id":"account-001","status":"live","kind":"unknown","access":"unknown"}"#
        );
        assert_eq!(
            item("brain.gates", 0),
            r#"{"id":"gate-001","threshold":"< 3/10","applies_to":["channel-kind","channel-kind"]}"#
        );
        assert_eq!(
            item("instructions.rules", 0),
            r#"{"id":"rule-001","scope":"money","severity":"hard","date":"2026-01-01","status":"active","reason":"redacted"}"#
        );
        assert_eq!(
            item("instructions.rules", 1),
            r#"{"id":"rule-002","reason":"redacted","scope":"global","severity":"standing","date":null}"#
        );
        assert_eq!(
            item("instructions.sources", 0),
            r#"{"id":"source-001","kind":"global","path":"~/redacted"}"#
        );
        assert_eq!(
            item("tools.hooks", 0),
            r#"{"id":"hook-001","event":"Stop","command":"redacted"}"#
        );
        assert_eq!(
            item("tools.hooks", 1),
            r#"{"id":"hook-002","command":"redacted","event":"PreToolUse"}"#
        );
        assert_eq!(
            r["tools"]["permissions"],
            json!({ "allow": ["allow-entry-001", "allow-entry-002"] })
        );
        assert_eq!(
            item("loops", 0),
            r#"{"id":"loop-001","managed":true,"schedule":{"every_seconds":60,"keep_alive":true,"cron":"0 8 * * *"},"status":"loaded","runner":"manual","enabled":false}"#
        );
        assert_eq!(item("projects", 0), r#"{"id":"project-001"}"#);
        assert_eq!(
            item("projects", 1),
            r#"{"id":"project-002","status":"paused"}"#
        );
        assert_eq!(
            item("projects", 2),
            r#"{"id":"project-003","status":"other"}"#
        );
        assert_eq!(
            item("business.lanes", 0),
            r#"{"id":"lane-001","status":"halted"}"#
        );
        assert_eq!(
            item("business.lanes", 1),
            r#"{"id":"lane-002","status":"active"}"#
        );
        assert_eq!(
            item("data.state", 0),
            r#"{"id":"state-001","path":"~/redacted","kind":"file"}"#
        );
        assert_eq!(
            r["data"]["memory"],
            json!({ "files": 12, "by_type": { "user": 2 } })
        );
        assert_eq!(item("infra.clis", 0), r#"{"id":"git","present":true}"#);
        assert_eq!(
            item("infra.clis", 1),
            r#"{"id":"item-002","present":false}"#
        );
        assert_eq!(
            item("safety.policies", 0),
            r#"{"id":"policy-001","check":"custom","severity":"standing"}"#
        );
        assert_eq!(
            serde_json::to_string(&r["channels"]).unwrap(),
            r#"{"slack":[{"id":"slack-01","status":"halted"},{"id":"slack-02"}],"sms":[{"id":"sms-01"}]}"#
        );
        let text = serde_json::to_string(&r).unwrap();
        for leak in [
            "private",
            "alex",
            "C-home",
            "com.example",
            "Bash",
            "/bin/sh",
        ] {
            assert!(!text.contains(leak), "{leak} leaked: {text}");
        }
    }

    #[test]
    fn redact_empty_setup_is_just_the_header() {
        let r = redact(&json!({ "tools": { "skills": [] }, "channels": { "slack": [] } }));
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            r#"{"version":1,"meta":{"instance":"redacted","description":"Shape-only redaction of a real setup: counts, kinds, severities and schedules kept; every name, path and text replaced."}}"#
        );
    }
}
