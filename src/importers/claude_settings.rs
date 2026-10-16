//! Importer `claude-settings`: Claude Code user settings (model and effort, permissions, denied
//! MCP servers, hooks, enabled plugins) from `~/.claude/settings.json` and
//! `~/.claude/settings.local.json`. Env var values are dropped; only their names are kept.
//! Ported from the reference `framework/importers/claude-settings.mjs`.

use super::rules::{entries, js_order, js_string, key_list, present, truthy};
use super::Importer;
use crate::ctx::Ctx;
use crate::secrets::find_secrets;
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

/// Claude Code writes an absolute path in a permission rule with a leading `//`
/// (`Read(//<home>/.ssh/**)`); `~/` is relative to the home and a single leading `/` to the
/// settings file. `//<home>/x` becomes `~/x` and a bare `//<home>` becomes `~` here, before the
/// generic scrub, which would otherwise leave `/~/x`: a different path. A match must not follow
/// another `/` (three slashes are not the documented form), and a bare home must be followed by
/// the end of the text, whitespace, a quote or `)`, so a sibling such as `<home>-old` is left
/// alone. Paths outside the home keep their `//` form.
fn home_rule_paths(rule: &str, home: &str) -> String {
    if home.len() <= 1 {
        return rule.to_string();
    }
    let needle = format!("/{home}");
    let mut out = String::with_capacity(rule.len());
    let (mut copied, mut from) = (0, 0);
    while let Some(offset) = rule[from..].find(&needle) {
        let start = from + offset;
        let end = start + needle.len();
        // `map_or(true, ..)` rather than `is_none_or`, which is newer than the crate's MSRV
        let bounded = rule[end..].chars().next().map_or(true, |c| {
            matches!(c, '/' | ')' | '"' | '\'') || c.is_whitespace()
        });
        if bounded && !rule[..start].ends_with('/') {
            out.push_str(&rule[copied..start]);
            out.push('~');
            copied = end;
            from = end;
        } else {
            from = start + 1;
        }
    }
    out.push_str(&rule[copied..]);
    out
}

/// Claude Code matches a `Bash(...)` rule against the command text as typed and does not
/// expand `~` inside it, so a home path in it must stay exactly as written: no `~` rewrite of
/// any kind. A secret-shaped value is still dropped, with the warning `Ctx::scrub` gives.
fn scrub_bash_rule(ctx: &Ctx, rule: &str) -> Doc {
    match find_secrets(rule).first() {
        Some(hit) => {
            ctx.warn(format!("dropped a secret-looking value ({})", hit.id));
            json!("<redacted>")
        }
        None => json!(rule),
    }
}

