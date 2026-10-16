//! The user crontab (read-only `crontab -l`, real home only). Each non-comment line becomes a loop.

use super::Importer;
use crate::ctx::Ctx;
use crate::util::slug;
use crate::Doc;
use anyhow::Result;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

pub struct Cron;

/// Crontab text to loops: blank lines, comments and `NAME=value` lines are skipped; a line
/// starting with `@` has a one-word schedule, any other has five fields.
fn parse_crontab(ctx: &Ctx, text: &str) -> Vec<Doc> {
    static ENV_LINE: OnceLock<Regex> = OnceLock::new();
    let env_line = ENV_LINE.get_or_init(|| Regex::new(r"^[A-Z_]+=").expect("regex compiles"));
    let mut loops = Vec::new();
    for line in text.split('\n') {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') || env_line.is_match(l) {
            continue;
        }
        let parts: Vec<&str> = l.split_whitespace().collect();
        let n = if parts[0].starts_with('@') {
            1
        } else {
            parts.len().min(5)
        };
        let cmd = parts[n..].join(" ");
        loops.push(json!({
            "id": format!("cron-{}", slug(&cmd, 40)),
            "runner": "cron",
            "enabled": true,
            "status": "installed",
            "schedule": { "cron": parts[..n].join(" ") },
            "program": [ctx.scrub(&Value::String(cmd))],
        }));
    }
    loops
}

impl Importer for Cron {
    fn id(&self) -> &str {
        "cron"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["loops"]
    }
    fn description(&self) -> &str {
        "User crontab entries."
    }
    fn detect(&self, ctx: &Ctx) -> bool {
        ctx.is_real_home() && ctx.platform() != "win32"
    }
    fn run(&self, ctx: &Ctx, _opts: &Doc) -> Result<Doc> {
        let text = if ctx.is_real_home() {
            ctx.try_exec("crontab", &["-l"]).unwrap_or_default()
        } else {
            String::new()
        };
        Ok(json!({ "loops": parse_crontab(ctx, &text) }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> (tempfile::TempDir, Ctx) {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        (tmp, ctx)
    }

    #[test]
    fn parses_five_field_and_macro_lines() {
        let (_tmp, ctx) = sandbox();
        let h = ctx.home().to_string_lossy().into_owned();
        let backup = format!("{h}/bin/backup.sh --full");
        let text = format!(
            "# m h dom mon dow command\nMAILTO=\"\"\nPATH=/usr/bin:/bin\n\n\
             */5 * * * * cmd\n@daily {backup}\n  30 9 * * 1-5   /usr/local/bin/report  \n"
        );
        let loops = parse_crontab(&ctx, &text);
        assert_eq!(
            Doc::Array(loops),
            json!([
                {
                    "id": "cron-cmd", "runner": "cron", "enabled": true, "status": "installed",
                    "schedule": { "cron": "*/5 * * * *" }, "program": ["cmd"]
                },
                {
                    "id": format!("cron-{}", slug(&backup, 40)), "runner": "cron", "enabled": true,
                    "status": "installed", "schedule": { "cron": "@daily" },
                    "program": ["~/bin/backup.sh --full"]
                },
                {
                    "id": "cron-usr-local-bin-report", "runner": "cron", "enabled": true,
                    "status": "installed", "schedule": { "cron": "30 9 * * 1-5" },
                    "program": ["/usr/local/bin/report"]
                }
            ])
        );
    }

    #[test]
    fn short_lines_and_secret_commands() {
        let (_tmp, ctx) = sandbox();
        let fake = format!("{}={}", "token", "a".repeat(30));
        let text = format!("* * * cmd\n0 * * * * curl -s {fake}\n");
        let loops = parse_crontab(&ctx, &text);
        assert_eq!(loops[0]["id"], json!("cron-item"));
        assert_eq!(loops[0]["schedule"], json!({ "cron": "* * * cmd" }));
        assert_eq!(loops[0]["program"], json!([""]));
        assert_eq!(loops[1]["program"], json!(["<redacted>"]));
        assert_eq!(ctx.warnings().len(), 1);
    }

    #[test]
    fn sandbox_home_is_skipped() {
        let (_tmp, ctx) = sandbox();
        assert!(!Cron.detect(&ctx));
        assert_eq!(Cron.run(&ctx, &json!({})).unwrap(), json!({ "loops": [] }));
    }
}
