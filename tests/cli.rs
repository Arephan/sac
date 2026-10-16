//! End-to-end tests of the `sac` binary over a synthetic home. See tests/common/mod.rs.

mod common;

use common::{build_home, init_instance, sac};
use sac::util::{arr, get_path, s};
use sac::yaml::read_yaml;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn ids(doc: &Value, path: &str) -> Vec<String> {
    arr(get_path(doc, path))
        .iter()
        .map(|x| s(x.get("id")).to_string())
        .collect()
}

fn find<'a>(doc: &'a Value, path: &str, id: &str) -> &'a Value {
    arr(get_path(doc, path))
        .iter()
        .find(|x| s(x.get("id")) == id)
        .unwrap_or_else(|| panic!("{path} has no item {id}"))
}

#[test]
fn init_then_validate() {
    let fx = build_home();
    let root = fx.tmp.path();
    let inst = root.join("instance/alex");
    let r = sac(&["init", inst.to_str().unwrap(), "--name", "alex"], root);
    assert_eq!(r.code, 0, "{}", r.out());
    for f in [
        "setup.yaml",
        "policies.yaml",
        "overrides.yaml",
        "lint-denylist.txt",
    ] {
        assert!(inst.join(f).exists(), "{f} missing");
    }
    assert!(inst.join("importers").is_dir() && inst.join("checks").is_dir());
    let again = sac(&["init", inst.to_str().unwrap()], root);
    assert_eq!(again.code, 1, "second init without --force must fail");
    let v = sac(&["validate", "-i", inst.to_str().unwrap()], root);
    assert_eq!(v.code, 0, "{}", v.out());
    assert!(v.stdout.contains("VALID"), "{}", v.stdout);
    let protected = sac(
        &[
            "init",
            fx.home.join(".claude/x").to_str().unwrap(),
            "--home",
            fx.home.to_str().unwrap(),
        ],
        root,
    );
    assert_ne!(protected.code, 0);
}

