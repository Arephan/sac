//! Importer `claude-memory`: Claude Code auto-memory, one markdown file per fact with
//! frontmatter (`name`, `description`, `metadata.type` = user|feedback|project|reference).
//!
//! - feedback  -> `instructions.rules` (uppercase HARD in the description = hard)
//! - project   -> `projects`
//! - reference -> `data.references`
//! - user      -> `identity.person.profile_refs` (file paths only, contents never copied)
//!
//! The memory folder is found from the home directory, so nothing about the person is
//! hardcoded. Ported from `framework/importers/claude-memory.mjs`.

use super::rules::{
    classify_scope, first_date, js_order, js_p, js_shown, js_slice, js_string, severity_of,
    static_re, truthy,
};
use super::Importer;
use crate::ctx::Ctx;
use crate::util::{claude_project_slug, one_line};
use crate::yaml::parse_yaml;
use crate::Doc;
use anyhow::Result;
use serde_json::{json, Map, Value};

pub struct ClaudeMemory;

fn default_dir(ctx: &Ctx) -> String {
    format!(
        "~/.claude/projects/{}/memory",
        claude_project_slug(&ctx.home().to_string_lossy())
    )
}

/// Split frontmatter from body. Frontmatter that is not valid YAML is read line by line as
/// `key: value` pairs (one pair of surrounding double quotes removed), as in the reference.
fn parse(text: &str) -> (Doc, String) {
    let Some(c) = static_re!(r"(?s)\A---\n(.*?)\n---\n?(.*)\z").captures(text) else {
        return (json!({}), text.to_string());
    };
    let fm = match parse_yaml(&c[1]) {
        Ok(v) if v.is_object() => v,
        Ok(_) => json!({}),
        Err(_) => {
            let mut m = Map::new();
            // JavaScript's `.` stops at \r, \u2028 and \u2029 as well as \n.
            let kv = static_re!(r"\A\s*([A-Za-z0-9_]+):\s*([^\r\n\u{2028}\u{2029}]*)\z");
            for line in c[1].split('\n') {
                if let Some(p) = kv.captures(line) {
                    let v = &p[2];
                    let v = v.strip_prefix('"').unwrap_or(v);
                    let v = v.strip_suffix('"').unwrap_or(v);
                    m.insert(p[1].to_string(), json!(v));
                }
            }
            Value::Object(m)
        }
    };
    (fm, c[2].to_string())
}

/// Project status from markers in the description.
fn status_of(desc: &str) -> &'static str {
    let d = desc.to_uppercase();
    if static_re!(r"KILLED|DEAD(?-u:\b)|NEVER REBUILD").is_match(&d) {
        "killed"
    } else if d.contains("SUPERSEDED") {
        "superseded"
    } else if static_re!(r"HALT|FROZEN|PAUSED|STOPPED").is_match(&d) {
        "halted"
    } else if static_re!(r"BLOCKED|WALL(?-u:\b)|PENDING").is_match(&d) {
        "blocked"
    } else if static_re!(r"(?-u:\b)LIVE(?-u:\b)|SHIPPED|PUBLIC(?-u:\b)").is_match(&d) {
        "live"
    } else {
        "active"
    }
}

