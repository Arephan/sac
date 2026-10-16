//! Executable importer plugins. Any executable file in `<instance>/importers/` is an importer
//! that speaks this protocol:
//!
//! - `<exe> describe` prints `{"id":"todo-md","layers":["channels"],"description":"..."}`.
//! - `<exe> detect` exits 0 when the importer applies to this machine.
//! - `<exe> run` reads the importer's options as JSON on stdin and prints a JSON fragment of
//!   `setup.yaml` on stdout.
//!
//! `detect` and `run` get `SAC_HOME` (the home being imported) and `SAC_REAL_HOME` (`1` or `0`)
//! in their environment. Every call is killed after 20 seconds. A fragment goes through
//! `Ctx::scrub` like built-in output, so home paths become `~` and credential values never
//! reach a manifest.

use super::Importer;
use crate::ctx::Ctx;
use crate::util::{arr, s, str_list};
use crate::Doc;
use anyhow::{anyhow, bail, Context, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(20);

pub struct ExternalImporter {
    path: PathBuf,
    id: String,
    layers: Vec<String>,
    description: String,
    timeout: Duration,
}

/// What a finished plugin call left behind.
struct Finished {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Spawn, retrying briefly on "text file busy": a script written a moment ago can still be
/// held open for writing by a fork happening on another thread.
fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    const ETXTBSY: i32 = 26;
    let mut tries = 0;
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(ETXTBSY) && tries < 50 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            other => return other,
        }
    }
}

/// Read a pipe to the end on a helper thread.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    rx
}

fn human(d: Duration) -> String {
    if d.as_millis() < 1000 {
        format!("{}ms", d.as_millis())
    } else {
        format!("{}s", d.as_secs())
    }
}

/// Run `<exe> <verb>` with a deadline. Output is read on helper threads so a chatty plugin
/// cannot stall on a full pipe; a plugin still running at the deadline is killed.
fn call(
    exe: &Path,
    verb: &str,
    env: &[(&str, String)],
    input: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<Finished> {
    let mut cmd = Command::new(exe);
    cmd.arg(verb)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = spawn(&mut cmd).with_context(|| format!("cannot start {}", exe.display()))?;
    if let (Some(mut stdin), Some(data)) = (child.stdin.take(), input) {
        std::thread::spawn(move || {
            let _ = stdin.write_all(&data);
        });
    }
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{verb} timed out after {}", human(timeout));
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    // A grandchild may still hold the pipes open; do not wait on it past the deadline.
    let left = || {
        deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(200))
    };
    let stdout = out.recv_timeout(left()).unwrap_or_default();
    let stderr = err.recv_timeout(left()).unwrap_or_default();
    Ok(Finished {
        status,
        stdout,
        stderr,
    })
}

fn exit_text(st: ExitStatus) -> String {
    match st.code() {
        Some(c) => format!("code {c}"),
        None => "a signal".into(),
    }
}

/// `": <last part of stderr on one line>"`, or nothing when stderr is empty.
fn stderr_tail(stderr: &str) -> String {
    let words: Vec<&str> = stderr.split_whitespace().collect();
    let line = words.join(" ");
    let n = line.chars().count();
    if n == 0 {
        String::new()
    } else if n > 300 {
        format!(": …{}", line.chars().skip(n - 299).collect::<String>())
    } else {
        format!(": {line}")
    }
}

impl ExternalImporter {
    /// Ask `<path> describe` who the plugin is, and check the answer.
    pub fn load(path: &Path) -> Result<Self> {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let fail = |msg: String| anyhow!("importer {name}: {msg}");
        let out = call(&path, "describe", &[], None, TIMEOUT).map_err(|e| fail(e.to_string()))?;
        if !out.status.success() {
            return Err(fail(format!(
                "describe exited with {}{}",
                exit_text(out.status),
                stderr_tail(&out.stderr)
            )));
        }
        let doc: Doc = serde_json::from_str(out.stdout.trim())
            .map_err(|e| fail(format!("describe did not print JSON ({e})")))?;
        let mut problems = Vec::new();
        if !doc.is_object() {
            problems.push("describe must print a JSON object");
        } else {
            let id = s(doc.get("id"));
            if id.is_empty()
                || !id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                problems.push("id must be kebab-case");
            }
            let layers = arr(doc.get("layers"));
            if layers.is_empty() || !layers.iter().all(|l| l.is_string()) {
                problems.push("layers must be a non-empty array");
            }
        }
        if !problems.is_empty() {
            return Err(fail(problems.join("; ")));
        }
        Ok(ExternalImporter {
            id: s(doc.get("id")).to_string(),
            layers: str_list(doc.get("layers")),
            description: s(doc.get("description")).to_string(),
            path,
            timeout: TIMEOUT,
        })
    }

    #[cfg(test)]
    fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    fn env(ctx: &Ctx) -> Vec<(&'static str, String)> {
        vec![
            ("SAC_HOME", ctx.home().to_string_lossy().into_owned()),
            (
                "SAC_REAL_HOME",
                if ctx.is_real_home() { "1" } else { "0" }.to_string(),
            ),
        ]
    }
}