#[test]
fn import_reads_the_whole_home() {
    let fx = build_home();
    let root = fx.tmp.path();
    let inst = init_instance(root, "alex");
    let home = fx.home.to_str().unwrap();
    let r = sac(
        &["import", "-i", inst.to_str().unwrap(), "--home", home, "-v"],
        root,
    );
    assert_eq!(r.code, 0, "{}", r.out());
    assert!(r.stdout.contains("counts per layer"), "{}", r.stdout);
    let d = read_yaml(&inst.join("setup.yaml")).unwrap();

    // tools
    let mcp = ids(&d, "tools.mcp_servers");
    assert_eq!(mcp.len(), 4, "{mcp:?}");
    assert!(mcp.contains(&"notes".to_string()));
    assert!(mcp.contains(&"local-tool".to_string()));
    assert!(mcp.contains(&"work-chat".to_string()));
    assert!(mcp.iter().any(|m| m.starts_with("notes@")), "{mcp:?}");
    let notes = find(&d, "tools.mcp_servers", "notes");
    assert_eq!(notes["url"], "https://mcp.example.com/notes");
    assert_eq!(notes["header_names"], serde_json::json!(["Authorization"]));
    assert!(notes.get("headers").is_none());
    let local = find(&d, "tools.mcp_servers", "local-tool");
    assert_eq!(local["env_names"], serde_json::json!(["API_KEY"]));
    assert_eq!(local["args"][0], "~/tools/local.js");
    assert_eq!(ids(&d, "tools.skills").len(), 2);
    assert_eq!(
        find(&d, "tools.skills", "morning-brief")["description"],
        "Summarise calendar and inbox into five bullets"
    );
    assert_eq!(ids(&d, "tools.commands"), vec!["report"]);
    assert_eq!(ids(&d, "tools.plugins").len(), 2);
    assert_eq!(ids(&d, "tools.hooks").len(), 1);
    let hook = &arr(get_path(&d, "tools.hooks"))[0];
    assert_eq!(hook["event"], "PreToolUse");
    assert_eq!(hook["command"], "~/.claude/hooks/guard.sh");
    let allow = get_path(&d, "tools.permissions.allow").unwrap();
    assert!(
        allow
            .as_array()
            .unwrap()
            .contains(&Value::String("Bash(ls*)".into())),
        "{allow}"
    );
    assert_eq!(
        get_path(&d, "tools.permissions.deny")
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        get_path(&d, "tools.denied_mcp_servers").unwrap(),
        &serde_json::json!(["work-chat"])
    );
    assert_eq!(get_path(&d, "brain.claude_code.model").unwrap(), "opus");
    assert_eq!(
        get_path(&d, "brain.claude_code.env_names").unwrap(),
        &serde_json::json!(["FOO_FLAG"])
    );

    // instructions: 4 CLAUDE.md sections + 2 memory feedback files
    let rules = ids(&d, "instructions.rules");
    assert_eq!(rules.len(), 6, "{rules:?}");
    let money = find(&d, "instructions.rules", "md-budget");
    assert_eq!(money["severity"], "hard");
    assert_eq!(money["date"], "2026-01-02");
    assert_eq!(money["reason"], "no purchases without a human");
    assert_eq!(
        find(&d, "instructions.rules", "md-old-lane")["status"],
        "superseded"
    );
    assert_eq!(
        find(&d, "instructions.rules", "mem-feedback_no_spend")["severity"],
        "hard"
    );
    assert_eq!(
        find(&d, "instructions.rules", "mem-feedback_no_spend")["reason"],
        "an unattended checkout is not acceptable."
    );
    let sources = ids(&d, "instructions.sources");
    assert!(
        sources.contains(&"global-claude-md".to_string())
            && sources.contains(&"memory".to_string()),
        "{sources:?}"
    );
    assert!(
        sources
            .iter()
            .any(|x| x.starts_with("import-skills-morning-brief")),
        "{sources:?}"
    );

    // projects, data, identity
    assert_eq!(ids(&d, "projects").len(), 2);
    assert_eq!(find(&d, "projects", "project_app")["path"], "~/code/app");
    assert_eq!(find(&d, "projects", "project_app")["status"], "live");
    assert_eq!(find(&d, "projects", "project_dead")["status"], "killed");
    assert_eq!(ids(&d, "data.references"), vec!["reference_api"]);
    assert_eq!(get_path(&d, "data.memory.files").unwrap(), 7);
    assert_eq!(get_path(&d, "data.memory.by_type.unknown").unwrap(), 1);
    assert_eq!(get_path(&d, "data.memory.by_type.feedback").unwrap(), 2);
    let refs = get_path(&d, "identity.person.profile_refs").unwrap();
    assert!(refs.as_array().unwrap()[0]
        .as_str()
        .unwrap()
        .ends_with("/memory/user_alex.md"));
    assert_eq!(find(&d, "data.state", "journal")["exists"], true);
    assert_eq!(find(&d, "data.state", "missing")["exists"], false);
    assert_eq!(ids(&d, "identity.accounts").len(), 3);

    // loops from launchd
    let loops = ids(&d, "loops");
    assert_eq!(loops.len(), 6, "{loops:?}");
    let brief = find(&d, "loops", "com.example.morning-brief");
    assert_eq!(brief["enabled"], true);
    assert_eq!(brief["status"], "installed");
    assert_eq!(
        brief["schedule"]["calendar"],
        serde_json::json!(["hour=8,minute=0"])
    );
    assert_eq!(brief["program"][1], "~/assistant/loops/brief.sh");
    assert_eq!(brief["log"], "~/assistant/logs/brief.log");
    assert_eq!(brief["working_dir"], "~/assistant");
    assert_eq!(brief["success_metric"], "brief posted by 08:15");
    let watch = find(&d, "loops", "com.example.watch");
    assert_eq!(watch["schedule"]["every_seconds"], 300);
    assert_eq!(watch["schedule"]["keep_alive"], true);
    assert_eq!(watch["env_names"], serde_json::json!(["PATH", "TOKEN"]));
    assert_eq!(
        find(&d, "loops", "com.example.old")["status"],
        "parked:killed"
    );
    assert_eq!(
        find(&d, "loops", "com.example.bak")["status"],
        "backup-only"
    );
    assert_eq!(find(&d, "loops", "com.example.parked")["status"], "parked");
    assert_eq!(find(&d, "loops", "com.vendor.updater")["managed"], false);

    // infra
    let repo = find(&d, "infra.repos", "app");
    assert_eq!(repo["remote"], "https://github.com/alex/app.git");
    assert_eq!(repo["path"], "~/code/app");
    assert!(repo.get("last_commit").is_none());
    assert_eq!(find(&d, "infra.clis", "git")["present"], true);
    assert!(!get_path(&d, "infra.machine.platform")
        .unwrap()
        .as_str()
        .unwrap()
        .is_empty());

    // declared-only collections survive the import
    assert_eq!(ids(&d, "brain.gates"), vec!["human-voice-gate"]);
    assert_eq!(ids(&d, "channels.slack"), vec!["home-channel"]);
    assert_eq!(ids(&d, "safety.policies").len(), 5);
    assert!(get_path(&d, "meta.generated_at").is_some());

    // the written file must not leak values
    let text = fs::read_to_string(inst.join("setup.yaml")).unwrap();
    for leak in [
        "secretpass",
        "proj-abcdefghijklmnopqrstuvwxyz",
        "not-a-real-token",
        "abcdefghijklmnopqrstuvwxyz0123",
    ] {
        assert!(!text.contains(leak), "secret leaked: {leak}");
    }
    assert!(!text.contains(home), "absolute home path leaked");
}

