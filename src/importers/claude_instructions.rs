//! Importer `claude-instructions`: the global `~/.claude/CLAUDE.md`. Each `## ` section becomes
//! a rule; each `@path` import line becomes a source (the imported file is not read).
//! Ported from the Node prototype (tag v0.2.0).

use super::rules::{
    classify_scope, first_date, js_shown, js_slice, scrub_text, severity_of, static_re,
};
use super::Importer;
use crate::ctx::Ctx;
use crate::util::{one_line, slug};
use crate::Doc;
use anyhow::Result;
use serde_json::json;

pub struct ClaudeInstructions;

const MAIN: &str = "~/.claude/CLAUDE.md";

/// One rule per `## ` section of `text`: id from the heading's title (the part before a
/// spaced dash or an opening parenthesis), severity and status from markers in the heading,
/// reason from a quote or parenthesis in the heading, statement from the body. `scrub` cleans
/// the reason and statement text before they are cut to length (the importer passes
/// `Ctx::scrub`, so home paths become `~` and secret-shaped text is dropped).
pub fn parse_instruction_sections(
    text: &str,
    source: &str,
    scope_keywords: Option<&Doc>,
    fallback_date: Option<&str>,
    scrub: &dyn Fn(&str) -> String,
) -> Vec<Doc> {
    let mut rules = Vec::new();
    for part in static_re!(r"(?m)^## ").split(text).skip(1) {
        let (heading_line, rest) = part.split_once('\n').unwrap_or((part, ""));
        let heading = heading_line.trim();
        let body = rest.trim();
        let title = match static_re!(r"\s+[—–-]\s+|\s*\(").find(heading) {
            Some(m) => &heading[..m.start()],
            None => heading,
        };
        let quoted = static_re!(r#"["“]([^"”]+)["”]"#)
            .captures(heading)
            .map(|c| c[1].to_string());
        let in_parens = static_re!(r"\(([^)]*)\)")
            .captures(heading)
            .map(|c| c[1].to_string())
            .filter(|s| !s.is_empty());
        let reason = quoted.or(in_parens).unwrap_or_else(|| title.to_string());
        let statement = static_re!(r"(?m)^[-*]\s+").replace_all(body, "");
        let scope_text = format!("{heading} {heading} {heading} {}", js_slice(body, 600));
        rules.push(json!({
            "id": format!("md-{}", slug(title, 48)),
            "scope": classify_scope(&scope_text, scope_keywords),
            "severity": severity_of(heading),
            "date": first_date(&[heading, body]).or_else(|| fallback_date.map(str::to_string)),
            "reason": one_line(&scrub(&reason), 200),
            "statement": one_line(&scrub(&statement), 400),
            "source": source,
            "status": if heading.contains("SUPERSEDED") { "superseded" } else { "active" },
        }));
    }
    rules
}

impl Importer for ClaudeInstructions {
    fn id(&self) -> &str {
        "claude-instructions"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["instructions"]
    }
    fn description(&self) -> &str {
        "Global CLAUDE.md sections as rules; @imports as sources."
    }
    fn detect(&self, ctx: &Ctx) -> bool {
        ctx.exists(MAIN)
    }
    fn run(&self, ctx: &Ctx, opts: &Doc) -> Result<Doc> {
        let text = ctx.read_text(MAIN)?;
        let mut sources = vec![json!({ "id": "global-claude-md", "path": MAIN, "kind": "global" })];
        for c in static_re!(r"(?m)^@(\S+)\s*$").captures_iter(&text) {
            let target = &c[1];
            let rel = if target.starts_with('~') || target.starts_with('/') {
                target.to_string()
            } else {
                format!("~/.claude/{target}")
            };
            sources.push(json!({
                "id": format!("import-{}", slug(target, 40)),
                "path": js_shown(ctx, &rel),
                "kind": "import",
            }));
        }
        let rules = parse_instruction_sections(
            &text,
            MAIN,
            opts.get("scope_keywords"),
            ctx.mtime_date(MAIN).as_deref(),
            &|s: &str| scrub_text(ctx, s),
        );
        Ok(json!({
            "instructions": { "sources": sources, "rules": rules },
            "_sources": [MAIN],
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importers::rules::testing::{home, keys, DROPPED};
    use serde_json::json;

    const CLAUDE_MD: &str = "@NOTES.md
@skills/x/SKILL.md
@~/shared/RULES.md

Intro text that is not a rule.

## BIKES — HARD RULE (crew 2031-04-05: \"lock every bike\")
- Chain each bike to the rack.
- Test   the brakes weekly.

## Old pump rule (SUPERSEDED by the new one)
Use the old floor pump.

## Formatting
Use metric units.
### Sub heading stays in the body
";

    #[test]
    fn instructions_turn_sections_into_rules_and_imports_into_sources() {
        let h = home();
        h.write(".claude/CLAUDE.md", CLAUDE_MD);
        let ctx = h.ctx();
        assert!(ClaudeInstructions.detect(&ctx));
        let mtime = ctx.mtime_date("~/.claude/CLAUDE.md").unwrap();
        let f = ClaudeInstructions.run(&ctx, &json!({})).unwrap();

        assert_eq!(
            f["instructions"]["sources"],
            json!([
                { "id": "global-claude-md", "path": "~/.claude/CLAUDE.md", "kind": "global" },
                { "id": "import-notes-md", "path": "~/.claude/NOTES.md", "kind": "import" },
                { "id": "import-skills-x-skill-md", "path": "~/.claude/skills/x/SKILL.md", "kind": "import" },
                { "id": "import-shared-rules-md", "path": "~/shared/RULES.md", "kind": "import" }
            ])
        );
        let rules = f["instructions"]["rules"].as_array().unwrap();
        assert_eq!(rules.len(), 3);
        assert_eq!(
            rules[0],
            json!({
                "id": "md-bikes",
                "scope": "code",
                "severity": "hard",
                "date": "2031-04-05",
                "reason": "lock every bike",
                "statement": "Chain each bike to the rack. Test the brakes weekly.",
                "source": "~/.claude/CLAUDE.md",
                "status": "active"
            })
        );
        assert_eq!(rules[1]["id"], json!("md-old-pump-rule"));
        assert_eq!(rules[1]["status"], json!("superseded"));
        assert_eq!(rules[1]["severity"], json!("standing"));
        assert_eq!(rules[1]["reason"], json!("SUPERSEDED by the new one"));
        assert_eq!(rules[1]["scope"], json!("global"));
        assert_eq!(rules[1]["date"], json!(mtime));
        assert_eq!(
            rules[2],
            json!({
                "id": "md-formatting",
                "scope": "global",
                "severity": "standing",
                "date": mtime,
                "reason": "Formatting",
                "statement": "Use metric units. ### Sub heading stays in the body",
                "source": "~/.claude/CLAUDE.md",
                "status": "active"
            })
        );
        assert_eq!(
            keys(&rules[0]),
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
        assert_eq!(f["_sources"], json!(["~/.claude/CLAUDE.md"]));
    }

    #[test]
    fn import_paths_expand_and_normalize_like_the_reference() {
        let h = home();
        h.write(
            ".claude/CLAUDE.md",
            "@../AGENTS.md\n@./x.md\n@~/a/../b.md\n@/opt/x/./y.md\n  @indented.md\n@two words\n",
        );
        let f = ClaudeInstructions.run(&h.ctx(), &json!({})).unwrap();
        let sources = f["instructions"]["sources"].as_array().unwrap();
        let paths: Vec<&str> = sources
            .iter()
            .map(|s| s["path"].as_str().unwrap())
            .collect();
        assert_eq!(
            paths,
            [
                "~/.claude/CLAUDE.md",
                "~/AGENTS.md",
                "~/.claude/x.md",
                "~/b.md",
                "/opt/x/./y.md"
            ]
        );
        assert_eq!(sources[1]["id"], json!("import-agents-md"));
        assert_eq!(f["instructions"]["rules"], json!([]));
    }

    #[test]
    fn section_titles_reasons_and_dates_follow_the_heading_rules() {
        let rules = parse_instruction_sections(
            "## (only parens)\nx\n## Hyphen - split - title (with (nested) parens)\nbody\n## Empty () reason\n## Curly “quoted” – en dash\nsee 2031-13-45\n",
            "~/CLAUDE.md",
            None,
            None,
            &|s: &str| s.to_string(),
        );
        let ids: Vec<&str> = rules.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["md-item", "md-hyphen", "md-empty", "md-curly-quoted"]);
        assert_eq!(rules[0]["reason"], json!("only parens"));
        assert_eq!(rules[1]["reason"], json!("with (nested"));
        assert_eq!(rules[2]["reason"], json!("Empty"));
        assert_eq!(rules[3]["reason"], json!("quoted"));
        // the date pattern does not validate month or day, as in the reference
        assert_eq!(rules[3]["date"], json!("2031-13-45"));
        assert_eq!(rules[0]["date"], json!(null));
        assert_eq!(rules[2]["statement"], json!(""));
    }

    #[test]
    fn section_reasons_and_statements_are_scrubbed() {
        let h = home();
        h.write(
            ".claude/CLAUDE.md",
            "## Drafts (kept in @HOME@/drafts)\nSave drafts under @HOME@/drafts/new before sharing.\n\n## Keys\nThe service key is @TOKEN@ for now.\n",
        );
        let ctx = h.ctx();
        let f = ClaudeInstructions.run(&ctx, &json!({})).unwrap();
        let rules = &f["instructions"]["rules"];
        assert_eq!(rules[0]["id"], json!("md-drafts"));
        assert_eq!(rules[0]["reason"], json!("kept in ~/drafts"));
        assert_eq!(
            rules[0]["statement"],
            json!("Save drafts under ~/drafts/new before sharing.")
        );
        assert_eq!(rules[1]["reason"], json!("Keys"));
        assert_eq!(rules[1]["statement"], json!("<redacted>"));
        assert_eq!(ctx.warnings(), [DROPPED]);
        let text = f.to_string();
        assert!(!text.contains("xoxb"), "a token leaked: {text}");
        assert!(!text.contains(&h.s()), "a home path leaked: {text}");
    }

    #[test]
    fn instructions_detect_and_scope_keywords_option() {
        let h = home();
        assert!(!ClaudeInstructions.detect(&h.ctx()));
        h.write(".claude/CLAUDE.md", "## Pets\nFeed the cat.\n");
        let f = ClaudeInstructions
            .run(&h.ctx(), &json!({ "scope_keywords": { "pets": ["cat"] } }))
            .unwrap();
        assert_eq!(f["instructions"]["rules"][0]["scope"], json!("pets"));
        assert_eq!(f["instructions"]["sources"].as_array().unwrap().len(), 1);
    }
}
