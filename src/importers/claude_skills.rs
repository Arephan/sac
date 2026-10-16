//! Importer `claude-skills`: user skills (`~/.claude/skills/<name>/SKILL.md`) and slash
//! commands (`~/.claude/commands/*.md`). Ported from `framework/importers/claude-skills.mjs`.

use super::rules::{js_shown, js_slice, js_string, static_re, truthy};
use super::Importer;
use crate::ctx::Ctx;
use crate::yaml::parse_yaml;
use crate::Doc;
use anyhow::Result;
use serde_json::json;

pub struct ClaudeSkills;

/// The YAML frontmatter of a skill file. Unlike memory files there is no line-by-line
/// fallback: frontmatter that is not valid YAML reads as `{}`, as in the reference.
fn frontmatter(text: &str) -> Doc {
    let Some(c) = static_re!(r"(?s)\A---\n(.*?)\n---").captures(text) else {
        return json!({});
    };
    match parse_yaml(&c[1]) {
        Ok(v) if v.is_object() => v,
        _ => json!({}),
    }
}

impl Importer for ClaudeSkills {
    fn id(&self) -> &str {
        "claude-skills"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["tools"]
    }
    fn description(&self) -> &str {
        "User skills and slash commands."
    }
    fn detect(&self, ctx: &Ctx) -> bool {
        ctx.is_dir("~/.claude/skills") || ctx.is_dir("~/.claude/commands")
    }
    fn run(&self, ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        let mut skills = Vec::new();
        for name in ctx.list("~/.claude/skills") {
            let file = format!("~/.claude/skills/{name}/SKILL.md");
            if !ctx.exists(&file) {
                continue;
            }
            // A refused or unreadable SKILL.md fails the whole importer, as in the reference.
            let fm = frontmatter(&ctx.read_text(&file)?);
            let raw = truthy(fm.get("description"))
                .map(js_string)
                .unwrap_or_default();
            let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            skills.push(json!({
                "id": name,
                "source": js_shown(ctx, &file),
                "description": js_slice(&collapsed, 200),
            }));
        }
        let commands: Vec<Doc> = ctx
            .list("~/.claude/commands")
            .into_iter()
            .filter_map(|f| {
                let id = f.strip_suffix(".md")?.to_string();
                Some(json!({ "id": id, "source": format!("~/.claude/commands/{f}") }))
            })
            .collect();
        Ok(json!({
            "tools": { "skills": skills, "commands": commands },
            "_sources": ["~/.claude/skills", "~/.claude/commands"],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importers::rules::testing::home;
    use serde_json::json;

    #[test]
    fn skills_from_frontmatter_and_commands_from_markdown_files() {
        let h = home();
        h.write(
            ".claude/skills/alpha/SKILL.md",
            "---\nname: alpha\ndescription: |\n  Does   many\n  things.\n---\n# Alpha\n",
        );
        let long = format!("{} tail", "word ".repeat(60));
        h.write(
            ".claude/skills/long/SKILL.md",
            &format!("---\nname: long\ndescription: \"{long}\"\n---\nbody\n"),
        );
        h.write(
            ".claude/skills/broken/SKILL.md",
            "---\nname: broken\ndescription: [oops\n---\nbody\n",
        );
        h.write(".claude/skills/plain/SKILL.md", "no frontmatter at all\n");
        h.mkdir(".claude/skills/empty-dir");
        h.write(".claude/commands/deploy.md", "Deploy it.\n");
        h.write(".claude/commands/review.md", "Review it.\n");
        h.write(".claude/commands/notes.txt", "not a command\n");
        let ctx = h.ctx();
        assert!(ClaudeSkills.detect(&ctx));
        let f = ClaudeSkills.run(&ctx, &json!({})).unwrap();

        let expected_long: String = long
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(200)
            .collect();
        assert_eq!(expected_long.chars().count(), 200);
        assert_eq!(
            f["tools"]["skills"],
            json!([
                { "id": "alpha", "source": "~/.claude/skills/alpha/SKILL.md", "description": "Does many things." },
                { "id": "broken", "source": "~/.claude/skills/broken/SKILL.md", "description": "" },
                { "id": "long", "source": "~/.claude/skills/long/SKILL.md", "description": expected_long },
                { "id": "plain", "source": "~/.claude/skills/plain/SKILL.md", "description": "" }
            ])
        );
        assert_eq!(
            f["tools"]["commands"],
            json!([
                { "id": "deploy", "source": "~/.claude/commands/deploy.md" },
                { "id": "review", "source": "~/.claude/commands/review.md" }
            ])
        );
        assert_eq!(
            f["_sources"],
            json!(["~/.claude/skills", "~/.claude/commands"])
        );
    }

    #[test]
    fn skills_detect_on_either_directory() {
        let h = home();
        assert!(!ClaudeSkills.detect(&h.ctx()));
        h.mkdir(".claude/commands");
        let ctx = h.ctx();
        assert!(ClaudeSkills.detect(&ctx));
        let f = ClaudeSkills.run(&ctx, &json!({})).unwrap();
        assert_eq!(f["tools"], json!({ "skills": [], "commands": [] }));
    }
}