#[test]
fn plan_detects_drift_and_verify_exit_codes() {
    let fx = build_home();
    let root = fx.tmp.path();
    let inst = init_instance(root, "alex");
    let home = fx.home.to_str().unwrap();
    let i = inst.to_str().unwrap();
    assert_eq!(sac(&["import", "-i", i, "--home", home], root).code, 0);

    let p = sac(&["plan", "-i", i, "--home", home], root);
    assert_eq!(p.code, 0, "{}", p.out());
    assert!(
        p.stdout
            .contains("Plan: 0 to add, 0 to change, 0 untracked"),
        "{}",
        p.stdout
    );

    let v = sac(&["verify", "-i", i, "--home", home], root);
    assert_eq!(v.code, 0, "{}", v.out());
    assert!(v.stdout.contains("VERIFY OK"), "{}", v.stdout);
    assert!(
        v.stdout.contains("[PASS] hard     no-payment-tools"),
        "{}",
        v.stdout
    );

    // drift: a new skill on the machine, a loop file removed
    fs::create_dir_all(fx.home.join(".claude/skills/new-skill")).unwrap();
    fs::write(
        fx.home.join(".claude/skills/new-skill/SKILL.md"),
        "---\ndescription: new\n---\n",
    )
    .unwrap();
    let watch = fx.home.join("Library/LaunchAgents/com.example.watch.plist");
    let saved = fs::read_to_string(&watch).unwrap();
    fs::remove_file(&watch).unwrap();
    let p2 = sac(&["plan", "-i", i, "--home", home, "--json"], root);
    assert_eq!(p2.code, 0, "{}", p2.out());
    let j: Value = serde_json::from_str(&p2.stdout).unwrap();
    // the removed plist is still declared in overrides.yaml, so it shows as changed
    // (status installed vs declared-only), while the new skill is untracked on the machine
    assert_eq!(j["totals"]["add"], 0, "{}", p2.stdout);
    assert_eq!(j["totals"]["change"], 1, "{}", p2.stdout);
    assert_eq!(j["totals"]["untracked"], 1, "{}", p2.stdout);
    let text = sac(&["plan", "-i", i, "--home", home], root).stdout;
    assert!(text.contains("- tools.skills new-skill"), "{text}");
    assert!(text.contains("~ loops com.example.watch"), "{text}");
    let d = sac(
        &["verify", "-i", i, "--home", home, "--fail-on-drift"],
        root,
    );
    assert_eq!(d.code, 2, "{}", d.out());
    fs::write(&watch, saved).unwrap();
    fs::remove_dir_all(fx.home.join(".claude/skills/new-skill")).unwrap();

    // a hard policy failure exits 1
    let pol = fs::read_to_string(inst.join("policies.yaml")).unwrap();
    fs::write(
        inst.join("policies.yaml"),
        format!("{pol}    - id: must-exist\n      check: required_rules\n      severity: hard\n      params: {{ ids: [does-not-exist] }}\n"),
    )
    .unwrap();
    let f = sac(&["verify", "-i", i, "--home", home], root);
    assert_eq!(f.code, 1, "{}", f.out());
    assert!(
        f.stdout.contains("VERIFY FAILED") && f.stdout.contains("must-exist"),
        "{}",
        f.stdout
    );
    let a = sac(
        &[
            "apply",
            "-i",
            i,
            "--home",
            home,
            "--out",
            root.join("out").to_str().unwrap(),
        ],
        root,
    );
    assert_eq!(
        a.code,
        1,
        "apply must refuse on a hard failure: {}",
        a.out()
    );
    fs::write(inst.join("policies.yaml"), pol).unwrap();
}

