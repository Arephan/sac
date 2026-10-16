//! A synthetic home directory for integration tests. Nothing here comes from a real machine.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

pub struct HomeFixture {
    pub tmp: TempDir,
    pub home: PathBuf,
    pub memory_dir: PathBuf,
}

fn w(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn plist(label: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
{body}
</dict>
</plist>
"#
    )
}

/// Build the synthetic home "alex" inside a fresh temp dir and return it. The memory directory
/// is created at the slug Claude Code would use for this exact home path.
pub fn build_home() -> HomeFixture {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home-alex");
    fs::create_dir_all(&home).unwrap();
    let home = home.canonicalize().unwrap();
    let h = home.to_string_lossy().to_string();
    let bearer = ["Bearer ", "abcdefghijklmnopqrstuvwxyz0123"].concat();
    let apikey = ["sk-", "proj-abcdefghijklmnopqrstuvwxyz"].concat();
    let slack = ["xoxb-", "not-a-real-token-0000000000"].concat();

    w(
        &home.join(".claude/settings.json"),
        &format!(
            r#"{{
  "model": "opus",
  "effortLevel": "high",
  "theme": "dark",
  "env": {{ "FOO_FLAG": "1" }},
  "permissions": {{
    "defaultMode": "default",
    "allow": ["Read", "Bash(git status*)", "mcp__notes__*"],
    "deny": ["Bash(git push --force*)", "mcp__work-chat__*"],
    "ask": ["Bash(rm *)"]
  }},
  "deniedMcpServers": [{{ "serverName": "work-chat" }}],
  "enabledPlugins": {{ "frontend-design@official": true, "old-plugin@official": false }},
  "hooks": {{ "PreToolUse": [{{ "matcher": "Bash", "hooks": [{{ "type": "command", "command": "{h}/.claude/hooks/guard.sh" }}] }}] }}
}}
"#
        ),
    );
    w(
        &home.join(".claude/settings.local.json"),
        r#"{ "permissions": { "allow": ["Bash(ls*)"] } }"#,
    );
    w(
        &home.join(".claude.json"),
        &format!(
            r#"{{
  "mcpServers": {{
    "notes": {{ "type": "http", "url": "https://mcp.example.com/notes?token=abc", "headers": {{ "Authorization": "{bearer}" }} }},
    "local-tool": {{ "command": "node", "args": ["{h}/tools/local.js"], "env": {{ "API_KEY": "{apikey}" }} }}
  }},
  "projects": {{
    "{h}/code/app": {{ "mcpServers": {{ "notes": {{ "command": "notes-cli", "args": [] }} }} }}
  }}
}}
"#
        ),
    );
    w(
        &home.join(".claude/mcp.json"),
        r#"{ "mcpServers": { "work-chat": { "command": "wc", "args": ["--serve"] } } }"#,
    );
    w(
        &home.join(".claude/plugins/installed_plugins.json"),
        r#"{ "version": 2, "plugins": { "frontend-design@official": [{ "version": "abc123" }], "old-plugin@official": [{ "version": "1" }] } }"#,
    );
    w(
        &home.join(".claude/skills/morning-brief/SKILL.md"),
        "---\nname: morning-brief\ndescription: Summarise calendar   and inbox\n  into five bullets\n---\nDo the brief.\n",
    );
    w(
        &home.join(".claude/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review a diff\n---\n",
    );
    w(
        &home.join(".claude/commands/report.md"),
        "Write the report.\n",
    );
    w(
        &home.join(".claude/CLAUDE.md"),
        "@skills/morning-brief/SKILL.md\n\n## BUDGET — HARD RULE (Alex 2026-01-02: \"no purchases without a human\")\n- No purchases, subscriptions or metered API calls.\n\n## Reply style\nKeep replies short.\n\n## Old lane — SUPERSEDED 2026-02-01\nWas a thing.\n\n## Browser Automation\nAlways use the Playwright CLI.\n",
    );

    let slug: String = h
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let memory_dir = home.join(".claude/projects").join(&slug).join("memory");
    w(&memory_dir.join("MEMORY.md"), "# index\n");
    w(
        &memory_dir.join("feedback_no_spend.md"),
        "---\nname: feedback_no_spend\ndescription: HARD rule (Alex 2026-01-03) purchases need a human click\nmetadata:\n  type: feedback\n---\n\nNo money leaves any account.\n\n**Why:** an unattended checkout is not acceptable.\n",
    );
    w(
        &memory_dir.join("feedback_threads.md"),
        "---\nname: feedback_threads\ndescription: reply in the thread you were asked in\nmetadata:\n  type: feedback\n---\n\nKeeps channels readable.\n",
    );
    w(
        &memory_dir.join("project_app.md"),
        "---\nname: project_app\ndescription: The side app lives in ~/code/app and is LIVE on the store\nmetadata:\n  type: project\n---\n\nNotes.\n",
    );
    w(
        &memory_dir.join("project_dead.md"),
        "---\nname: project_dead\ndescription: Old experiment, KILLED 2026-02-02, never rebuild\ntype: project\n---\n",
    );
    w(
        &memory_dir.join("reference_api.md"),
        "---\nname: reference_api\ndescription: Gotchas for the notes API\nmetadata:\n  type: reference\n---\n",
    );
    w(
        &memory_dir.join("user_alex.md"),
        "---\nname: user_alex\ndescription: who alex is\nmetadata:\n  type: user\n---\n",
    );
    w(
        &memory_dir.join("broken.md"),
        "---\nname: [unclosed\ndescription: \"broken frontmatter\"\n---\nbody\n",
    );

    let la = home.join("Library/LaunchAgents");
    w(
        &la.join("com.example.morning-brief.plist"),
        &plist(
            "com.example.morning-brief",
            &format!(
                "\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/bin/bash</string>\n\t\t<string>{h}/assistant/loops/brief.sh</string>\n\t</array>\n\t<key>RunAtLoad</key>\n\t<false/>\n\t<key>StandardOutPath</key>\n\t<string>{h}/assistant/logs/brief.log</string>\n\t<key>StandardErrorPath</key>\n\t<string>{h}/assistant/logs/brief.log</string>\n\t<key>StartCalendarInterval</key>\n\t<array>\n\t\t<dict>\n\t\t\t<key>Hour</key>\n\t\t\t<integer>8</integer>\n\t\t\t<key>Minute</key>\n\t\t\t<integer>0</integer>\n\t\t</dict>\n\t</array>\n\t<key>WorkingDirectory</key>\n\t<string>{h}/assistant</string>"
            ),
        ),
    );
    w(
        &la.join("com.example.watch.plist"),
        &plist(
            "com.example.watch",
            &format!(
                "\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>{h}/assistant/watch.sh</string>\n\t</array>\n\t<key>StartInterval</key>\n\t<integer>300</integer>\n\t<key>KeepAlive</key>\n\t<true/>\n\t<key>RunAtLoad</key>\n\t<true/>\n\t<key>EnvironmentVariables</key>\n\t<dict>\n\t\t<key>PATH</key>\n\t\t<string>/usr/bin</string>\n\t\t<key>TOKEN</key>\n\t\t<string>{slack}</string>\n\t</dict>"
            ),
        ),
    );
    w(
        &la.join("com.example.old.plist.killed-20260913"),
        &plist(
            "com.example.old",
            "\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/bin/true</string>\n\t</array>",
        ),
    );
    w(
        &la.join("com.example.bak.plist.bak"),
        &plist(
            "com.example.bak",
            "\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/bin/true</string>\n\t</array>",
        ),
    );
    w(
        &la.join("disabled/com.example.parked.plist"),
        &plist(
            "com.example.parked",
            "\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/bin/true</string>\n\t</array>",
        ),
    );
    w(
        &la.join("com.vendor.updater.plist"),
        &plist(
            "com.vendor.updater",
            "\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/usr/bin/true</string>\n\t</array>",
        ),
    );

    let remote = format!(
        "https://{}@github.com/alex/app.git",
        ["user:", "secretpass"].concat()
    );
    w(
        &home.join("code/app/.git/config"),
        &format!("[core]\n\trepositoryformatversion = 0\n[remote \"origin\"]\n\turl = {remote}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"),
    );
    w(&home.join("code/app/.git/HEAD"), "ref: refs/heads/main\n");
    w(
        &home.join("assistant/todo.md"),
        "- [ ] one\n- [ ] two\n- [x] done\n",
    );
    w(&home.join("assistant/journal.jsonl"), "{}\n");

    HomeFixture {
        tmp,
        home,
        memory_dir,
    }
}

