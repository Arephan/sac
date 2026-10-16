//! Importer `claude-plugins`: installed Claude Code plugins from
//! `~/.claude/plugins/installed_plugins.json`. Enabled flags come from `claude-settings`.
//! Ported from `framework/importers/claude-plugins.mjs`.

use super::rules::{entries, js_order, js_string, truthy};
use super::Importer;
use crate::ctx::Ctx;
use crate::Doc;
use anyhow::Result;
use serde_json::{json, Map, Value};

pub struct ClaudePlugins;

const INSTALLED: &str = "~/.claude/plugins/installed_plugins.json";

impl Importer for ClaudePlugins {
    fn id(&self) -> &str {
        "claude-plugins"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["tools"]
    }
    fn description(&self) -> &str {
        "Installed plugins and marketplaces under ~/.claude/plugins."
    }
    fn detect(&self, ctx: &Ctx) -> bool {
        ctx.exists(INSTALLED)
    }
    fn run(&self, ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        let j = ctx
            .try_json(INSTALLED)
            .map(js_order)
            .unwrap_or_else(|| json!({}));
        // Each value is a list of installs (newer format) or one install (older format); the
        // first install's version is the one reported.
        let plugins: Vec<Doc> = entries(j.get("plugins"))
            .into_iter()
            .map(|(id, v)| {
                let first = match v {
                    Value::Array(a) => a.first(),
                    other => Some(other),
                };
                let mut m = Map::new();
                m.insert("id".into(), json!(id));
                m.insert("installed".into(), json!(true));
                if let Some(version) = truthy(first.and_then(|f| f.get("version"))) {
                    m.insert("version".into(), json!(js_string(version)));
                }
                Value::Object(m)
            })
            .collect();
        Ok(json!({ "tools": { "plugins": plugins }, "_sources": [INSTALLED] }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importers::rules::testing::{home, keys};
    use serde_json::json;

    #[test]
    fn plugins_come_from_installed_plugins_json_with_version() {
        let h = home();
        h.write(
            ".claude/plugins/installed_plugins.json",
            r#"{
              "version": 2,
              "plugins": {
                "fmt@example-market": [
                  { "scope": "user", "version": "1.2.0", "installPath": "@HOME@/.claude/plugins/cache/fmt" },
                  { "scope": "project", "version": "0.9.0" }
                ],
                "lint@example-market": [{ "installPath": "@HOME@/.claude/plugins/cache/lint" }],
                "old@example-market": { "version": 3 },
                "none@example-market": []
              }
            }"#,
        );
        let ctx = h.ctx();
        assert!(ClaudePlugins.detect(&ctx));
        let f = ClaudePlugins.run(&ctx, &json!({})).unwrap();
        assert_eq!(
            f["tools"]["plugins"],
            json!([
                { "id": "fmt@example-market", "installed": true, "version": "1.2.0" },
                { "id": "lint@example-market", "installed": true },
                { "id": "old@example-market", "installed": true, "version": "3" },
                { "id": "none@example-market", "installed": true }
            ])
        );
        assert_eq!(
            f["_sources"],
            json!(["~/.claude/plugins/installed_plugins.json"])
        );
        assert_eq!(
            keys(&f["tools"]["plugins"][0]),
            ["id", "installed", "version"]
        );
    }

    #[test]
    fn plugins_detect_and_unreadable_file() {
        let h = home();
        assert!(!ClaudePlugins.detect(&h.ctx()));
        h.write(".claude/plugins/installed_plugins.json", "[oops");
        let f = ClaudePlugins.run(&h.ctx(), &json!({})).unwrap();
        assert_eq!(f["tools"]["plugins"], json!([]));
    }
}
