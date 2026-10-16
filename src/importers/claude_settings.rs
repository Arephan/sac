//! Importer `claude-settings`: Claude Code user settings (model and effort, permissions, denied
//! MCP servers, hooks, enabled plugins) from `~/.claude/settings.json` and
//! `~/.claude/settings.local.json`. Env var values are dropped; only their names are kept.
//! Ported from the reference `framework/importers/claude-settings.mjs`.

use super::rules::{entries, js_order, js_string, key_list, present, truthy};
use super::Importer;
use crate::ctx::Ctx;
use crate::util::{arr, is_truthy, slug};
use crate::Doc;
use anyhow::Result;
use serde_json::{json, Map, Value};

pub struct ClaudeSettings;

const SETTINGS: &str = "~/.claude/settings.json";
const LOCAL: &str = "~/.claude/settings.local.json";

/// The parsed file when it is a JSON object, else `{}` (unreadable files read as empty).
fn read_object(ctx: &Ctx, rel: &str) -> Doc {
    match ctx.try_json(rel).map(js_order) {
        Some(v @ Value::Object(_)) => v,
        _ => json!({}),
    }
}

/// `[...new Set(lists.flat().filter(Boolean))]`: one level of flattening, falsy items
/// dropped, duplicates removed keeping the first.
fn uniq(lists: &[Option<&Doc>]) -> Vec<Doc> {
    let mut out: Vec<Doc> = Vec::new();
    for list in lists {
        let items: Vec<&Doc> = match truthy(*list) {
            Some(Value::Array(a)) => a.iter().collect(),
            Some(v) => vec![v],
            None => Vec::new(),
        };
        for x in items {
            if is_truthy(Some(x)) && !out.contains(x) {
                out.push(x.clone());
            }
        }
    }
    out
}

/// One entry per hook command, keyed `<event>-<matcher or any>-<last path segment>`.
fn hook_list(ctx: &Ctx, settings: &Doc) -> Vec<Doc> {
    let mut out = Vec::new();
    for (event, groups) in entries(settings.get("hooks")) {
        for group in arr(Some(groups)) {
            let matcher = truthy(group.get("matcher"));
            for hook in arr(group.get("hooks")) {
                let raw = present(hook.get("command"))
                    .or_else(|| present(hook.get("url")))
                    .or_else(|| present(hook.get("type")));
                let raw = raw.map(js_string).unwrap_or_else(|| "undefined".into());
                let command = js_string(&ctx.scrub(&Value::String(raw)));
                let last = command.rsplit('/').next().unwrap_or("");
                let id = format!(
                    "{}-{}-{}",
                    slug(&event, 60),
                    slug(&matcher.map(js_string).unwrap_or_else(|| "any".into()), 24),
                    slug(last, 30)
                );
                out.push(json!({
                    "id": id,
                    "event": event,
                    "matcher": matcher.cloned().unwrap_or_else(|| json!("*")),
                    "command": command,
                }));
            }
        }
    }
    out
}

