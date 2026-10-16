//! Machine facts, which CLIs exist, and git repos directly under the home dir. Everything is
//! read in-process: `PATH` is scanned instead of spawning `which`, and each repo's
//! `.git/config` is read instead of spawning `git`.
//!
//! `options.clis`: extra CLI names to probe. `options.repo_roots`: dirs to scan (default
//! `["~"]`). `options.include_hidden`: also scan dot-directories.

use super::external::is_executable_file;
use super::Importer;
use crate::ctx::Ctx;
use crate::util::{is_truthy, str_list};
use crate::Doc;
use anyhow::Result;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::OnceLock;

const DEFAULT_CLIS: [&str; 20] = [
    "claude",
    "node",
    "npm",
    "python3",
    "git",
    "gh",
    "brew",
    "docker",
    "codex",
    "gemini",
    "wrangler",
    "gcloud",
    "firebase",
    "eas",
    "xcodebuild",
    "ffmpeg",
    "rtk",
    "uv",
    "bun",
    "deno",
];

const SYSTEM_VERSION: &str = "/System/Library/CoreServices/SystemVersion.plist";

pub struct Infra;

/// CPU architecture in Node's `process.arch` vocabulary.
fn node_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        "x86" => "ia32",
        "powerpc64" => "ppc64",
        "loongarch64" => "loong64",
        other => other,
    }
}

/// `PRETTY_NAME` from `/etc/os-release` text, unquoted.
fn pretty_name(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let v = line.trim().strip_prefix("PRETTY_NAME=")?.trim();
        let v = ['"', '\'']
            .iter()
            .find_map(|q| v.strip_prefix(*q)?.strip_suffix(*q))
            .unwrap_or(v);
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// The OS version: the macOS product version, a Linux distribution's pretty name, or
/// `uname -sr`; empty when none can be read.
fn os_version(ctx: &Ctx) -> String {
    let uname = || {
        ctx.try_exec("uname", &["-sr"])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let v = match ctx.platform() {
        "darwin" => ctx
            .read_text(SYSTEM_VERSION)
            .ok()
            .and_then(|t| plist::Value::from_reader(std::io::Cursor::new(t.into_bytes())).ok())
            .and_then(|v| {
                v.as_dictionary()?
                    .get("ProductVersion")?
                    .as_string()
                    .map(str::to_string)
            })
            .or_else(|| {
                ctx.try_exec("sw_vers", &["-productVersion"])
                    .map(|s| s.trim().to_string())
            }),
        "linux" => ctx
            .read_text("/etc/os-release")
            .ok()
            .and_then(|t| pretty_name(&t))
            .or_else(uname),
        _ => uname(),
    };
    v.unwrap_or_default()
}

/// What `which <name>` prints for this `PATH`: the first executable file named `name`.
fn which_in(name: &str, path_var: &OsStr) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if name.contains('/') {
        let p = PathBuf::from(name);
        return is_executable_file(&p).then_some(p);
    }
    std::env::split_paths(path_var)
        .filter(|d| !d.as_os_str().is_empty())
        .map(|d| d.join(name))
        .find(|p| is_executable_file(p))
}

/// `[remote "origin"]` (or legacy `[remote.origin]`) section header contents.
fn is_origin_section(header: &str) -> bool {
    let h = header.trim();
    match h.split_once(|c: char| c.is_ascii_whitespace()) {
        Some((section, sub)) => {
            section.eq_ignore_ascii_case("remote") && sub.trim() == "\"origin\""
        }
        None => h.eq_ignore_ascii_case("remote.origin"),
    }
}

/// A git config value: quotes removed, escapes applied, a trailing `#`/`;` comment dropped.
fn config_value(raw: &str) -> String {
    let mut out = String::new();
    let mut quoted = false;
    let mut chars = raw.trim().chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('b') => {
                    out.pop();
                }
                Some(x) => out.push(x),
                None => {}
            },
            '#' | ';' if !quoted => break,
            c => out.push(c),
        }
    }
    out.trim().to_string()
}

/// The first `url` of the `origin` remote in a `.git/config`, as `git remote get-url origin`
/// prints it (without `insteadOf` rewrites).
fn origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for raw in config.lines() {
        let mut line = raw.trim();
        if line.starts_with('[') {
            let Some(end) = line.find(']') else {
                in_origin = false;
                continue;
            };
            in_origin = is_origin_section(&line[1..end]);
            line = line[end + 1..].trim();
        }
        if !in_origin || line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("url") {
            let v = config_value(value);
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Drop `user:password@` after the scheme's `//`.
fn strip_credentials(url: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"//[^@/]+@").expect("regex compiles"));
    re.replace(url, "//").into_owned()
}

