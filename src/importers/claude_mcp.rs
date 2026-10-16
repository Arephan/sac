//! Importer `claude-mcp`: MCP servers from every place Claude Code reads them, `~/.claude.json`
//! (user and per-project) and `~/.claude/mcp.json`. Only the command/url/args shape and the
//! NAMES of env vars and headers are kept. Ported from `framework/importers/claude-mcp.mjs`.

use super::rules::{entries, js_order, js_string, key_list, static_re, truthy};
use super::Importer;
use crate::ctx::Ctx;
use crate::util::is_truthy;
use crate::Doc;
use anyhow::Result;
use serde_json::{json, Map, Value};

pub struct ClaudeMcp;

const CLAUDE_JSON: &str = "~/.claude.json";
const MCP_JSON: &str = "~/.claude/mcp.json";

fn sorted_names(v: Option<&Doc>) -> Vec<String> {
    let mut names = key_list(v);
    names.sort();
    names
}

/// Append one server. A name already taken by an earlier server gets `@<source>` appended.
fn add(ctx: &Ctx, servers: &mut Vec<Doc>, name: &str, def: &Doc, source: &str) {
    let transport = match def.get("type").and_then(|t| t.as_str()) {
        Some(t @ ("http" | "sse")) => t,
        _ if is_truthy(def.get("url")) => "http",
        _ if is_truthy(def.get("command")) => "stdio",
        _ => "unknown",
    };
    let taken = servers
        .iter()
        .any(|s| s.get("id").and_then(|id| id.as_str()) == Some(name));
    let mut m = Map::new();
    let id = if taken {
        format!("{name}@{source}")
    } else {
        name.to_string()
    };
    m.insert("id".into(), json!(id));
    m.insert("source".into(), json!(source));
    m.insert("transport".into(), json!(transport));
    if let Some(command) = truthy(def.get("command")) {
        m.insert("command".into(), command.clone());
    }
    if let Some(Value::Array(args)) = truthy(def.get("args")) {
        let args: Vec<String> = args.iter().map(js_string).collect();
        m.insert("args".into(), json!(args));
    }
    if let Some(url) = truthy(def.get("url")) {
        let url = js_string(url);
        let url = static_re!(r"\?.*$").replace(&url, "");
        m.insert("url".into(), json!(url));
    }
    m.insert("env_names".into(), json!(sorted_names(def.get("env"))));
    m.insert(
        "header_names".into(),
        json!(sorted_names(def.get("headers"))),
    );
    m.insert("enabled".into(), json!(!is_truthy(def.get("disabled"))));
    servers.push(ctx.scrub(&Value::Object(m)));
}