/// One entry per hook command, keyed `<event>-<matcher or any>-<last path segment>`.
fn hook_list(ctx: &Ctx, settings: &Doc) -> Vec<Doc> {
    let mut out = Vec::new();
    for (event, groups) in entries(settings.get("hooks")) {
        for group in arr(Some(groups)) {
            let matcher = truthy(group.get("matcher"));
            // A null hook is skipped (the reference crashes on it).
            for hook in arr(group.get("hooks")).iter().filter(|h| !h.is_null()) {
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
        // All three lists are scrubbed. The reference scrubs only `allow`; scrubbing `deny`
        // and `ask` too keeps home paths and secret-shaped values out of the manifest. Rules in
        // Claude Code's `//<home>` form are rewritten to `~` first so they keep their meaning,
        // and `Bash(...)` rules keep their home paths as written (only secrets are dropped).
        let home = ctx.home().to_string_lossy().to_string();
        for key in ["allow", "deny", "ask"] {
            let list: Vec<Doc> = uniq(&[perms.get(key), local_perms.get(key)])
                .iter()
                .map(|x| match x {
                    Value::String(rule) if rule.starts_with("Bash(") => scrub_bash_rule(ctx, rule),
                    Value::String(rule) => ctx.scrub(&Value::String(home_rule_paths(rule, &home))),
                    other => ctx.scrub(other),
                })
                .collect();
            permissions.insert(key.into(), Value::Array(list));
        }

        // A null entry is skipped (the reference crashes on it).
        let denied: Vec<Doc> = arr(s.get("deniedMcpServers"))
            .iter()
            .filter(|d| !d.is_null())
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
    use crate::importers::rules::testing::{home, keys, DROPPED};
    use serde_json::json;

    const SETTINGS: &str = r#"{
      "model": "opus",
      "effortLevel": "high",
      "env": { "EXAMPLE_API_KEY": "@TOKEN@", "DEBUG_LEVEL": "1" },
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

    #[test]
    fn settings_keep_env_names_only_and_list_hooks_plugins_and_denied_servers() {
        let h = home();
        h.write(".claude/settings.json", SETTINGS);
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
    fn settings_scrub_permission_lists_and_hook_commands() {
        let h = home();
        h.write(
            ".claude/settings.json",
            r#"{
              "permissions": {
                "allow": ["Read(@HOME@/notes/*)", "Bash(curl -H @TOKEN@)"],
                "deny": ["Read(@HOME@/private/**)", "Bash(echo @TOKEN@)"],
                "ask": ["Edit(@HOME@/notes/todo.md)", "WebFetch(@TOKEN@)"]
              },
              "hooks": {
                "Stop": [{ "hooks": [
                  { "type": "command", "command": "@HOME@/bin/notify --key @TOKEN@" },
                  { "type": "command", "command": "@HOME@/bin/done.sh" }
                ] }]
              }
            }"#,
        );
        let ctx = h.ctx();
        let f = ClaudeSettings.run(&ctx, &json!({})).unwrap();
        assert_eq!(
            f["tools"]["permissions"],
            json!({
                "allow": ["Read(~/notes/*)", "<redacted>"],
                "deny": ["Read(~/private/**)", "<redacted>"],
                "ask": ["Edit(~/notes/todo.md)", "<redacted>"]
            })
        );
        assert_eq!(
            f["tools"]["hooks"],
            json!([
                { "id": "stop-any-redacted", "event": "Stop", "matcher": "*", "command": "<redacted>" },
                { "id": "stop-any-done-sh", "event": "Stop", "matcher": "*", "command": "~/bin/done.sh" }
            ])
        );
        // one warning per dropped value: the hook command, then allow, deny and ask
        assert_eq!(ctx.warnings(), [DROPPED; 4]);
        let text = f.to_string();
        assert!(!text.contains("xoxb"), "a token leaked: {text}");
        assert!(!text.contains(&h.s()), "a home path leaked: {text}");
    }

    #[test]
    fn settings_double_slash_home_rules_become_tilde_paths() {
        let h = home();
        // `@HOME@` is the absolute home, so `/@HOME@` is Claude Code's `//<home>` form.
        h.write(
            ".claude/settings.json",
            r#"{
              "permissions": {
                "allow": ["Read(/@HOME@/notes/**)", "Edit(/@HOME@)"],
                "deny": ["Read(/@HOME@/.ssh/**)", "Read(/@HOME@/.config/app/*.json)"],
                "ask": ["Edit(/@HOME@/.zshrc)", "Write(/@HOME@/notes/todo.md)"]
              }
            }"#,
        );
        let ctx = h.ctx();
        let f = ClaudeSettings.run(&ctx, &json!({})).unwrap();
        assert_eq!(
            f["tools"]["permissions"],
            json!({
                "allow": ["Read(~/notes/**)", "Edit(~)"],
                "deny": ["Read(~/.ssh/**)", "Read(~/.config/app/*.json)"],
                "ask": ["Edit(~/.zshrc)", "Write(~/notes/todo.md)"]
            })
        );
        assert!(ctx.warnings().is_empty());
    }

    #[test]
    fn settings_bash_rules_keep_absolute_home_paths() {
        let h = home();
        // `@HOME@` is the absolute home: `@HOME@/x` is a plain absolute path and `/@HOME@/x`
        // is Claude Code's `//` form.
        h.write(
            ".claude/settings.json",
            r#"{
              "permissions": {
                "allow": ["Bash(node @HOME@/x/run.mjs)", "Read(/@HOME@/x/**)"],
                "deny": ["Bash(node @HOME@/x/deploy.mjs:*)", "Read(/@HOME@/x/**)"],
                "ask": ["Bash(cat /@HOME@/x/notes.md)", "Bash(curl -H @TOKEN@ @HOME@/x)"]
              }
            }"#,
        );
        let ctx = h.ctx();
        let f = ClaudeSettings.run(&ctx, &json!({})).unwrap();
        let home = h.s();
        // Bash rules match the command text as typed and `~` is not expanded in them, so the
        // home path stays exactly as written (`//` included); other rules still get `~/`.
        assert_eq!(
            f["tools"]["permissions"],
            json!({
                "allow": [format!("Bash(node {home}/x/run.mjs)"), "Read(~/x/**)"],
                "deny": [format!("Bash(node {home}/x/deploy.mjs:*)"), "Read(~/x/**)"],
                "ask": [format!("Bash(cat /{home}/x/notes.md)"), "<redacted>"]
            })
        );
        // a secret-shaped Bash rule is still dropped, with the usual warning
        assert_eq!(ctx.warnings(), [DROPPED]);
    }

    #[test]
    fn settings_double_slash_rules_outside_the_home_stay_untouched() {
        let h = home();
        h.write(
            ".claude/settings.json",
            r#"{
              "permissions": {
                "allow": ["Read(//etc/hosts)", "Read(/@HOME@-old/**)"],
                "deny": ["Read(//opt/shared/**)", "Read(/@HOME@x)"],
                "ask": ["Edit(//tmp/scratch/*)"]
              }
            }"#,
        );
        let f = ClaudeSettings.run(&h.ctx(), &json!({})).unwrap();
        let home = h.s();
        // a sibling of the home (`<home>-old`, `<home>x`) is not the home and keeps its path
        assert_eq!(
            f["tools"]["permissions"],
            json!({
                "allow": ["Read(//etc/hosts)", format!("Read(/{home}-old/**)")],
                "deny": ["Read(//opt/shared/**)", format!("Read(/{home}x)")],
                "ask": ["Edit(//tmp/scratch/*)"]
            })
        );
    }

    #[test]
    fn settings_skip_null_entries_and_keep_plain_string_denied_servers() {
        let h = home();
        h.write(
            ".claude/settings.json",
            r#"{
              "deniedMcpServers": [
                null, "plain-server", { "serverName": "work" }, { "serverUrl": "https://mcp.example.com/x" }, 7
              ],
              "hooks": {
                "PreToolUse": [
                  null,
                  { "matcher": "Bash", "hooks": [null, { "type": "command", "command": "guard" }, "loose"] }
                ]
              }
            }"#,
        );
        let ctx = h.ctx();
        let f = ClaudeSettings.run(&ctx, &json!({})).unwrap();
        assert_eq!(
            f["tools"]["denied_mcp_servers"],
            json!(["plain-server", "work", "https://mcp.example.com/x", "7"])
        );
        // null groups and null hooks are skipped; a non-null entry that is not an object
        // still becomes a hook with command `undefined`, as in the reference
        assert_eq!(
            f["tools"]["hooks"],
            json!([
                { "id": "pretooluse-bash-guard", "event": "PreToolUse", "matcher": "Bash", "command": "guard" },
                { "id": "pretooluse-bash-undefined", "event": "PreToolUse", "matcher": "Bash", "command": "undefined" }
            ])
        );
        assert!(ctx.warnings().is_empty());
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
