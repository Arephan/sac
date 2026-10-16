//! Executable checks found in `<instance>/checks/`: any executable file there is a check, in
//! any language. Hidden files (names starting with `.`) are skipped. Unix only for now: on
//! other platforms no external check is loaded.
//!
//! Protocol:
//! - `<exe> describe` prints `{"id": "...", "doc": "..."}`.
//! - `<exe> run` reads `{"setup": {...}, "params": {...}, "ctx": {"home": "...", "out_root":
//!   "..." or null}}` on stdin and prints `{"status": "pass|fail|warn|skip", "message": "...",
//!   "evidence": ["..."]}` on stdout, then exits 0.
//!
//! A check that cannot start, exits non-zero, runs longer than 20 seconds, or prints anything
//! but that one JSON object fails, with a message starting `check crashed:`. A `describe` that
//! fails is a load error, as a broken `.mjs` check was in the reference.

use super::{Check, CheckCtx, CheckResult, Status};
use crate::util::one_line;
use crate::Doc;
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long `describe` or `run` may take before the process is killed.
const TIMEOUT: Duration = Duration::from_secs(20);

struct ExternalCheck {
    path: PathBuf,
    id: String,
    doc: String,
    timeout: Duration,
}

impl ExternalCheck {
    /// Ask an executable to describe itself.
    fn load(path: &Path, timeout: Duration) -> Result<ExternalCheck> {
        let name = file_name(path);
        let out = run_exe(path, "describe", None, timeout)
            .map_err(|e| anyhow!("custom check {name}: describe failed: {e}"))?;
        let v: Value = serde_json::from_str(out.trim())
            .map_err(|e| anyhow!("custom check {name}: describe printed bad JSON ({e})"))?;
        let id = match v.get("id") {
            Some(Value::String(id)) if !id.is_empty() => id.clone(),
            _ => return Err(anyhow!("custom check {name}: describe printed no \"id\"")),
        };
        let doc = v
            .get("doc")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(ExternalCheck {
            path: path.to_path_buf(),
            id,
            doc,
            timeout,
        })
    }
}

impl Check for ExternalCheck {
    fn id(&self) -> &str {
        &self.id
    }
    fn doc(&self) -> &str {
        &self.doc
    }
    fn run(&self, setup: &Doc, params: &Doc, ctx: &CheckCtx) -> CheckResult {
        let input = json!({
            "setup": setup,
            "params": params,
            "ctx": {
                "home": ctx.home.to_string_lossy(),
                "out_root": ctx.out_root.as_ref().map(|p| p.to_string_lossy().into_owned()),
            },
        });
        let bytes = input.to_string().into_bytes();
        match run_exe(&self.path, "run", Some(bytes), self.timeout)
            .and_then(|out| parse_result(&out))
        {
            Ok(r) => r,
            Err(e) => CheckResult::fail(format!("check crashed: {e}"), Vec::new()),
        }
    }
}

/// Load every executable in `dir`, sorted by file name.
pub fn load_dir(dir: &Path) -> Result<Vec<Box<dyn Check>>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_executable(p))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|p| ExternalCheck::load(p, TIMEOUT).map(|c| Box::new(c) as Box<dyn Check>))
        .collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// A regular file (after following links) with an execute bit, not hidden.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    !file_name(path).starts_with('.')
        && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Plugins are unix-only for now: without an execute bit there is no way to tell a check from
/// a stray file such as a README, so nothing is loaded.
#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

/// The check's stdout parsed as one result object.
fn parse_result(stdout: &str) -> std::result::Result<CheckResult, String> {
    let v: Value =
        serde_json::from_str(stdout.trim()).map_err(|e| format!("bad JSON on stdout ({e})"))?;
    let Value::Object(o) = v else {
        return Err("stdout is not a JSON object".to_string());
    };
    let status = match o.get("status") {
        Some(Value::String(s)) => {
            Status::parse(s).ok_or_else(|| format!("bad status {}", Value::String(s.clone())))?
        }
        Some(other) => return Err(format!("bad status {other}")),
        None => return Err("no status".to_string()),
    };
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let message = match o.get("message") {
        None | Some(Value::Null) => String::new(),
        Some(m) => text(m),
    };
    let evidence = match o.get("evidence") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.iter().map(text).collect(),
        Some(other) => vec![text(other)],
    };
    Ok(CheckResult {
        status,
        message,
        evidence,
    })
}