#[test]
fn apply_renders_files_and_plists() {
    let fx = build_home();
    let root = fx.tmp.path();
    let inst = init_instance(root, "alex");
    let home = fx.home.to_str().unwrap();
    let i = inst.to_str().unwrap();
    assert_eq!(sac(&["import", "-i", i, "--home", home], root).code, 0);
    let out = root.join("out");
    let a = sac(
        &[
            "apply",
            "-i",
            i,
            "--home",
            home,
            "--out",
            out.to_str().unwrap(),
        ],
        root,
    );
    assert_eq!(a.code, 0, "{}", a.out());
    let od = out.join("alex");
    for f in [
        "claude/settings.json",
        "claude/mcp-servers.json",
        "claude/RULES.md",
        "launchd/com.example.morning-brief.plist",
        "launchd/com.example.watch.plist",
        "launchd/disabled/com.example.old.plist",
        "setup.lock.json",
        "setup.resolved.yaml",
        "report.md",
    ] {
        assert!(od.join(f).exists(), "{f} not rendered");
    }
    let settings: Value =
        serde_json::from_str(&fs::read_to_string(od.join("claude/settings.json")).unwrap())
            .unwrap();
    assert_eq!(settings["deniedMcpServers"][0]["serverName"], "work-chat");
    assert_eq!(settings["permissions"]["deny"].as_array().unwrap().len(), 2);
    assert_eq!(
        settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "~/.claude/hooks/guard.sh"
    );
    let mcp: Value =
        serde_json::from_str(&fs::read_to_string(od.join("claude/mcp-servers.json")).unwrap())
            .unwrap();
    assert!(
        mcp["mcpServers"].get("work-chat").is_none(),
        "denied server must not render"
    );
    assert_eq!(
        mcp["mcpServers"]["notes"]["headers"]["Authorization"],
        "${AUTHORIZATION}"
    );
    let plist = fs::read_to_string(od.join("launchd/com.example.morning-brief.plist")).unwrap();
    assert!(
        plist.contains("<key>Label</key>") && plist.contains("<key>StartCalendarInterval</key>"),
        "{plist}"
    );
    assert!(
        plist.contains(&format!("{home}/assistant/loops/brief.sh")),
        "program expanded against the home"
    );
    if which("plutil") {
        let st = std::process::Command::new("plutil")
            .args([
                "-lint",
                "-s",
                od.join("launchd/com.example.morning-brief.plist")
                    .to_str()
                    .unwrap(),
            ])
            .status()
            .unwrap();
        assert!(st.success(), "plutil -lint failed");
    }
    let rules = fs::read_to_string(od.join("claude/RULES.md")).unwrap();
    assert!(
        rules.contains("## Hard rules") && rules.contains("md-budget"),
        "{rules}"
    );
    let lock: Value =
        serde_json::from_str(&fs::read_to_string(od.join("setup.lock.json")).unwrap()).unwrap();
    assert_eq!(lock["instance"], "alex");
    assert_eq!(lock["sha256"].as_str().unwrap().len(), 64);
    // out root inside live config is refused
    let bad = sac(
        &[
            "apply",
            "-i",
            i,
            "--home",
            home,
            "--out",
            fx.home.join(".claude/out").to_str().unwrap(),
        ],
        root,
    );
    assert_ne!(bad.code, 0, "{}", bad.out());
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

#[test]
fn lint_catches_canaries_and_passes_clean_trees() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("instance/x")).unwrap();
    fs::write(root.join("instance/x/lint-denylist.txt"), "# me\nzorblax\n").unwrap();
    fs::write(
        root.join("src/clean.rs"),
        "// me@example.com lives at /Users/alex and that is fine\n",
    )
    .unwrap();
    let clean = sac(&["lint"], root);
    assert_eq!(clean.code, 0, "{}", clean.out());
    assert!(clean.stdout.contains("LINT PASSED"), "{}", clean.stdout);
    let email = ["someone@", "realmail.io"].concat();
    let phone = ["514-", "555-0199"].concat();
    let home_path = ["/Users/", "someone/x"].concat();
    let chat_id = ["C01234", "56789"].concat();
    let token = ["xoxb-", "1234567890-abcdefghij"].concat();
    fs::write(
        root.join("src/dirty.rs"),
        format!("// contact {email} or {phone}\n// home {home_path}\n// channel {chat_id} token {token}\n// Zorblax was here\n"),
    )
    .unwrap();
    let dirty = sac(&["lint"], root);
    assert_eq!(dirty.code, 1, "{}", dirty.out());
    for kind in [
        "email",
        "phone",
        "home-path",
        "chat-id",
        "secret",
        "instance-term",
    ] {
        assert!(
            dirty.stdout.contains(kind),
            "missing {kind} in {}",
            dirty.stdout
        );
    }
    assert!(dirty.stdout.contains("src/dirty.rs:4"), "{}", dirty.stdout);
    // instance/ itself is never scanned
    fs::write(root.join("instance/x/notes.md"), format!("{email}\n")).unwrap();
    let again = sac(&["lint"], root);
    assert!(
        !again.stdout.contains("instance/x/notes.md"),
        "{}",
        again.stdout
    );
}