impl Importer for ExternalImporter {
    fn id(&self) -> &str {
        &self.id
    }
    fn layers(&self) -> Vec<&str> {
        self.layers.iter().map(String::as_str).collect()
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn detect(&self, ctx: &Ctx) -> bool {
        call(&self.path, "detect", &Self::env(ctx), None, self.timeout)
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    fn run(&self, ctx: &Ctx, opts: &Doc) -> Result<Doc> {
        let input = serde_json::to_vec(opts)?;
        let out = call(
            &self.path,
            "run",
            &Self::env(ctx),
            Some(input),
            self.timeout,
        )?;
        if !out.status.success() {
            bail!(
                "run exited with {}{}",
                exit_text(out.status),
                stderr_tail(&out.stderr)
            );
        }
        let frag: Doc = serde_json::from_str(out.stdout.trim())
            .map_err(|e| anyhow!("run did not print JSON ({e})"))?;
        if !frag.is_object() {
            bail!("run must print a JSON object");
        }
        Ok(ctx.scrub(&frag))
    }
}

/// Load every executable in `dir`, sorted by file name. Dotfiles and editor backups (`name~`)
/// are skipped.
pub fn load_dir(dir: &Path) -> Result<Vec<Box<dyn Importer>>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            !n.starts_with('.') && !n.ends_with('~')
        })
        .map(|e| e.path())
        .filter(|p| is_executable_file(p))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|p| ExternalImporter::load(p).map(|i| Box::new(i) as Box<dyn Importer>))
        .collect()
}

/// A regular file (after following symlinks) with an execute bit; any regular file off unix.
pub(crate) fn is_executable_file(p: &Path) -> bool {
    match std::fs::metadata(p) {
        Ok(m) if m.is_file() => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                true
            }
        }
        _ => false,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::importers::{builtin, load_importers};
    use serde_json::json;
    use std::time::Instant;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// A plugin that answers `describe` and does nothing else.
    fn describing(id: &str, description: &str) -> String {
        format!(
            "case \"$1\" in\n  describe) echo '{{\"id\":\"{id}\",\"layers\":[\"loops\"],\"description\":\"{description}\"}}' ;;\n  detect) exit 0 ;;\n  run) echo '{{}}' ;;\nesac\n"
        )
    }

    /// A plugin whose `run` verb is the given shell code.
    fn running(run: &str) -> String {
        format!(
            "case \"$1\" in\n  describe) echo '{{\"id\":\"x\",\"layers\":[\"loops\"]}}' ;;\n  run) {run} ;;\nesac\n"
        )
    }

    const TODO: &str = r#"case "$1" in
  describe) echo '{"id":"todo-md","layers":["channels"],"description":"Todo file as a channel."}' ;;
  detect) [ "$SAC_REAL_HOME" = "0" ] && [ -f "$SAC_HOME/todo.md" ] ;;
  run) input=$(cat); printf '{"channels":{"todo":[{"id":"todo","path":"%s/todo.md","options":%s}]}}\n' "$SAC_HOME" "$input" ;;
  *) exit 2 ;;