impl Importer for ClaudeMemory {
    fn id(&self) -> &str {
        "claude-memory"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["instructions", "projects", "data", "identity"]
    }
    fn description(&self) -> &str {
        "Auto-memory files: feedback=rules, project=projects, reference=references."
    }
    /// Only the default folder is probed, even when `dir` is set (as in the reference).
    fn detect(&self, ctx: &Ctx) -> bool {
        ctx.is_dir(&default_dir(ctx))
    }
    fn run(&self, ctx: &Ctx, opts: &Doc) -> Result<Doc> {
        let dir = truthy(opts.get("dir"))
            .map(js_string)
            .unwrap_or_else(|| default_dir(ctx));
        // Files are read through the expanded, normalized path, as Node does.
        let dir_abs = js_p(ctx, &dir);
        let files: Vec<String> = ctx
            .list(&dir_abs)
            .into_iter()
            .filter(|f| f.ends_with(".md") && f != "MEMORY.md" && f != "REFERENCE.md")
            .collect();
        let scope_keywords = opts.get("scope_keywords");
        let mut rules = Vec::new();
        let mut projects = Vec::new();
        let mut references = Vec::new();
        let mut profile = Vec::new();
        let mut by_type = Map::new();
        for f in &files {
            let path = format!("{dir}/{f}");
            let abs = js_p(ctx, &path);
            let text = match ctx.read_text(&abs) {
                Ok(t) => t,
                Err(e) => {
                    ctx.warn(format!("{e:#}"));
                    continue;
                }
            };
            let (fm, body) = parse(&text);
            let meta = fm.get("metadata");
            let kind = truthy(meta.and_then(|m| m.get("type")))
                .or_else(|| truthy(fm.get("type")))
                .cloned()
                .unwrap_or_else(|| json!("unknown"));
            let n = by_type.entry(js_string(&kind)).or_insert(json!(0));
            *n = json!(n.as_u64().unwrap_or(0) + 1);

            let stem = f.strip_suffix(".md").unwrap_or(f);
            let id = truthy(fm.get("name"))
                .map(js_string)
                .unwrap_or_else(|| stem.to_string());
            let desc = truthy(fm.get("description"))
                .map(js_string)
                .unwrap_or_default();
            let src = js_shown(ctx, &path);
            match kind.as_str() {
                Some("feedback") => {
                    let modified = truthy(meta.and_then(|m| m.get("modified")))
                        .map(js_string)
                        .unwrap_or_default();
                    let mdate = js_slice(&modified, 10);
                    let date = first_date(&[&desc, js_slice(&body, 400)]).or_else(|| {
                        if static_re!(r"\A[0-9]{4}-[0-9]{2}-[0-9]{2}\z").is_match(mdate) {
                            Some(mdate.to_string())
                        } else {
                            ctx.mtime_date(&abs)
                        }
                    });
                    let why = static_re!(r"(?i)\*\*Why:?\*\*:?\s*([^\n]+)")
                        .captures(&body)
                        .map(|c| c[1].to_string());
                    let superseded = desc.to_ascii_uppercase().contains("SUPERSEDED");
                    let scope_text = format!("{desc} {}", id.replace(['_', '-'], " "));
                    rules.push(json!({
                        "id": format!("mem-{id}"),
                        "scope": classify_scope(&scope_text, scope_keywords),
                        "severity": severity_of(&desc),
                        "date": date,
                        "reason": one_line(why.as_deref().unwrap_or(&desc), 240),
                        "statement": one_line(&desc, 280),
                        "source": src,
                        "status": if superseded { "superseded" } else { "active" },
                    }));
                }
                Some("project") => {
                    let mut p = Map::new();
                    p.insert("id".into(), json!(id));
                    p.insert("description".into(), json!(one_line(&desc, 240)));
                    p.insert("status".into(), json!(status_of(&desc)));
                    if let Some(m) =
                        static_re!(r"~/[A-Za-z0-9_.@-]+(?:/[A-Za-z0-9_.@-]+)*").find(&desc)
                    {
                        p.insert("path".into(), json!(m.as_str()));
                    }
                    p.insert("source".into(), json!(src));
                    projects.push(Value::Object(p));
                }
                Some("reference") => references.push(json!({
                    "id": id,
                    "description": one_line(&desc, 200),
                    "source": src,
                })),
                Some("user") => profile.push(src),
                _ => {}
            }
        }
        let shown = js_shown(ctx, &dir);
        let index = js_shown(ctx, &format!("{dir}/MEMORY.md"));
        let identity = if profile.is_empty() {
            json!({})
        } else {
            json!({ "person": { "profile_refs": profile } })
        };
        Ok(json!({
            "instructions": {
                "sources": [{ "id": "memory", "path": shown, "kind": "memory" }],
                "rules": rules,
            },
            "projects": projects,
            "data": {
                "memory": {
                    "dir": shown,
                    "index": index,
                    "files": files.len(),
                    "by_type": js_order(Value::Object(by_type)),
                },
                "references": references,
            },
            "identity": identity,
            "_sources": [dir],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importers::rules::testing::{home, keys, Home};
    use crate::util::claude_project_slug;
    use serde_json::json;

    /// `(home-relative memory dir, the same dir as it appears in a manifest)`.
    fn memory_dir(h: &Home) -> (String, String) {
        let rel = format!(".claude/projects/{}/memory", claude_project_slug(&h.s()));
        let shown = format!("~/{rel}");
        (rel, shown)
    }

    #[test]
    fn memory_files_become_rules_projects_references_and_profile_refs() {
        let h = home();
        let (rel, shown) = memory_dir(&h);
        h.write(&format!("{rel}/MEMORY.md"), "# Memory Index\n- [x](x.md)\n");
        h.write(
            &format!("{rel}/feedback_money.md"),
            "---\nname: no-spend\ndescription: HARD never spend money (2026-08-29)\nmetadata:\n  type: feedback\n---\nNever buy anything.\n\n**Why:** the owner said so.\n**How to apply:** stop at checkout.\n",
        );
        h.write(
            &format!("{rel}/project_app.md"),
            "---\ndescription: Example app at ~/code/example-app, SHIPPED to the store\ntype: project\n---\nNotes.\n",
        );
        h.write(
            &format!("{rel}/reference_docs.md"),
            "---\nname: docs\ndescription: Docs live at example.com\nmetadata:\n  type: reference\n---\n",
        );
        h.write(
            &format!("{rel}/user_alex.md"),
            "---\nname: alex\ndescription: who the user is\nmetadata:\n  type: user\n---\nPrivate profile text.\n",
        );
        h.write(
            &format!("{rel}/broken.md"),
            "---\nname: [broken\ndescription: oops\n---\nbody\n",
        );
        h.write(&format!("{rel}/notes.txt"), "not memory\n");
        let ctx = h.ctx();
        assert!(ClaudeMemory.detect(&ctx));
        let f = ClaudeMemory.run(&ctx, &json!({})).unwrap();

        assert_eq!(
            f["instructions"]["sources"],
            json!([{ "id": "memory", "path": shown, "kind": "memory" }])
        );
        assert_eq!(
            f["instructions"]["rules"],
            json!([{
                "id": "mem-no-spend",
                "scope": "money",
                "severity": "hard",
                "date": "2026-08-29",
                "reason": "the owner said so.",
                "statement": "HARD never spend money (2026-08-29)",
                "source": format!("{shown}/feedback_money.md"),
                "status": "active"
            }])
        );
        assert_eq!(
            f["projects"],
            json!([{
                "id": "project_app",
                "description": "Example app at ~/code/example-app, SHIPPED to the store",
                "status": "live",
                "path": "~/code/example-app",
                "source": format!("{shown}/project_app.md")
            }])
        );
        assert_eq!(
            f["data"]["references"],
            json!([{ "id": "docs", "description": "Docs live at example.com", "source": format!("{shown}/reference_docs.md") }])
        );
        assert_eq!(
            f["identity"],
            json!({ "person": { "profile_refs": [format!("{shown}/user_alex.md")] } })
        );
        assert_eq!(
            f["data"]["memory"],
            json!({
                "dir": shown,
                "index": format!("{shown}/MEMORY.md"),
                "files": 5,
                "by_type": { "unknown": 1, "feedback": 1, "project": 1, "reference": 1, "user": 1 }
            })
        );
        assert_eq!(f["_sources"], json!([format!("~/{rel}")]));
        assert_eq!(
            keys(&f),
            ["instructions", "projects", "data", "identity", "_sources"]
        );
        assert_eq!(
            keys(&f["instructions"]["rules"][0]),
            [
                "id",
                "scope",
                "severity",
                "date",
                "reason",
                "statement",
                "source",
                "status"
            ]
        );
        assert_eq!(
            keys(&f["projects"][0]),
            ["id", "description", "status", "path", "source"]
        );
        assert_eq!(
            keys(&f["data"]["memory"]["by_type"]),
            ["unknown", "feedback", "project", "reference", "user"]
        );
        let text = f.to_string();
        assert!(!text.contains("Private profile text"));
        assert!(!text.contains(&h.s()), "a home path leaked: {text}");
    }

    #[test]
    fn feedback_without_why_uses_description_and_dates_fall_back() {
        let h = home();
        let (rel, _) = memory_dir(&h);
        h.write(
            &format!("{rel}/a_old.md"),
            "---\nname: old-way\ndescription: use the old way, superseded later\nmetadata:\n  type: feedback\n  modified: 2025-03-04T10:00:00Z\n---\nbody\n",
        );
        h.write(
            &format!("{rel}/b_plain.md"),
            "---\nname: plain\ndescription: keep it short\nmetadata:\n  type: feedback\n---\n",
        );
        let ctx = h.ctx();
        let mtime = ctx.mtime_date(&format!("~/{rel}/b_plain.md")).unwrap();
        let f = ClaudeMemory.run(&ctx, &json!({})).unwrap();
        let rules = f["instructions"]["rules"].as_array().unwrap();
        assert_eq!(rules[0]["id"], json!("mem-old-way"));
        assert_eq!(
            rules[0]["reason"],
            json!("use the old way, superseded later")
        );
        assert_eq!(rules[0]["status"], json!("superseded"));
        assert_eq!(rules[0]["severity"], json!("standing"));
        assert_eq!(rules[0]["date"], json!("2025-03-04"));
        assert_eq!(rules[1]["date"], json!(mtime));
        assert_eq!(f["identity"], json!({}));
        assert_eq!(f["projects"], json!([]));
    }

    #[test]
    fn secret_looking_files_are_skipped_with_a_warning_but_counted() {
        let h = home();
        let (rel, _) = memory_dir(&h);
        h.write(
            &format!("{rel}/project_token_audit.md"),
            "---\nname: audit\nmetadata:\n  type: project\n---\n",
        );
        let ctx = h.ctx();
        let f = ClaudeMemory.run(&ctx, &json!({})).unwrap();
        assert_eq!(f["data"]["memory"]["files"], json!(1));
        assert_eq!(f["data"]["memory"]["by_type"], json!({}));
        assert_eq!(f["projects"], json!([]));
        let w = ctx.warnings();
        assert_eq!(w.len(), 1);
        assert!(
            w[0].starts_with("refused to read secret-looking file"),
            "{w:?}"
        );
    }

    #[test]
    fn memory_dir_option_and_detect() {
        let h = home();
        assert!(!ClaudeMemory.detect(&h.ctx()));
        h.write(
            "notes/memory/project_x.md",
            "---\nname: x\ndescription: halted for now\nmetadata:\n  type: project\n---\n",
        );
        let f = ClaudeMemory
            .run(&h.ctx(), &json!({ "dir": "~/notes/memory" }))
            .unwrap();
        assert_eq!(f["projects"][0]["id"], json!("x"));
        assert_eq!(f["projects"][0]["status"], json!("halted"));
        assert_eq!(f["data"]["memory"]["dir"], json!("~/notes/memory"));
        assert_eq!(f["_sources"], json!(["~/notes/memory"]));
    }
}