#[test]
fn redact_is_shape_only_and_valid() {
    let fx = build_home();
    let root = fx.tmp.path();
    let inst = init_instance(root, "alex");
    let home = fx.home.to_str().unwrap();
    let i = inst.to_str().unwrap();
    assert_eq!(sac(&["import", "-i", i, "--home", home], root).code, 0);
    let out = root.join("redacted");
    let r = sac(&["redact", "-i", i, "--out", out.to_str().unwrap()], root);
    assert_eq!(r.code, 0, "{}", r.out());
    let text = fs::read_to_string(out.join("setup.yaml")).unwrap();
    assert!(
        !text.contains("morning-brief") && !text.contains("alex") && !text.contains("example.com"),
        "{text}"
    );
    assert!(text.contains("severity: hard"));
    let v = sac(&["validate", "-i", out.to_str().unwrap()], root);
    assert_eq!(v.code, 0, "{}", v.out());
}

#[test]
fn external_importer_and_check_from_the_minimal_example() {
    let fx = build_home();
    let root = fx.tmp.path();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let example = repo.join("examples/minimal");
    let list = sac(&["importers", "-i", example.to_str().unwrap()], root);
    assert_eq!(list.code, 0, "{}", list.out());
    assert!(list.stdout.contains("todo-md"), "{}", list.stdout);
    let home = fx.home.to_str().unwrap();
    let v = sac(
        &["verify", "-i", example.to_str().unwrap(), "--home", home],
        root,
    );
    assert!(
        v.stdout.contains("few-loops (max_enabled_loops)"),
        "{}",
        v.out()
    );
    assert!(
        v.stdout.contains("[PASS] standing few-loops")
            || v.stdout.contains("[FAIL] standing few-loops"),
        "{}",
        v.stdout
    );
    let p = sac(
        &[
            "plan",
            "-i",
            example.to_str().unwrap(),
            "--home",
            home,
            "--json",
        ],
        root,
    );
    assert_eq!(p.code, 0, "{}", p.out());
}

