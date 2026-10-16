//! YAML in and out, through `serde_json::Value` so the whole tool shares one document type.

use crate::Doc;
use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::{Map, Number, Value};
use std::path::Path;
use std::sync::OnceLock;

fn yaml_to_json(v: serde_yaml::Value) -> Doc {
    match v {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(b) => Value::Bool(b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Number(i.into())
            } else if let Some(u) = n.as_u64() {
                Value::Number(u.into())
            } else {
                n.as_f64()
                    .and_then(Number::from_f64)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
        serde_yaml::Value::String(s) => Value::String(s),
        serde_yaml::Value::Sequence(a) => Value::Array(a.into_iter().map(yaml_to_json).collect()),
        serde_yaml::Value::Mapping(m) => {
            let mut out = Map::new();
            for (k, v) in m {
                let key = match k {
                    serde_yaml::Value::String(s) => s,
                    serde_yaml::Value::Number(n) => n.to_string(),
                    serde_yaml::Value::Bool(b) => b.to_string(),
                    serde_yaml::Value::Null => "null".to_string(),
                    other => serde_yaml::to_string(&other)
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                };
                out.insert(key, yaml_to_json(v));
            }
            Value::Object(out)
        }
        serde_yaml::Value::Tagged(t) => yaml_to_json(t.value),
    }
}

/// Parse YAML text into a document. An empty document is `{}`.
pub fn parse_yaml(text: &str) -> Result<Doc> {
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let v: serde_yaml::Value = serde_yaml::from_str(text).map_err(|e| anyhow!("{e}"))?;
    match yaml_to_json(v) {
        Value::Null => Ok(Value::Object(Map::new())),
        other => Ok(other),
    }
}

/// Read and parse a YAML file.
pub fn read_yaml(path: &Path) -> Result<Doc> {
    let text = std::fs::read_to_string(path).with_context(|| format!("{}", path.display()))?;
    parse_yaml(&text).with_context(|| format!("{}", path.display()))
}

/// Serialize a document as YAML with an optional comment header.
pub fn to_yaml(doc: &Doc, header: &str) -> String {
    let body = serde_yaml::to_string(doc).unwrap_or_else(|_| "{}\n".to_string());
    if header.trim().is_empty() {
        body
    } else {
        format!("{}\n{}", header.trim_end(), body)
    }
}

/// Split `---\n<yaml>\n---\n<body>`; returns `({}, text)` when there is no frontmatter and a
/// best-effort `key: value` map when the frontmatter is not valid YAML.
pub fn parse_frontmatter(text: &str) -> (Doc, String) {
    static FM: OnceLock<Regex> = OnceLock::new();
    let re = FM.get_or_init(|| Regex::new(r"(?s)\A---\n(.*?)\n---\n?(.*)\z").unwrap());
    let Some(c) = re.captures(text) else {
        return (Value::Object(Map::new()), text.to_string());
    };
    let fm_text = &c[1];
    let body = c[2].to_string();
    let fm = match parse_yaml(fm_text) {
        Ok(Value::Object(m)) => Value::Object(m),
        _ => {
            let mut m = Map::new();
            let kv = Regex::new(r"^\s*(\w+):\s*(.*)$").unwrap();
            for line in fm_text.lines() {
                if let Some(c) = kv.captures(line) {
                    let v = c[2].trim().trim_matches('"').to_string();
                    m.insert(c[1].to_string(), Value::String(v));
                }
            }
            Value::Object(m)
        }
    };
    (fm, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn yaml_round_trip_keeps_order_and_types() {
        let d = parse_yaml("version: 1\nmeta:\n  instance: x\nlist:\n  - id: a\n    on: true\n  - id: b\ndate: 2026-01-01\n").unwrap();
        assert_eq!(d["version"], json!(1));
        assert_eq!(d["date"], json!("2026-01-01"));
        assert_eq!(d["list"][0]["on"], json!(true));
        let text = to_yaml(&d, "# header");
        assert!(text.starts_with("# header\nversion: 1\n"));
        assert_eq!(parse_yaml(&text).unwrap(), d);
        assert_eq!(parse_yaml("").unwrap(), json!({}));
    }

    #[test]
    fn frontmatter_parses_and_falls_back() {
        let (fm, body) =
            parse_frontmatter("---\nname: n\nmetadata:\n  type: feedback\n---\nBody here\n");
        assert_eq!(fm["metadata"]["type"], json!("feedback"));
        assert_eq!(body, "Body here\n");
        let (fm2, _) = parse_frontmatter("---\nname: [unclosed\ndescription: \"d\"\n---\nx");
        assert_eq!(fm2["description"], json!("d"));
        let (fm3, body3) = parse_frontmatter("no frontmatter");
        assert_eq!(fm3, json!({}));
        assert_eq!(body3, "no frontmatter");
    }
}