impl Importer for ClaudeMcp {
    fn id(&self) -> &str {
        "claude-mcp"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["tools"]
    }
    fn description(&self) -> &str {
        "MCP server definitions (user, per-project, mcp.json). Env/header values never kept."
    }
    fn detect(&self, ctx: &Ctx) -> bool {
        ctx.exists(CLAUDE_JSON) || ctx.exists(MCP_JSON)
    }
    fn run(&self, ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        let mut servers: Vec<Doc> = Vec::new();
        if let Some(cj) = ctx.try_json(CLAUDE_JSON).map(js_order) {
            for (name, def) in entries(cj.get("mcpServers")) {
                if !def.is_null() {
                    add(ctx, &mut servers, &name, def, CLAUDE_JSON);
                }
            }
            for (project, p) in entries(cj.get("projects")) {
                let source = format!("{CLAUDE_JSON}#projects/{}", ctx.tilde(&project));
                for (name, def) in entries(p.get("mcpServers")) {
                    if !def.is_null() {
                        add(ctx, &mut servers, &name, def, &source);
                    }
                }
            }
        }
        if let Some(mj) = ctx.try_json(MCP_JSON).map(js_order) {
            // `mcpServers` when present, else the file itself is the name -> server map.
            let list = truthy(mj.get("mcpServers")).unwrap_or(&mj);
            for (name, def) in entries(Some(list)) {
                if def.is_object() || def.is_array() {
                    add(ctx, &mut servers, &name, def, MCP_JSON);
                }
            }
        }
        Ok(json!({
            "tools": { "mcp_servers": servers },
            "_sources": [CLAUDE_JSON, MCP_JSON],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importers::rules::testing::{home, keys};
    use serde_json::json;

    const CLAUDE_JSON: &str = r#"{
      "numStartups": 3,
      "mcpServers": {
        "alpha": { "type": "stdio", "command": "npx", "args": ["-y", "alpha-mcp", 3], "env": { "ALPHA_KEY": "@VALUE@" } },
        "remote": { "type": "http", "url": "https://mcp.example.com/mcp?key=@VALUE@", "headers": { "Authorization": "@VALUE@", "X-Team": "t" } }
      },
      "projects": {
        "@HOME@/code/app": { "mcpServers": { "beta": { "command": "@HOME@/bin/beta", "args": [], "disabled": true } } },
        "@HOME@/code/other": { "allowedTools": [] }
      }
    }"#;

    const MCP_JSON: &str = r#"{
      "mcpServers": {
        "alpha": { "type": "sse", "url": "https://alpha.example.com/sse?session=@VALUE@" },
        "gamma": {}
      }
    }"#;

    #[test]
    fn mcp_servers_from_all_sources_keep_shape_and_names_only() {
        let h = home();
        h.write(
            ".claude.json",
            &CLAUDE_JSON.replace("@VALUE@", "hidden-value-1"),
        );
        h.write(
            ".claude/mcp.json",
            &MCP_JSON.replace("@VALUE@", "hidden-value-2"),
        );
        let ctx = h.ctx();
        assert!(ClaudeMcp.detect(&ctx));
        let f = ClaudeMcp.run(&ctx, &json!({})).unwrap();
        assert_eq!(
            f["tools"]["mcp_servers"],
            json!([
                {
                    "id": "alpha", "source": "~/.claude.json", "transport": "stdio",
                    "command": "npx", "args": ["-y", "alpha-mcp", "3"],
                    "env_names": ["ALPHA_KEY"], "header_names": [], "enabled": true
                },
                {
                    "id": "remote", "source": "~/.claude.json", "transport": "http",
                    "url": "https://mcp.example.com/mcp",
                    "env_names": [], "header_names": ["Authorization", "X-Team"], "enabled": true
                },
                {
                    "id": "beta", "source": "~/.claude.json#projects/~/code/app", "transport": "stdio",
                    "command": "~/bin/beta", "args": [],
                    "env_names": [], "header_names": [], "enabled": false
                },
                {
                    "id": "alpha@~/.claude/mcp.json", "source": "~/.claude/mcp.json", "transport": "sse",
                    "url": "https://alpha.example.com/sse",
                    "env_names": [], "header_names": [], "enabled": true
                },
                {
                    "id": "gamma", "source": "~/.claude/mcp.json", "transport": "unknown",
                    "env_names": [], "header_names": [], "enabled": true
                }
            ])
        );
        assert_eq!(
            f["_sources"],
            json!(["~/.claude.json", "~/.claude/mcp.json"])
        );
        let servers = &f["tools"]["mcp_servers"];
        assert_eq!(
            keys(&servers[0]),
            [
                "id",
                "source",
                "transport",
                "command",
                "args",
                "env_names",
                "header_names",
                "enabled"
            ]
        );
        assert_eq!(
            keys(&servers[1]),
            [
                "id",
                "source",
                "transport",
                "url",
                "env_names",
                "header_names",
                "enabled"
            ]
        );
        let text = f.to_string();
        assert!(!text.contains("hidden-value"), "a value leaked: {text}");
        assert!(!text.contains(&h.s()), "a home path leaked: {text}");
    }

    #[test]
    fn mcp_json_without_wrapper_and_detect() {
        let h = home();
        assert!(!ClaudeMcp.detect(&h.ctx()));
        h.write(
            ".claude/mcp.json",
            r#"{ "delta": { "url": "https://d.example.com/" }, "note": "text", "list": [1] }"#,
        );
        let ctx = h.ctx();
        assert!(ClaudeMcp.detect(&ctx));
        let f = ClaudeMcp.run(&ctx, &json!({})).unwrap();
        let ids: Vec<&str> = f["tools"]["mcp_servers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["delta", "list"]);
        assert_eq!(f["tools"]["mcp_servers"][0]["transport"], json!("http"));
        assert_eq!(f["tools"]["mcp_servers"][1]["transport"], json!("unknown"));
    }

    #[test]
    fn third_server_with_a_taken_name_also_gets_its_source_suffix() {
        let h = home();
        h.write(
            ".claude.json",
            r#"{ "mcpServers": { "x": { "command": "a" } },
                 "projects": { "/p1": { "mcpServers": { "x": { "command": "b" } } },
                               "/p2": { "mcpServers": { "x": { "command": "c" } } } } }"#,
        );
        let f = ClaudeMcp.run(&h.ctx(), &json!({})).unwrap();
        let ids: Vec<&str> = f["tools"]["mcp_servers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            [
                "x",
                "x@~/.claude.json#projects//p1",
                "x@~/.claude.json#projects//p2"
            ]
        );
    }
}
