//! Path, string and document helpers shared by every module. Ported from the reference
//! the reference at tag v0.2.0; semantics (deep merge by id, stable stringify, slug) are kept exact
//! so that manifests written by either implementation diff cleanly.

use crate::Doc;
use regex::Regex;
use serde_json::{Map, Value};
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

/// Expand `~`, `~/x`, `$HOME` and `${HOME}` against `home`.
pub fn expand_home(p: &str, home: &Path) -> String {
    let h = home.to_string_lossy();
    let h = h.trim_end_matches('/');
    if p == "~" {
        return h.to_string();
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return format!("{h}/{rest}");
    }
    p.replace("${HOME}", h).replace("$HOME", h)
}

/// Replace the home directory in a string with `~`. `home/x` becomes `~/x`; a bare `home`
/// followed by the end of the string, whitespace or a quote becomes `~`.
pub fn tilde(p: &str, home: &Path) -> String {
    let h = home.to_string_lossy();
    let h = h.trim_end_matches('/');
    if h.is_empty() {
        return p.to_string();
    }
    let s = p.replace(&format!("{h}/"), "~/");
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(i) = rest.find(h) {
        let after = &rest[i + h.len()..];
        let boundary = after.is_empty()
            || after.starts_with(|c: char| c.is_whitespace() || c == '"' || c == '\'');
        out.push_str(&rest[..i]);
        if boundary {
            out.push('~');
        } else {
            out.push_str(h);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// `tilde` applied to every string inside a document.
pub fn tilde_deep(v: &Doc, home: &Path) -> Doc {
    match v {
        Value::String(s) => Value::String(tilde(s, home)),
        Value::Array(a) => Value::Array(a.iter().map(|x| tilde_deep(x, home)).collect()),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, x)| (k.clone(), tilde_deep(x, home)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Claude Code names a project directory by replacing every non-alphanumeric character with `-`.
pub fn claude_project_slug(abs: &str) -> String {
    abs.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Lowercase ASCII slug: accents stripped, runs of other characters collapsed to `-`,
/// trimmed, cut to `max` characters, `item` when nothing is left.
pub fn slug(s: &str, max: usize) -> String {
    let base: String = s
        .nfkd()
        .filter(|c| !('\u{0300}'..='\u{036f}').contains(c))
        .collect::<String>()
        .to_lowercase();
    let mut out = String::new();
    let mut dash = false;
    for c in base.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    let cut: String = trimmed.chars().take(max).collect();
    let cut = cut.trim_end_matches('-');
    if cut.is_empty() {
        "item".to_string()
    } else {
        cut.to_string()
    }
}

/// Expand `${env:NAME}` and `${vars.NAME}` in every string of a document. Keys that name a
/// credential (`credential_ref`, `env`, `headers`, `token`, ...) are left unexpanded, so a
/// secret can never be copied out of the environment into a rendered file.
pub fn resolve_vars(v: &Doc, vars: &Doc) -> Doc {
    fn env_re() -> &'static Regex {
        static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        R.get_or_init(|| Regex::new(r"\$\{env:([A-Za-z_][A-Za-z0-9_]*)\}").unwrap())
    }
    fn var_re() -> &'static Regex {
        static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        R.get_or_init(|| Regex::new(r"\$\{vars\.([A-Za-z_][A-Za-z0-9_]*)\}").unwrap())
    }
    fn sub(s: &str, vars: &Doc, expand_env: bool) -> String {
        if !s.contains("${") {
            return s.to_string();
        }
        let mut out = s.to_string();
        if expand_env {
            out = env_re()
                .replace_all(&out, |c: &regex::Captures| {
                    std::env::var(&c[1]).unwrap_or_default()
                })
                .into_owned();
        }
        var_re()
            .replace_all(&out, |c: &regex::Captures| match vars.get(&c[1]) {
                Some(Value::String(x)) => x.clone(),
                Some(Value::Null) | None => String::new(),
                Some(other) => other.to_string(),
            })
            .into_owned()
    }
    fn walk(v: &Doc, vars: &Doc, key: Option<&str>) -> Doc {
        let secret_key = key
            .map(|k| k == "credential_ref" || crate::secrets::is_secret_key(k))
            .unwrap_or(false);
        match v {
            Value::String(s) => Value::String(sub(s, vars, !secret_key)),
            Value::Array(a) => Value::Array(a.iter().map(|x| walk(x, vars, key)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, x)| (k.clone(), walk(x, vars, Some(k))))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    walk(v, vars, None)
}

/// Follow a dotted path through objects.
pub fn get_path<'a>(doc: &'a Doc, path: &str) -> Option<&'a Doc> {
    let mut cur = doc;
    for k in path.split('.') {
        cur = cur.get(k)?;
    }
    Some(cur)
}

/// Set a dotted path, creating intermediate objects (and replacing non-objects on the way).
pub fn set_path(doc: &mut Doc, path: &str, v: Doc) {
    let keys: Vec<&str> = path.split('.').collect();
    let mut cur = doc;
    for k in &keys[..keys.len() - 1] {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        let m = cur.as_object_mut().unwrap();
        if !m.get(*k).map(|x| x.is_object()).unwrap_or(false) {
            m.insert((*k).to_string(), Value::Object(Map::new()));
        }
        cur = m.get_mut(*k).unwrap();
    }
    if !cur.is_object() {
        *cur = Value::Object(Map::new());
    }
    cur.as_object_mut()
        .unwrap()
        .insert(keys[keys.len() - 1].to_string(), v);
}

fn all_objects(list: &[Doc]) -> bool {
    list.iter().all(|x| x.is_object())
}

/// Merge two lists. Lists of objects merge by `id` (matching items deep-merged, new ones
/// appended); anything else is a de-duplicated concatenation.
pub fn merge_by_id(a: &[Doc], b: &[Doc]) -> Vec<Doc> {
    if !all_objects(a) || !all_objects(b) {
        let mut out: Vec<Doc> = Vec::new();
        for x in a.iter().chain(b.iter()) {
            if !out.contains(x) {
                out.push(x.clone());
            }
        }
        return out;
    }
    let mut out: Vec<Doc> = a.to_vec();
    for item in b {
        let id = item.get("id");
        let pos = id.and_then(|id| out.iter().position(|x| x.get("id") == Some(id)));
        match pos {
            Some(i) => out[i] = deep_merge(Some(&out[i]), Some(item)),
            None => out.push(item.clone()),
        }
    }
    out
}

/// Deep merge `b` onto `a`. Objects merge key by key; a list merged with an object keyed by id
/// (`{ some-id: { field: v } }`) merges onto the matching list items; two lists merge by id.
pub fn deep_merge(a: Option<&Doc>, b: Option<&Doc>) -> Doc {
    match (a, b) {
        (Some(a), None) => a.clone(),
        (None, Some(b)) => b.clone(),
        (None, None) => Value::Null,
        (Some(Value::Array(a)), Some(Value::Object(b))) => {
            let items: Vec<Doc> = b
                .iter()
                .map(|(id, v)| {
                    let mut m = Map::new();
                    m.insert("id".to_string(), Value::String(id.clone()));
                    match v {
                        Value::Object(o) => {
                            for (k, x) in o {
                                m.insert(k.clone(), x.clone());
                            }
                        }
                        other => {
                            m.insert("value".to_string(), other.clone());
                        }
                    }
                    Value::Object(m)
                })
                .collect();
            Value::Array(merge_by_id(a, &items))
        }
        (Some(Value::Array(a)), Some(Value::Array(b))) => Value::Array(merge_by_id(a, b)),
        (Some(Value::Object(a)), Some(Value::Object(b))) => {
            let mut out = a.clone();
            for (k, v) in b {
                let merged = deep_merge(a.get(k), Some(v));
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        (Some(_), Some(b)) => b.clone(),
    }
}

/// JSON with object keys sorted, for order-independent comparison and hashing.
pub fn stable_string(v: &Doc) -> String {
    match v {
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(stable_string).collect::<Vec<_>>().join(",")
        ),
        Value::Object(o) => {
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            format!(
                "{{{}}}",
                keys.iter()
                    .map(|k| format!("{}:{}", Value::String((*k).clone()), stable_string(&o[*k])))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
        other => other.to_string(),
    }
}

/// `*` wildcard glob to an anchored, case-insensitive regex.
pub fn glob_to_regex(g: &str) -> Regex {
    let body: Vec<String> = g.split('*').map(regex::escape).collect();
    Regex::new(&format!("(?i)^{}$", body.join(".*"))).unwrap_or_else(|_| Regex::new("^$").unwrap())
}

/// Today's date, UTC, `YYYY-MM-DD`.
pub fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Now, UTC, RFC 3339 with milliseconds and a `Z` suffix.
pub fn now_iso() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

/// Collapse whitespace and cut to `max` characters with an ellipsis.
pub fn one_line(s: &str, max: usize) -> String {
    let t: Vec<&str> = s.split_whitespace().collect();
    let t = t.join(" ");
    if t.chars().count() > max {
        let cut: String = t.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    } else {
        t
    }
}

/// Dotted paths of every secret-shaped string in a document (`a.b[2].c: slack-token`).
pub fn secret_paths(doc: &Doc) -> Vec<String> {
    fn walk(v: &Doc, at: &str, out: &mut Vec<String>) {
        match v {
            Value::String(s) => {
                for h in crate::secrets::find_secrets(s) {
                    out.push(format!("{at}: {}", h.id));
                }
            }
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    walk(x, &format!("{at}[{i}]"), out);
                }
            }
            Value::Object(o) => {
                for (k, x) in o {
                    let p = if at.is_empty() {
                        k.clone()
                    } else {
                        format!("{at}.{k}")
                    };
                    walk(x, &p, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(doc, "", &mut out);
    out
}

/// The strings of an array value; anything else is empty.
pub fn str_list(v: Option<&Doc>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

/// The items of an array value; anything else is an empty slice.
pub fn arr(v: Option<&Doc>) -> &[Doc] {
    match v {
        Some(Value::Array(a)) => a.as_slice(),
        _ => &[],
    }
}

/// A string value, or `""`.
pub fn s(v: Option<&Doc>) -> &str {
    v.and_then(|x| x.as_str()).unwrap_or("")
}

/// JavaScript-style truthiness for a value (absent, null, false, 0, "" are false).
pub fn is_truthy(v: Option<&Doc>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn home() -> PathBuf {
        PathBuf::from("/Users/alex")
    }

    #[test]
    fn expand_and_tilde_round_trip() {
        assert_eq!(expand_home("~/x/y", &home()), "/Users/alex/x/y");
        assert_eq!(expand_home("~", &home()), "/Users/alex");
        assert_eq!(expand_home("$HOME/z", &home()), "/Users/alex/z");
        assert_eq!(tilde("/Users/alex/x", &home()), "~/x");
        assert_eq!(tilde("cd /Users/alex && ls", &home()), "cd ~ && ls");
        let longer = format!("/Users/{}/x", "alexandra");
        assert_eq!(tilde(&longer, &home()), longer);
    }

    #[test]
    fn slug_strips_accents_and_limits() {
        assert_eq!(slug("Élise & Co!", 60), "elise-co");
        assert_eq!(slug("---", 10), "item");
        assert_eq!(slug("a-very-long-name-here", 6), "a-very");
        assert_eq!(claude_project_slug("/Users/alex"), "-Users-alex");
    }

    #[test]
    fn deep_merge_lists_by_id_and_objects_keyed_by_id() {
        let a = json!([{ "id": "x", "k": 1 }, { "id": "y" }]);
        let b = json!({ "x": { "k": 2, "n": true }, "z": "v" });
        let m = deep_merge(Some(&a), Some(&b));
        assert_eq!(
            m,
            json!([{ "id": "x", "k": 2, "n": true }, { "id": "y" }, { "id": "z", "value": "v" }])
        );
        let strings = deep_merge(Some(&json!(["a", "b"])), Some(&json!(["b", "c"])));
        assert_eq!(strings, json!(["a", "b", "c"]));
        let obj = deep_merge(
            Some(&json!({ "a": { "b": 1 } })),
            Some(&json!({ "a": { "c": 2 } })),
        );
        assert_eq!(obj, json!({ "a": { "b": 1, "c": 2 } }));
    }

    #[test]
    fn paths_and_stable_string() {
        let mut d = json!({});
        set_path(&mut d, "a.b.c", json!(1));
        assert_eq!(get_path(&d, "a.b.c"), Some(&json!(1)));
        assert_eq!(get_path(&d, "a.x"), None);
        assert_eq!(
            stable_string(&json!({ "b": 1, "a": [2, { "d": 1, "c": 2 }] })),
            r#"{"a":[2,{"c":2,"d":1}],"b":1}"#
        );
    }

    #[test]
    fn glob_and_vars_and_one_line() {
        assert!(glob_to_regex("mcp__slack__*").is_match("mcp__slack__post"));
        assert!(!glob_to_regex("mcp__slack__*").is_match("mcp__slackx__post"));
        std::env::set_var("SAC_TEST_VAR", "ok");
        let v = resolve_vars(
            &json!({ "p": "${env:SAC_TEST_VAR}/${vars.name}" }),
            &json!({ "name": "n" }),
        );
        assert_eq!(v, json!({ "p": "ok/n" }));
        assert_eq!(one_line("  a \n b  ", 10), "a b");
        assert_eq!(one_line("abcdefghij", 5), "abcd…");
    }
}