#[test]
fn round_trip_apply_live_into_an_empty_home() {
    let fx = build_home();
    let root = fx.tmp.path();
    let inst = init_instance(root, "alex");
    let home_a = fx.home.to_str().unwrap();
    let i = inst.to_str().unwrap();
    assert_eq!(sac(&["import", "-i", i, "--home", home_a], root).code, 0);

    let home_b = root.join("home-b");
    fs::create_dir_all(&home_b).unwrap();
    let home_b = home_b.canonicalize().unwrap();
    let hb = home_b.to_str().unwrap();
    let out = root.join("out-b");
    let refused = sac(
        &[
            "apply",
            "-i",
            i,
            "--home",
            hb,
            "--out",
            out.to_str().unwrap(),
            "--live",
        ],
        root,
    );
    assert_ne!(
        refused.code,
        0,
        "--live without --yes must refuse: {}",
        refused.out()
    );
    let a = sac(
        &[
            "apply",
            "-i",
            i,
            "--home",
            hb,
            "--out",
            out.to_str().unwrap(),
            "--live",
            "--yes",
        ],
        root,
    );
    assert_eq!(a.code, 0, "{}", a.out());
    assert!(home_b.join(".claude/settings.json").exists());
    assert!(home_b.join(".claude.json").exists());
    assert!(home_b
        .join("Library/LaunchAgents/com.example.morning-brief.plist")
        .exists());
    assert!(home_b
        .join("Library/LaunchAgents/disabled/com.example.old.plist")
        .exists());
    assert!(home_b.join(".claude/setup-as-code/RULES.md").exists());
    assert!(a.stdout.contains("launchctl bootstrap"), "{}", a.stdout);

    // import B into its own instance and compare the managed collections
    let p = sac(&["plan", "-i", i, "--home", hb, "--json"], root);
    assert_eq!(p.code, 0, "{}", p.out());
    let j: Value = serde_json::from_str(&p.stdout).unwrap();
    let mut offenders = Vec::new();
    for layer in j["layers"].as_array().unwrap() {
        for c in layer["changes"].as_array().unwrap() {
            let path = c["path"].as_str().unwrap();
            let id = c["id"].as_str().unwrap();
            let op = c["op"].as_str().unwrap();
            let fields: Vec<String> = c["fields"]
                .as_array()
                .map(|f| f.iter().map(|x| x.as_str().unwrap().to_string()).collect())
                .unwrap_or_default();
            let ok = match path {
                "tools.permissions.allow"
                | "tools.permissions.deny"
                | "tools.permissions.ask"
                | "tools.denied_mcp_servers"
                | "tools.hooks" => false,
                "tools.mcp_servers" => op == "+" && (id == "work-chat" || id.starts_with("notes@")),
                "loops" => {
                    let parked = ["com.example.old", "com.example.bak", "com.example.parked"];
                    if parked.contains(&id) {
                        op == "~" && fields.iter().all(|f| f == "status" || f == "files")
                    } else if id == "com.vendor.updater" {
                        op == "+"
                    } else {
                        false
                    }
                }
                _ => true,
            };
            if !ok {
                offenders.push(format!("{op} {path} {id} {fields:?}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "round trip drift: {offenders:#?}\n{}",
        p.stdout
    );

    // a second live apply over B keeps user values and backs up what it rewrites
    let settings_path = home_b.join(".claude/settings.json");
    let mut st: Value = serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    st["theme"] = Value::String("dark".into());
    st["env"] = serde_json::json!({ "KEEP_ME": "value" });
    st["permissions"]["allow"] = serde_json::json!(["Nothing"]);
    fs::write(&settings_path, serde_json::to_string_pretty(&st).unwrap()).unwrap();
    let again = sac(
        &[
            "apply",
            "-i",
            i,
            "--home",
            hb,
            "--out",
            out.to_str().unwrap(),
            "--live",
            "--yes",
        ],
        root,
    );
    assert_eq!(again.code, 0, "{}", again.out());
    let st2: Value = serde_json::from_str(&fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(st2["theme"], "dark");
    assert_eq!(st2["env"]["KEEP_ME"], "value");
    assert!(st2["permissions"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .any(|x| x == "Read"));
    assert!(
        home_b.join(".setup-as-code/backups").is_dir(),
        "backup dir missing"
    );
    assert!(again.stdout.contains("backups:"), "{}", again.stdout);
}

/// Not a test of behaviour: with SAC_FIXTURE_OUT set, writes the synthetic home and a ready
/// instance there so shell scripts (scripts/parity.sh) can use the same fixture.
/// Run: SAC_FIXTURE_OUT=/tmp/sac-fixture cargo test --test cli dump_fixture -- --ignored
#[test]
#[ignore]
fn dump_fixture_for_scripts() {
    let Some(out) = std::env::var_os("SAC_FIXTURE_OUT") else {
        return;
    };
    let out = std::path::PathBuf::from(out);
    let fx = build_home();
    let root = fx.tmp.path();
    let inst = init_instance(root, "alex");
    // the memory dir slug depends on the final home path, so rebuild it under the destination
    // never delete a directory this test did not write: refuse unless absent, empty, or ours
    if out.exists() {
        let ours = out.join("instance/alex").is_dir() && out.join("home-alex").is_dir();
        let empty = fs::read_dir(&out)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        assert!(
            ours || empty,
            "SAC_FIXTURE_OUT {} exists and is not a fixture dir",
            out.display()
        );
        if ours {
            fs::remove_dir_all(&out).unwrap();
        }
    }
    let dest_home = out.join("home-alex");
    common::copy_tree(&fx.home, &dest_home);
    let dest_home = dest_home.canonicalize().unwrap();
    let slug: String = dest_home
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let new_mem = dest_home.join(".claude/projects").join(slug).join("memory");
    fs::create_dir_all(new_mem.parent().unwrap()).unwrap();
    let old_mem = dest_home
        .join(".claude/projects")
        .join(fx.memory_dir.parent().unwrap().file_name().unwrap())
        .join("memory");
    if old_mem != new_mem {
        fs::rename(&old_mem, &new_mem).unwrap();
    }
    common::copy_tree(&inst, &out.join("instance/alex"));
    eprintln!("fixture written to {}", out.display());
}

#[test]
fn apply_refuses_secret_values_with_or_without_the_policy() {
    let fx = build_home();
    let root = fx.tmp.path();
    let inst = init_instance(root, "alex");
    let home = fx.home.to_str().unwrap();
    let i = inst.to_str().unwrap();
    assert_eq!(sac(&["import", "-i", i, "--home", home], root).code, 0);
    let token = ["xoxb-", "1234567890-abcdefghij"].concat();
    let yaml = fs::read_to_string(inst.join("setup.yaml")).unwrap();
    assert!(yaml.contains("\nvars:\n"), "{yaml}");
    let leaky = yaml.replacen("\nvars:\n", &format!("\nvars:\n  leak: \"{token}\"\n"), 1);
    fs::write(inst.join("setup.yaml"), leaky).unwrap();
    // with the template policies the hard secrets_by_reference check fails verify
    let v = sac(&["verify", "-i", i, "--home", home], root);
    assert_eq!(v.code, 1, "{}", v.out());
    assert!(v.stdout.contains("secrets-by-reference"), "{}", v.stdout);
    // and apply refuses even when no policy is declared at all
    fs::write(inst.join("policies.yaml"), "safety:\n  policies: []\n").unwrap();
    let out = root.join("out");
    let a = sac(
        &[
            "apply",
            "-i",
            i,
            "--home",
            home,
            "--out",
            out.to_str().unwrap(),
        ],
        root,
    );
    assert_eq!(a.code, 1, "{}", a.out());
    assert!(a.stdout.contains("secret-shaped"), "{}", a.out());
    assert!(!out.join("alex").exists(), "nothing may be rendered");
}
