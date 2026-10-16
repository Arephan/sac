//! The read-only machine context every importer goes through. It is what keeps `import`
//! safe: secret-looking files cannot be opened, only allowlisted read-only commands run, and
//! values that look like credentials never reach a manifest.

use crate::secrets::{find_secrets, is_secret_file, is_secret_key};
use crate::util::{expand_home, tilde};
use crate::Doc;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Ctx {
    home: PathBuf,
    is_real_home: bool,
    platform: &'static str,
    options: Doc,
    warnings: RefCell<Vec<String>>,
}

/// Platform name in the reference's vocabulary.
pub fn platform_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => Box::leak(other.to_string().into_boxed_str()),
    }
}

/// The real home directory of the current user.
pub fn real_home() -> PathBuf {
    home::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

fn allowed(cmd: &str, args: &[&str]) -> bool {
    let a = |i: usize| args.get(i).copied().unwrap_or("");
    match cmd {
        "launchctl" => matches!(a(0), "list" | "print-disabled"),
        "crontab" => args.len() == 1 && a(0) == "-l",
        "sw_vers" | "uname" => true,
        "git" => {
            a(0) == "-C"
                && matches!(a(2), "remote" | "rev-parse" | "log")
                && !args
                    .iter()
                    .any(|x| matches!(*x, "set-url" | "add" | "remove"))
        }
        "systemctl" => a(0) == "--user" && matches!(a(1), "list-timers" | "list-unit-files"),
        _ => false,
    }
}

impl Ctx {
    /// `home` overrides `$HOME` (a sandbox). `options` is the `import.options` map.
    pub fn new(home: Option<&Path>, options: Doc) -> Ctx {
        let real = real_home();
        let home = home
            .map(|h| std::fs::canonicalize(h).unwrap_or_else(|_| h.to_path_buf()))
            .unwrap_or_else(|| real.clone());
        let real_canon = std::fs::canonicalize(&real).unwrap_or(real);
        Ctx {
            is_real_home: home == real_canon,
            home,
            platform: platform_name(),
            options,
            warnings: RefCell::new(Vec::new()),
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }
    pub fn is_real_home(&self) -> bool {
        self.is_real_home
    }
    pub fn platform(&self) -> &str {
        self.platform
    }

    /// The options object for one importer (`import.options.<id>`), or `{}`.
    pub fn option(&self, importer: &str) -> Doc {
        self.options
            .get(importer)
            .cloned()
            .unwrap_or(Value::Object(Map::new()))
    }

    pub fn warn(&self, msg: impl Into<String>) {
        self.warnings.borrow_mut().push(msg.into());
    }
    pub fn warnings(&self) -> Vec<String> {
        self.warnings.borrow().clone()
    }

    /// Expand `~` and `$HOME` against this context's home.
    pub fn p(&self, rel: &str) -> PathBuf {
        PathBuf::from(expand_home(rel, &self.home))
    }
    pub fn tilde(&self, abs: &str) -> String {
        tilde(abs, &self.home)
    }
    pub fn exists(&self, rel: &str) -> bool {
        self.p(rel).exists()
    }
    pub fn is_dir(&self, rel: &str) -> bool {
        self.p(rel).is_dir()
    }
    /// Sorted entry names of a directory; empty when it cannot be read.
    pub fn list(&self, rel: &str) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(self.p(rel))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }
    /// Modification date of a path as `YYYY-MM-DD` (UTC).
    pub fn mtime_date(&self, rel: &str) -> Option<String> {
        let m = std::fs::metadata(self.p(rel)).ok()?.modified().ok()?;
        let dt: chrono::DateTime<chrono::Utc> = m.into();
        Some(dt.format("%Y-%m-%d").to_string())
    }
    /// Read a text file. Refuses names that look like credentials.
    pub fn read_text(&self, rel: &str) -> Result<String> {
        let abs = self.p(rel);
        let shown = self.tilde(&abs.to_string_lossy());
        if is_secret_file(&abs.to_string_lossy()) {
            bail!("refused to read secret-looking file {shown}");
        }
        let bytes = std::fs::read(&abs).with_context(|| shown)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
    pub fn read_json(&self, rel: &str) -> Result<Doc> {
        let text = self.read_text(rel)?;
        serde_json::from_str(&text).map_err(|e| anyhow!("{}: {e}", rel))
    }
    pub fn try_json(&self, rel: &str) -> Option<Doc> {
        self.read_json(rel).ok()
    }

    /// Run an allowlisted read-only command with a 15 second timeout. Anything else errors.
    pub fn exec(&self, cmd: &str, args: &[&str]) -> Result<String> {
        if !allowed(cmd, args) {
            bail!("exec not allowed: {cmd} {}", args.join(" "));
        }
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("spawn {cmd}"))?;
        let start = Instant::now();
        let mut stdout = child.stdout.take();
        let reader = std::thread::spawn(move || {
            let mut buf = String::new();
            if let Some(out) = stdout.as_mut() {
                use std::io::Read;
                let _ = out.read_to_string(&mut buf);
            }
            buf
        });
        loop {
            match child.try_wait()? {
                Some(status) => {
                    let out = reader.join().unwrap_or_default();
                    if !status.success() {
                        bail!("{cmd} exited with {status}");
                    }
                    return Ok(out);
                }
                None => {
                    if start.elapsed() > Duration::from_secs(15) {
                        let _ = child.kill();
                        let _ = child.wait();
                        bail!("{cmd} timed out");
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
    pub fn try_exec(&self, cmd: &str, args: &[&str]) -> Option<String> {
        self.exec(cmd, args).ok()
    }

    /// Sorted key names of an object, for `env` / `headers` style maps.
    pub fn names_only(&self, v: &Doc) -> Doc {
        match v {
            Value::Object(o) => {
                let mut keys: Vec<String> = o.keys().cloned().collect();
                keys.sort();
                Value::Array(keys.into_iter().map(Value::String).collect())
            }
            _ => Value::Array(Vec::new()),
        }
    }

    /// Make a value safe for a manifest: home paths become `~`, secret-shaped strings are
    /// dropped (with a warning), and keys like `env` or `headers` become `<key>_names`.
    pub fn scrub(&self, v: &Doc) -> Doc {
        match v {
            Value::String(s) => {
                let t = self.tilde(s);
                let hits = find_secrets(&t);
                if let Some(h) = hits.first() {
                    self.warn(format!("dropped a secret-looking value ({})", h.id));
                    Value::String("<redacted>".to_string())
                } else {
                    Value::String(t)
                }
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| self.scrub(x)).collect()),
            Value::Object(o) => {
                let mut out = Map::new();
                for (k, x) in o {
                    if is_secret_key(k) {
                        out.insert(format!("{}_names", k.to_lowercase()), self.names_only(x));
                    } else {
                        out.insert(k.clone(), self.scrub(x));
                    }
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }

    /// Numeric uid of the home directory's owner (unix only).
    #[cfg(unix)]
    pub fn uid(&self) -> Option<u32> {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(&self.home).ok().map(|m| m.uid())
    }
    #[cfg(not(unix))]
    pub fn uid(&self) -> Option<u32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sandbox_home_is_not_real_and_paths_expand() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(
            Some(tmp.path()),
            json!({ "launchd": { "managed_prefixes": ["com.example."] } }),
        );
        assert!(!ctx.is_real_home());
        assert_eq!(ctx.p("~/x"), tmp.path().canonicalize().unwrap().join("x"));
        assert_eq!(
            ctx.option("launchd")["managed_prefixes"][0],
            json!("com.example.")
        );
        assert_eq!(ctx.option("missing"), json!({}));
    }

    #[test]
    fn refuses_secret_files_and_disallowed_exec() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join(".bot_token"), "placeholder").unwrap();
        std::fs::write(tmp.path().join("settings.json"), "{\"a\":1}").unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        assert!(ctx.read_text("~/.bot_token").is_err());
        assert_eq!(ctx.read_json("~/settings.json").unwrap()["a"], json!(1));
        assert!(ctx.exec("launchctl", &["load", "x"]).is_err());
        assert!(ctx.exec("rm", &["-rf", "/"]).is_err());
        assert!(ctx
            .exec("git", &["-C", "/tmp", "remote", "set-url", "x"])
            .is_err());
        assert!(allowed(
            "git",
            &["-C", "/tmp", "remote", "get-url", "origin"]
        ));
    }

    #[test]
    fn scrub_hides_values_and_renames_secret_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        let home_s = tmp
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let tok = ["xoxb-", "1234567890-abcdefghij"].concat();
        let v = json!({ "command": format!("{home_s}/bin/x"), "env": { "B": "2", "A": "1" }, "note": tok });
        let out = ctx.scrub(&v);
        assert_eq!(out["command"], json!("~/bin/x"));
        assert_eq!(out["env_names"], json!(["A", "B"]));
        assert_eq!(out["note"], json!("<redacted>"));
        assert_eq!(ctx.warnings().len(), 1);
    }
}