impl Importer for ClaudeSettings {
    fn id(&self) -> &str {
        "claude-settings"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["brain", "tools"]
    }
    fn description(&self) -> &str {
        "Claude Code settings.json + settings.local.json (permissions, hooks, plugins, model)."
    }
    fn detect(&self, ctx: &Ctx) -> bool {
        ctx.exists(SETTINGS)
    }
    fn run(&self, ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        let s = read_object(ctx, SETTINGS);
        let local = read_object(ctx, LOCAL);
        let empty = json!({});
        let perms = s
            .get("permissions")
            .filter(|p| p.is_object())
            .unwrap_or(&empty);
        let local_perms = local
            .get("permissions")
            .filter(|p| p.is_object())
            .unwrap_or(&empty);

        // Same scrub order as the reference: hooks, then the brain object, then `allow`.
        let hooks = hook_list(ctx, &s);
        let plugins: Vec<Doc> = entries(s.get("enabledPlugins"))
            .into_iter()
            .map(|(id, enabled)| json!({ "id": id, "enabled": is_truthy(Some(enabled)) }))
            .collect();

        // Keys whose source value is absent are left out, as JavaScript drops `undefined`.
        let mut cc = Map::new();
        for (key, value) in [
            ("model", s.get("model")),
            ("effort", s.get("effortLevel")),
            ("per_model", s.get("modelSettings")),
            ("permission_mode", perms.get("defaultMode")),
        ] {
            if let Some(v) = value {
                cc.insert(key.to_string(), v.clone());
            }
        }
        cc.insert("env_names".into(), json!(key_list(s.get("env"))));
        cc.insert(
            "marketplaces".into(),
            json!(key_list(s.get("extraKnownMarketplaces"))),
        );
        let claude_code = ctx.scrub(&Value::Object(cc));

        let mut permissions = Map::new();
        if let Some(mode) = truthy(perms.get("defaultMode")) {
            permissions.insert("default_mode".into(), mode.clone());
        }
        let allow: Vec<Doc> = uniq(&[perms.get("allow"), local_perms.get("allow")])
            .iter()
            .map(|x| ctx.scrub(x))
            .collect();
        permissions.insert("allow".into(), Value::Array(allow));
        permissions.insert(
            "deny".into(),
            Value::Array(uniq(&[perms.get("deny"), local_perms.get("deny")])),
        );
        permissions.insert(
            "ask".into(),
            Value::Array(uniq(&[perms.get("ask"), local_perms.get("ask")])),
        );

        let denied: Vec<Doc> = arr(s.get("deniedMcpServers"))
            .iter()
            .map(|d| {
                truthy(d.get("serverName"))
                    .or_else(|| truthy(d.get("serverUrl")))
                    .cloned()
                    .unwrap_or_else(|| Value::String(js_string(d)))
            })
            .collect();

        Ok(json!({
            "brain": { "claude_code": claude_code },
            "tools": {
                "permissions": permissions,
                "denied_mcp_servers": denied,
                "hooks": hooks,
                "plugins": plugins,
            },
            "_sources": [SETTINGS, LOCAL],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importers::rules::testing::{home, keys};
    use serde_json::json;

    const SETTINGS: &str = r#"{
      "model": "opus",
      "effortLevel": "high",
      "env": { "EXAMPLE_API_KEY": "@SECRET@", "DEBUG_LEVEL": "1" },
      "permissions": {
        "allow": ["Bash(ls)", "Read(@HOME@/notes/*)"],
        "deny": ["mcp__work__*"],
        "ask": ["Bash(git push:*)"],
        "defaultMode": "acceptEdits"
      },
      "deniedMcpServers": [{ "serverName": "work" }, { "serverUrl": "https://mcp.example.com/x" }],
      "enabledPlugins": { "fmt@example-market": true, "old@example-market": false },
      "hooks": {
        "PreToolUse": [
          { "matcher": "Bash", "hooks": [{ "type": "command", "command": "@HOME@/.claude/hooks/guard.sh" }] }
        ],
        "Stop": [{ "hooks": [{ "type": "command", "command": "say done" }] }]
      },
      "extraKnownMarketplaces": { "example-market": { "source": { "source": "github", "repo": "example/market" } } }
    }"#;

    fn secret() -> String {
        format!("xoxb-{}-{}", "1234567890", "abcdefghij")
    }

    #[test]
    fn settings_keep_env_names_only_and_list_hooks_plugins_and_denied_servers() {
        let h = home();
        h.write(
            ".claude/settings.json",
            &SETTINGS.replace("@SECRET@", &secret()),
        );
        h.write(
            ".claude/settings.local.json",
            r#"{ "permissions": { "allow": ["Bash(ls)", "WebFetch(domain:example.com)"], "deny": ["Bash(rm:*)"] } }"#,
        );
        let ctx = h.ctx();
        assert!(ClaudeSettings.detect(&ctx));
        let f = ClaudeSettings.run(&ctx, &json!({})).unwrap();

        assert_eq!(
            f["brain"]["claude_code"],
            json!({
                "model": "opus",
                "effort": "high",
                "permission_mode": "acceptEdits",
                "env_names": ["EXAMPLE_API_KEY", "DEBUG_LEVEL"],
                "marketplaces": ["example-market"]
            })
        );
        assert_eq!(
            f["tools"]["permissions"],
            json!({
                "default_mode": "acceptEdits",
                "allow": ["Bash(ls)", "Read(~/notes/*)", "WebFetch(domain:example.com)"],
                "deny": ["mcp__work__*", "Bash(rm:*)"],
                "ask": ["Bash(git push:*)"]
            })
        );
        assert_eq!(
            f["tools"]["denied_mcp_servers"],
            json!(["work", "https://mcp.example.com/x"])
        );
        assert_eq!(
            f["tools"]["hooks"],
            json!([
                { "id": "pretooluse-bash-guard-sh", "event": "PreToolUse", "matcher": "Bash", "command": "~/.claude/hooks/guard.sh" },
                { "id": "stop-any-say-done", "event": "Stop", "matcher": "*", "command": "say done" }
            ])
        );
        assert_eq!(
            f["tools"]["plugins"],
            json!([
                { "id": "fmt@example-market", "enabled": true },
                { "id": "old@example-market", "enabled": false }
            ])
        );
        assert_eq!(
            f["_sources"],
            json!(["~/.claude/settings.json", "~/.claude/settings.local.json"])
        );
        assert_eq!(keys(&f), ["brain", "tools", "_sources"]);
        assert_eq!(
            keys(&f["tools"]),
            ["permissions", "denied_mcp_servers", "hooks", "plugins"]
        );
        assert_eq!(
            keys(&f["brain"]["claude_code"]),
            [
                "model",
                "effort",
                "permission_mode",
                "env_names",
                "marketplaces"
            ]
        );
        assert_eq!(
            keys(&f["tools"]["permissions"]),
            ["default_mode", "allow", "deny", "ask"]
        );
        assert_eq!(
            keys(&f["tools"]["hooks"][0]),
            ["id", "event", "matcher", "command"]
        );
        let text = f.to_string();
        assert!(!text.contains("xoxb"), "an env value leaked: {text}");
        assert!(!text.contains(&h.s()), "a home path leaked: {text}");
    }

    #[test]
    fn settings_without_permissions_yield_empty_lists() {
        let h = home();
        h.write(".claude/settings.json", r#"{ "model": "sonnet" }"#);
        let f = ClaudeSettings.run(&h.ctx(), &json!({})).unwrap();
        assert_eq!(
            f["tools"]["permissions"],
            json!({ "allow": [], "deny": [], "ask": [] })
        );
        assert_eq!(f["tools"]["denied_mcp_servers"], json!([]));
        assert_eq!(f["tools"]["hooks"], json!([]));
        assert_eq!(f["tools"]["plugins"], json!([]));
        assert_eq!(
            f["brain"]["claude_code"],
            json!({ "model": "sonnet", "env_names": [], "marketplaces": [] })
        );
    }

    #[test]
    fn settings_detect_needs_the_file_and_bad_json_reads_as_empty() {
        let h = home();
        assert!(!ClaudeSettings.detect(&h.ctx()));
        h.write(".claude/settings.json", "{ not json");
        let ctx = h.ctx();
        assert!(ClaudeSettings.detect(&ctx));
        let f = ClaudeSettings.run(&ctx, &json!({})).unwrap();
        assert_eq!(
            f["brain"]["claude_code"],
            json!({ "env_names": [], "marketplaces": [] })
        );
        assert_eq!(f["tools"]["hooks"], json!([]));
    }
}