/// Start `<path> <arg>`, retrying briefly on ETXTBSY: an executable that another thread of
/// this process has just written can be busy for a moment after it is closed.
fn spawn(path: &Path, arg: &str, piped_stdin: bool) -> std::result::Result<Child, String> {
    const ETXTBSY: i32 = 26;
    let mut tries = 0;
    loop {
        let r = Command::new(path)
            .arg(arg)
            .stdin(if piped_stdin {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        match r {
            Ok(child) => return Ok(child),
            Err(e) if cfg!(unix) && e.raw_os_error() == Some(ETXTBSY) && tries < 10 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(format!("cannot start {}: {e}", file_name(path))),
        }
    }
}

fn exit_text(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "killed by a signal".to_string(),
    }
}

/// Run `<path> <arg>` with `input` on stdin and return its stdout. A start failure, a non-zero
/// exit (with the last line of stderr) or the timeout is an error.
fn run_exe(
    path: &Path,
    arg: &str,
    input: Option<Vec<u8>>,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let deadline = Instant::now() + timeout;
    let mut child = spawn(path, arg, input.is_some())?;
    // Feed stdin and drain stdout and stderr on their own threads, so a child that writes a lot
    // before it reads cannot deadlock against us.
    if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        });
    }
    let (tx, rx) = mpsc::channel::<(bool, Vec<u8>)>();
    let mut pipes = 0;
    if let Some(mut out) = child.stdout.take() {
        let tx = tx.clone();
        pipes += 1;
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = out.read_to_end(&mut buf);
            let _ = tx.send((true, buf));
        });
    }
    if let Some(mut err) = child.stderr.take() {
        let tx = tx.clone();
        pipes += 1;
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = err.read_to_end(&mut buf);
            let _ = tx.send((false, buf));
        });
    }
    drop(tx);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {timeout:?}"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("wait failed: {e}"));
            }
        }
    };
    // The pipes close when the child exits, unless something it started still holds them.
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    for _ in 0..pipes {
        let left = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(200));
        match rx.recv_timeout(left) {
            Ok((true, b)) => stdout = b,
            Ok((false, b)) => stderr = b,
            Err(_) => break,
        }
    }
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        let last = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| one_line(l, 200))
            .unwrap_or_default();
        return Err(if last.is_empty() {
            exit_text(status)
        } else {
            format!("{}: {last}", exit_text(status))
        });
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{load_checks, run_policies, Status};
    use serde_json::json;

    fn ctx() -> CheckCtx {
        CheckCtx {
            home: PathBuf::from("/home/user"),
            out_root: Some(PathBuf::from("/home/user/proj/out")),
        }
    }

    fn loops_setup() -> Doc {
        json!({ "loops": [
            { "id": "a", "runner": "cron", "enabled": true },
            { "id": "b", "runner": "cron", "enabled": true },
            { "id": "c", "runner": "cron", "enabled": false }
        ] })
    }

    #[test]
    fn default_timeout_is_twenty_seconds() {
        assert_eq!(TIMEOUT, Duration::from_secs(20));
    }

    #[test]
    fn a_result_is_one_object_with_a_known_status() {
        let err = |stdout: &str| parse_result(stdout).err().unwrap_or_default();
        assert_eq!(err("[1, 2]"), "stdout is not a JSON object");
        assert_eq!(err("\"pass\""), "stdout is not a JSON object");
        assert_eq!(err(r#"{"message": "x", "evidence": []}"#), "no status");
        assert_eq!(err(r#"{"status": null}"#), "bad status null");
        assert_eq!(err(r#"{"status": "PASS"}"#), "bad status \"PASS\"");
        assert!(err("").starts_with("bad JSON on stdout"));
        assert!(err("{\"status\":\"pass\"}\n{}").starts_with("bad JSON on stdout"));

        let r = parse_result(" {\"status\":\"warn\",\"message\":\"m\",\"evidence\":[\"a\",2]}\n")
            .unwrap();
        assert_eq!(r.status, Status::Warn);
        assert_eq!(r.message, "m");
        assert_eq!(r.evidence, ["a", "2"]);
        let r = parse_result(r#"{"status":"skip"}"#).unwrap();
        assert_eq!(r.status, Status::Skip);
        assert!(r.message.is_empty() && r.evidence.is_empty());
    }

    #[test]
    fn unknown_check_id_fails() {
        let mut setup = loops_setup();
        setup["safety"] = json!({ "policies": [
            { "id": "typo", "check": "x", "severity": "standing" }
        ] });
        let report = run_policies(&setup, &crate::policy::builtin_checks(), &ctx());
        assert_eq!(report.results.len(), 1);
        let r = &report.results[0];
        assert_eq!(r.status, Status::Fail);
        assert_eq!(r.message, "unknown check \"x\"");
        assert!(r.evidence.is_empty());
        assert!(report.ok, "a standing failure does not fail the report");
        assert_eq!(report.soft_fails().len(), 1);
    }

    #[cfg(unix)]
    mod unix {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;

        /// The example `max_enabled_loops` check, plus modes that misbehave on purpose.
        const LOOPS_CHECK: &str = r#"#!/usr/bin/env python3
import json, sys, time
if sys.argv[1:] == ["describe"]:
    print(json.dumps({"id": "max_enabled_loops", "doc": "At most params.max loops are enabled at once."}))
    sys.exit(0)
req = json.load(sys.stdin)
params = req.get("params") or {}
mode = params.get("mode")
if mode == "crash":
    raise ValueError("boom")
if mode == "garbage":
    print("not json")
    sys.exit(0)
if mode == "status":
    print(json.dumps({"status": "maybe", "message": "?"}))
    sys.exit(0)
if mode == "array":
    print(json.dumps([1, 2]))
    sys.exit(0)
if mode == "nostatus":
    print(json.dumps({"message": "no status here", "evidence": []}))
    sys.exit(0)
if mode == "sleep":
    time.sleep(30)
if mode == "ctx":
    ctx = req["ctx"]
    keys = ",".join(sorted(req))
    print(json.dumps({"status": "warn", "message": "ctx", "evidence": [ctx["home"], str(ctx["out_root"]), keys]}))
    sys.exit(0)
limit = params.get("max", 10)
on = [l["id"] for l in req["setup"].get("loops", []) if l.get("enabled")]
if len(on) > limit:
    print(json.dumps({"status": "fail", "message": f"{len(on)} enabled loops > {limit}", "evidence": on}))
else:
    print(json.dumps({"status": "pass", "message": f"{len(on)} enabled loop(s) <= {limit}", "evidence": []}))
"#;

        /// A shell check that describes itself with `id` and always passes.
        fn sh_check(id: &str, doc: &str) -> String {
            format!(
                "#!/bin/sh\nif [ \"$1\" = describe ]; then\n  echo '{{\"id\":\"{id}\",\"doc\":\"{doc}\"}}'\nelse\n  cat >/dev/null\n  echo '{{\"status\":\"pass\",\"message\":\"ok\",\"evidence\":[]}}'\nfi\n"
            )
        }

        fn write_exe(dir: &Path, name: &str, body: &str) -> PathBuf {
            let p = dir.join(name);
            std::fs::write(&p, body).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        }

        fn loops_check(dir: &Path) -> PathBuf {
            write_exe(dir, "max-enabled-loops", LOOPS_CHECK)
        }

        #[test]
        fn python_check_describes_itself_and_runs() {
            let tmp = tempfile::tempdir().unwrap();
            loops_check(tmp.path());
            let checks = load_dir(tmp.path()).unwrap();
            assert_eq!(checks.len(), 1);
            let c = &checks[0];
            assert_eq!(c.id(), "max_enabled_loops");
            assert_eq!(c.doc(), "At most params.max loops are enabled at once.");
            let r = c.run(&loops_setup(), &json!({ "max": 1 }), &ctx());
            assert_eq!(r.status, Status::Fail);
            assert_eq!(r.message, "2 enabled loops > 1");
            assert_eq!(r.evidence, ["a", "b"]);
            let r = c.run(&loops_setup(), &json!({}), &ctx());
            assert_eq!(r.status, Status::Pass);
            assert_eq!(r.message, "2 enabled loop(s) <= 10");
            assert!(r.evidence.is_empty());
        }

        #[test]
        fn check_reads_setup_params_and_ctx_on_stdin() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ExternalCheck::load(&loops_check(tmp.path()), TIMEOUT).unwrap();
            let r = c.run(&loops_setup(), &json!({ "mode": "ctx" }), &ctx());
            assert_eq!(r.status, Status::Warn);
            assert_eq!(
                r.evidence,
                ["/home/user", "/home/user/proj/out", "ctx,params,setup"]
            );
            let no_out = CheckCtx {
                home: PathBuf::from("/home/user"),
                out_root: None,
            };
            let r = c.run(&loops_setup(), &json!({ "mode": "ctx" }), &no_out);
            assert_eq!(r.evidence[1], "None");
        }

        #[test]
        fn crash_bad_json_and_bad_status_become_fail() {
            let tmp = tempfile::tempdir().unwrap();
            let c = ExternalCheck::load(&loops_check(tmp.path()), TIMEOUT).unwrap();
            let run = |mode: &str| c.run(&loops_setup(), &json!({ "mode": mode }), &ctx());

            let r = run("crash");
            assert_eq!(r.status, Status::Fail);
            assert_eq!(r.message, "check crashed: exit code 1: ValueError: boom");
            assert!(r.evidence.is_empty());

            let r = run("garbage");
            assert_eq!(r.status, Status::Fail);
            assert!(
                r.message.starts_with("check crashed: bad JSON on stdout"),
                "{}",
                r.message
            );

            let r = run("status");
            assert_eq!(r.status, Status::Fail);
            assert_eq!(r.message, "check crashed: bad status \"maybe\"");

            let r = run("array");
            assert_eq!(r.status, Status::Fail);
            assert_eq!(r.message, "check crashed: stdout is not a JSON object");

            let r = run("nostatus");
            assert_eq!(r.status, Status::Fail);
            assert_eq!(r.message, "check crashed: no status");
        }

        #[test]
        fn check_killed_by_a_signal_becomes_fail() {
            let tmp = tempfile::tempdir().unwrap();
            let body = "#!/bin/sh\nif [ \"$1\" = describe ]; then\n  echo '{\"id\":\"sig\",\"doc\":\"Dies.\"}'\nelse\n  cat >/dev/null\n  kill -KILL $$\nfi\n";
            let c = ExternalCheck::load(&write_exe(tmp.path(), "sig", body), TIMEOUT).unwrap();
            let r = c.run(&loops_setup(), &json!({}), &ctx());
            assert_eq!(r.status, Status::Fail);
            assert_eq!(r.message, "check crashed: killed by a signal");
        }

        #[test]
        fn slow_check_is_killed_at_the_timeout() {
            let tmp = tempfile::tempdir().unwrap();
            let mut c = ExternalCheck::load(&loops_check(tmp.path()), TIMEOUT).unwrap();
            c.timeout = Duration::from_millis(500);
            let start = Instant::now();
            let r = c.run(&loops_setup(), &json!({ "mode": "sleep" }), &ctx());
            assert!(start.elapsed() < Duration::from_secs(10));
            assert_eq!(r.status, Status::Fail);
            assert_eq!(r.message, "check crashed: timed out after 500ms");
        }

        #[test]
        fn missing_executable_becomes_fail() {
            let tmp = tempfile::tempdir().unwrap();
            let path = write_exe(tmp.path(), "gone", &sh_check("gone", "Gone."));
            let c = ExternalCheck::load(&path, TIMEOUT).unwrap();
            std::fs::remove_file(&path).unwrap();
            let r = c.run(&loops_setup(), &json!({}), &ctx());
            assert_eq!(r.status, Status::Fail);
            assert!(
                r.message.starts_with("check crashed: cannot start gone:"),
                "{}",
                r.message
            );
        }

        #[test]
        fn load_dir_takes_only_executables_sorted_by_name() {
            let tmp = tempfile::tempdir().unwrap();
            let d = tmp.path();
            write_exe(d, "b-check", &sh_check("b_check", "B."));
            write_exe(d, "a-check", &sh_check("a_check", "A."));
            write_exe(d, ".hidden", &sh_check("hidden", "Hidden."));
            std::fs::write(d.join("README.md"), "not a check").unwrap();
            std::fs::write(d.join("old.mjs"), "export default {}").unwrap();
            std::fs::create_dir(d.join("sub")).unwrap();
            let ids: Vec<String> = load_dir(d)
                .unwrap()
                .iter()
                .map(|c| c.id().to_string())
                .collect();
            assert_eq!(ids, ["a_check", "b_check"]);
        }

        #[test]
        fn bad_describe_is_a_load_error() {
            let tmp = tempfile::tempdir().unwrap();
            write_exe(tmp.path(), "no-id", "#!/bin/sh\necho '{\"doc\":\"x\"}'\n");
            let err = load_dir(tmp.path()).err().expect("an error").to_string();
            assert_eq!(err, "custom check no-id: describe printed no \"id\"");

            let tmp = tempfile::tempdir().unwrap();
            write_exe(
                tmp.path(),
                "broken",
                "#!/bin/sh\necho 'cannot describe' >&2\nexit 3\n",
            );
            let err = load_dir(tmp.path()).err().expect("an error").to_string();
            assert_eq!(
                err,
                "custom check broken: describe failed: exit code 3: cannot describe"
            );
        }

        #[test]
        fn load_checks_appends_externals_and_replaces_builtins_by_id() {
            let tmp = tempfile::tempdir().unwrap();
            let checks_dir = tmp.path().join("checks");
            std::fs::create_dir(&checks_dir).unwrap();
            loops_check(&checks_dir);
            write_exe(
                &checks_dir,
                "deny-override",
                &sh_check("deny_mcp", "Replaced."),
            );
            let list = load_checks(Some(tmp.path())).unwrap();
            let ids: Vec<&str> = list.iter().map(|c| c.id()).collect();
            assert_eq!(
                ids,
                [
                    "deny_mcp",
                    "no_payment_tools",
                    "forbidden_paths",
                    "required_gate",
                    "channel_identity",
                    "secrets_by_reference",
                    "loop_success_metric",
                    "required_rules",
                    "forbidden_accounts_unused",
                    "max_enabled_loops"
                ]
            );
            assert_eq!(list[0].doc(), "Replaced.");
            assert_eq!(load_checks(None).unwrap().len(), 9);
        }

        #[test]
        fn run_policies_runs_external_checks() {
            let tmp = tempfile::tempdir().unwrap();
            let checks_dir = tmp.path().join("checks");
            std::fs::create_dir(&checks_dir).unwrap();
            loops_check(&checks_dir);
            let checks = load_checks(Some(tmp.path())).unwrap();
            let mut setup = loops_setup();
            setup["safety"] = json!({ "policies": [
                { "id": "few-loops", "check": "max_enabled_loops", "severity": "hard", "params": { "max": 1 } },
                { "id": "typo", "check": "x", "severity": "standing" }
            ] });
            let report = run_policies(&setup, &checks, &ctx());
            let r = &report.results[0];
            assert_eq!((r.id.as_str(), r.status), ("few-loops", Status::Fail));
            assert_eq!(r.message, "2 enabled loops > 1");
            assert_eq!(report.results[1].message, "unknown check \"x\"");
            assert!(!report.ok);
            assert_eq!(report.hard_fails().len(), 1);
            assert_eq!(report.soft_fails().len(), 1);
        }
    }
}