esac
"#;

    #[test]
    fn shell_plugin_describes_detects_and_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let exe = script(&tmp.path().join("plugins"), "todo-md", TODO);
        let imp = ExternalImporter::load(&exe).unwrap();
        assert_eq!(imp.id(), "todo-md");
        assert_eq!(imp.layers(), vec!["channels"]);
        assert_eq!(imp.description(), "Todo file as a channel.");
        let ctx = Ctx::new(Some(&home), json!({}));
        assert!(!imp.detect(&ctx));
        std::fs::write(home.join("todo.md"), "- [ ] one\n").unwrap();
        assert!(imp.detect(&ctx));
        let out = imp.run(&ctx, &json!({ "max": 3 })).unwrap();
        assert_eq!(
            out,
            json!({ "channels": { "todo": [
                { "id": "todo", "path": "~/todo.md", "options": { "max": 3 } }
            ] } })
        );
    }

    #[test]
    fn load_importers_appends_new_ids_and_replaces_builtins() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("importers");
        script(
            &dir,
            "a-cron",
            &describing("cron", "Crontab from a plugin."),
        );
        script(&dir, "c-extra", &describing("extra-thing", "second"));
        script(&dir, "b-extra", &describing("extra-thing", "first"));
        script(&dir, ".hidden", &describing("hidden-thing", "dotfile"));
        script(
            &dir,
            "c-extra~",
            &describing("backup-thing", "editor backup"),
        );
        std::fs::write(dir.join("notes.txt"), "not a plugin\n").unwrap();

        let list = load_importers(Some(tmp.path())).unwrap();
        let mut want: Vec<String> = builtin().iter().map(|i| i.id().to_string()).collect();
        want.push("extra-thing".into());
        let got: Vec<String> = list.iter().map(|i| i.id().to_string()).collect();
        assert_eq!(got, want);
        let find = |id: &str| list.iter().find(|i| i.id() == id).unwrap();
        assert_eq!(find("cron").description(), "Crontab from a plugin.");
        assert_eq!(find("extra-thing").description(), "second");

        let ids: Vec<String> = load_dir(&dir)
            .unwrap()
            .iter()
            .map(|i| i.description().to_string())
            .collect();
        assert_eq!(ids, ["Crontab from a plugin.", "first", "second"]);
        assert_eq!(load_importers(None).unwrap().len(), builtin().len());
    }

    #[test]
    fn invalid_plugins_fail_to_load() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let err = |exe: PathBuf| ExternalImporter::load(&exe).err().unwrap().to_string();
        assert_eq!(
            err(script(
                d,
                "bad",
                "echo '{\"id\":\"Bad_Id\",\"layers\":[]}'\n"
            )),
            "importer bad: id must be kebab-case; layers must be a non-empty array"
        );
        assert!(err(script(d, "not-json", "echo hello\n"))
            .starts_with("importer not-json: describe did not print JSON"));
        assert_eq!(
            err(script(d, "fails", "echo oops >&2\nexit 3\n")),
            "importer fails: describe exited with code 3: oops"
        );
        assert_eq!(
            err(script(d, "arr", "echo '[1]'\n")),
            "importer arr: describe must print a JSON object"
        );
        let dir = d.join("importers");
        script(&dir, "no-layers", "echo '{\"id\":\"ok-id\"}'\n");
        assert_eq!(
            load_dir(&dir).err().unwrap().to_string(),
            "importer no-layers: layers must be a non-empty array"
        );
    }

    #[test]
    fn run_failures_are_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        let run_err = |name: &str, run: &str| {
            let exe = script(tmp.path(), name, &running(run));
            let imp = ExternalImporter::load(&exe).unwrap();
            imp.run(&ctx, &json!({})).err().unwrap().to_string()
        };
        assert_eq!(
            run_err("boom", "echo boom >&2; exit 1"),
            "run exited with code 1: boom"
        );
        assert_eq!(
            run_err("list", "echo '[1,2]'"),
            "run must print a JSON object"
        );
        assert!(run_err("text", "echo done").starts_with("run did not print JSON"));
    }

    #[test]
    fn plugin_output_is_scrubbed() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        let exe = script(
            tmp.path(),
            "leaky",
            &running(
                "printf '{\"loops\":[{\"id\":\"x\",\"runner\":\"manual\",\"enabled\":false,\"log\":\"%s/x.log\",\"env\":{\"API_KEY\":\"value-never-copied\"}}]}' \"$SAC_HOME\"",
            ),
        );
        let out = ExternalImporter::load(&exe)
            .unwrap()
            .run(&ctx, &json!({}))
            .unwrap();
        assert_eq!(
            out,
            json!({ "loops": [{
                "id": "x", "runner": "manual", "enabled": false, "log": "~/x.log",
                "env_names": ["API_KEY"]
            }] })
        );
    }

    #[test]
    fn slow_plugins_are_killed_at_the_deadline() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        let exe = script(
            tmp.path(),
            "slow",
            "case \"$1\" in\n  describe) echo '{\"id\":\"slow\",\"layers\":[\"loops\"]}' ;;\n  *) sleep 5 ;;\nesac\n",
        );
        let imp = ExternalImporter::load(&exe)
            .unwrap()
            .with_timeout(Duration::from_millis(300));
        let start = Instant::now();
        let err = imp.run(&ctx, &json!({})).err().unwrap().to_string();
        assert_eq!(err, "run timed out after 300ms");
        assert!(!imp.detect(&ctx));
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "{:?}",
            start.elapsed()
        );
    }
}