/// `overrides.yaml` content that names the synthetic home's managed jobs and roles.
pub fn overrides_yaml() -> &'static str {
    r#"import:
  importers: all
  options:
    launchd: { managed_prefixes: [com.example.] }
    infra: { repo_roots: ["~/code"], clis: [git] }

identity:
  accounts:
    - id: assistant-bot
      kind: chat_bot
      workspace: personal-workspace
      credential_ref: ~/.assistant_bot_token
      scopes: [chat:write]
      access: can_post
    - id: personal-mail
      kind: mailbox
      principal: me@example.com
      credential_ref: env:MAIL_APP_PASSWORD
      access: can_post
    - id: work-chat-user
      kind: chat_user
      workspace: employer-workspace
      access: forbidden

loops:
  com.example.morning-brief: { owner: personal, success_metric: brief posted by 08:15 }
  com.example.watch: { owner: personal, success_metric: alert within 5 minutes }

channels:
  slack:
    - { id: home-channel, role: every reply goes here, identity: assistant-bot }
  email:
    - { id: outbound-mail, identity: personal-mail, gate: human-voice-gate }
  todo:
    - { id: todo, path: ~/assistant/todo.md }

data:
  state:
    - { id: journal, path: ~/assistant/journal.jsonl, kind: journal }
    - { id: missing, path: ~/assistant/nope.jsonl, kind: ledger }
"#
}

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn out(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
    }
}

/// Run the `sac` binary built by cargo with colour off.
pub fn sac(args: &[&str], cwd: &Path) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_sac"))
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env_remove("SAC_INSTANCE")
        .output()
        .expect("sac runs");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

/// Create an instance folder with `init`, then write the synthetic overrides.
pub fn init_instance(root: &Path, name: &str) -> PathBuf {
    let inst = root.join("instance").join(name);
    let r = sac(&["init", inst.to_str().unwrap(), "--name", name], root);
    assert_eq!(r.code, 0, "init failed: {}", r.out());
    fs::write(inst.join("overrides.yaml"), overrides_yaml()).unwrap();
    inst
}

/// Recursive copy, used to materialise the fixture for scripts (parity, demos).
pub fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.path().is_dir() {
            copy_tree(&e.path(), &dest);
        } else {
            fs::copy(e.path(), &dest).unwrap();
        }
    }
}