/// The importer with an explicit `PATH` (tests pass their own).
fn run_with_path(ctx: &Ctx, opts: &Doc, path_var: Option<&OsStr>) -> Result<Doc> {
    let machine = json!({ "platform": ctx.platform(), "arch": node_arch(), "os": os_version(ctx) });

    let mut names: Vec<String> = DEFAULT_CLIS.iter().map(|s| s.to_string()).collect();
    for n in str_list(opts.get("clis")) {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let clis: Vec<Doc> = names
        .into_iter()
        .map(|name| {
            let found = path_var.and_then(|pv| which_in(&name, pv));
            let mut m = Map::new();
            m.insert("id".into(), json!(name));
            m.insert("present".into(), json!(found.is_some()));
            if let Some(p) = found {
                m.insert("path".into(), json!(ctx.tilde(&p.to_string_lossy())));
            }
            Value::Object(m)
        })
        .collect();

    let roots: Vec<String> = match opts.get("repo_roots") {
        v if !is_truthy(v) => vec!["~".to_string()],
        Some(Value::String(r)) => vec![r.clone()],
        v => str_list(v),
    };
    let include_hidden = is_truthy(opts.get("include_hidden"));
    let mut repos = Vec::new();
    for root in &roots {
        for d in ctx.list(root) {
            if d.starts_with('.') && !include_hidden {
                continue;
            }
            let rel = if root.ends_with('/') {
                format!("{root}{d}")
            } else {
                format!("{root}/{d}")
            };
            let git = format!("{rel}/.git");
            if !ctx.is_dir(&git) {
                continue;
            }
            let mut m = Map::new();
            m.insert("id".into(), json!(d));
            m.insert(
                "path".into(),
                json!(ctx.tilde(&ctx.p(&rel).to_string_lossy())),
            );
            let remote = ctx
                .read_text(&format!("{git}/config"))
                .ok()
                .and_then(|t| origin_url(&t));
            if let Some(r) = remote {
                m.insert(
                    "remote".into(),
                    ctx.scrub(&Value::String(strip_credentials(&r))),
                );
            }
            repos.push(Value::Object(m));
        }
    }
    Ok(json!({ "infra": { "machine": machine, "clis": clis, "repos": repos } }))
}

impl Importer for Infra {
    fn id(&self) -> &str {
        "infra"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["infra"]
    }
    fn description(&self) -> &str {
        "OS, installed CLIs, git repos under the home dir."
    }
    fn detect(&self, _ctx: &Ctx) -> bool {
        true
    }
    fn run(&self, ctx: &Ctx, opts: &Doc) -> Result<Doc> {
        run_with_path(ctx, opts, std::env::var_os("PATH").as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::Path;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[cfg(unix)]
    fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    fn chmod(_path: &Path, _mode: u32) {}

    fn no_path() -> OsString {
        OsString::new()
    }

    #[test]
    fn repos_come_from_git_config() {
        let tmp = tempfile::tempdir().unwrap();
        let t = tmp.path();
        write(
            &t.join("proj-a/.git/config"),
            "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://example.com/other/proj-a.git\n\
             [remote \"origin\"]\n\turl = https://alex:pw@example.com/alex/proj-a.git\n\
             \tfetch = +refs/heads/*:refs/remotes/origin/*\n[branch \"main\"]\n\tremote = origin\n",
        );
        write(&t.join("proj-b/.git/config"), "[core]\n\tbare = false\n");
        write(&t.join("proj-b/README.md"), "b\n");
        write(
            &t.join(".dotfiles/.git/config"),
            "[remote \"origin\"]\n\turl = https://example.com/alex/dotfiles.git\n",
        );
        write(&t.join("notes/todo.md"), "- [ ] x\n");
        write(&t.join("file.txt"), "x\n");
        write(
            &t.join("code/tool/.git/config"),
            "[remote \"origin\"]\n\turl = https://tok@example.com/alex/tool.git\n",
        );
        let ctx = Ctx::new(Some(t), json!({}));
        assert!(Infra.detect(&ctx));

        let out = run_with_path(&ctx, &json!({}), Some(&no_path())).unwrap();
        assert_eq!(
            out["infra"]["repos"],
            json!([
                { "id": "proj-a", "path": "~/proj-a", "remote": "https://example.com/alex/proj-a.git" },
                { "id": "proj-b", "path": "~/proj-b" }
            ])
        );

        let out =
            run_with_path(&ctx, &json!({ "include_hidden": true }), Some(&no_path())).unwrap();
        let ids: Vec<&str> = out["infra"]["repos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [".dotfiles", "proj-a", "proj-b"]);

        let out = run_with_path(
            &ctx,
            &json!({ "repo_roots": ["~/code/"] }),
            Some(&no_path()),
        )
        .unwrap();
        assert_eq!(
            out["infra"]["repos"],
            json!([{ "id": "tool", "path": "~/code/tool", "remote": "https://example.com/alex/tool.git" }])
        );
    }

    #[test]
    fn origin_url_reads_git_config_syntax() {
        assert_eq!(
            origin_url("[remote \"origin\"]\n\turl = \"https://example.com/a b.git\"\n"),
            Some("https://example.com/a b.git".into())
        );
        assert_eq!(
            origin_url("[remote \"origin\"]\n  url = https://example.com/x.git ; old one\n"),
            Some("https://example.com/x.git".into())
        );
        assert_eq!(
            origin_url("[remote.origin]\nurl=https://example.com/legacy.git\n"),
            Some("https://example.com/legacy.git".into())
        );
        assert_eq!(
            origin_url("[Remote \"origin\"] URL = https://example.com/y.git\n"),
            Some("https://example.com/y.git".into())
        );
        assert_eq!(
            origin_url(
                "[remote \"origin\"]\n# url = https://example.com/commented.git\n\turl = https://example.com/first.git\n\turl = https://example.com/second.git\n"
            ),
            Some("https://example.com/first.git".into())
        );
        assert_eq!(
            origin_url("[remote \"Origin\"]\n\turl = https://example.com/case.git\n"),
            None
        );
        assert_eq!(
            origin_url(
                "[branch \"main\"]\n\tremote = origin\n\turl = https://example.com/no.git\n"
            ),
            None
        );
    }

    #[test]
    fn url_credentials_are_stripped() {
        assert_eq!(
            strip_credentials("https://alex:pw@example.com/x.git"),
            "https://example.com/x.git"
        );
        assert_eq!(
            strip_credentials("ssh://git@example.com/x.git"),
            "ssh://example.com/x.git"
        );
        assert_eq!(
            strip_credentials("git@example.com:alex/x.git"),
            "git@example.com:alex/x.git"
        );
    }

    #[test]
    fn clis_are_found_by_scanning_path_in_process() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        let bin = ctx.home().join("bin");
        let other = ctx.home().join("other");
        write(&bin.join("sac-fake-tool"), "#!/bin/sh\n");
        chmod(&bin.join("sac-fake-tool"), 0o755);
        write(&other.join("sac-fake-tool"), "#!/bin/sh\n");
        chmod(&other.join("sac-fake-tool"), 0o755);
        write(&bin.join("not-exec"), "text\n");
        chmod(&bin.join("not-exec"), 0o644);
        std::fs::create_dir_all(bin.join("a-dir")).unwrap();
        let path_var = std::env::join_paths([&bin, &other]).unwrap();

        assert_eq!(
            which_in("sac-fake-tool", &path_var),
            Some(bin.join("sac-fake-tool"))
        );
        assert_eq!(which_in("a-dir", &path_var), None);
        assert_eq!(which_in("", &path_var), None);
        #[cfg(unix)]
        assert_eq!(which_in("not-exec", &path_var), None);

        let opts = json!({ "clis": ["sac-fake-tool", "a-dir", "git", "sac-fake-tool"] });
        let out = run_with_path(&ctx, &opts, Some(&path_var)).unwrap();
        let clis = out["infra"]["clis"].as_array().unwrap();
        assert_eq!(clis.len(), DEFAULT_CLIS.len() + 2);
        assert_eq!(clis[0], json!({ "id": "claude", "present": false }));
        assert_eq!(
            clis[DEFAULT_CLIS.len()],
            json!({ "id": "sac-fake-tool", "present": true, "path": "~/bin/sac-fake-tool" })
        );
        assert_eq!(
            clis[DEFAULT_CLIS.len() + 1],
            json!({ "id": "a-dir", "present": false })
        );
    }

    #[test]
    fn machine_facts_use_the_reference_vocabulary() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        let out = run_with_path(&ctx, &json!({}), Some(&no_path())).unwrap();
        let machine = &out["infra"]["machine"];
        let keys: Vec<&str> = machine
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(keys, ["platform", "arch", "os"]);
        assert_eq!(machine["platform"], json!(ctx.platform()));
        assert_eq!(machine["arch"], json!(node_arch()));
        assert!(machine["os"].is_string());
        if cfg!(target_os = "macos") {
            let os = machine["os"].as_str().unwrap();
            assert!(os.starts_with(|c: char| c.is_ascii_digit()), "{os}");
        }
        if std::env::consts::ARCH == "aarch64" {
            assert_eq!(node_arch(), "arm64");
        }
        if std::env::consts::ARCH == "x86_64" {
            assert_eq!(node_arch(), "x64");
        }
    }

    #[test]
    fn os_release_pretty_name() {
        assert_eq!(
            pretty_name("NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n"),
            Some("Ubuntu 24.04.1 LTS".into())
        );
        assert_eq!(
            pretty_name("PRETTY_NAME='Debian GNU/Linux 12'\n"),
            Some("Debian GNU/Linux 12".into())
        );
        assert_eq!(pretty_name("PRETTY_NAME=Alpine\n"), Some("Alpine".into()));
        assert_eq!(pretty_name("NAME=x\n"), None);
    }
}
