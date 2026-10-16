//! Render config files from the desired state into `<out_root>/<instance>/` and nowhere else.
//! Nothing is installed, loaded, enabled or sent; a person copies what they want from there.
//!
//! Ported from the reference `framework/lib/apply.mjs`: the same file set, JSON shapes, plist
//! layout, `RULES.md` layout, lock fields and report sections. Where the reference relies on
//! JavaScript coercion (`String(x)`, `Number(x)`, truthiness), the helpers below reproduce it so
//! hand-written manifests render the same way in both implementations.

use crate::ctx::real_home;
use crate::layers::count_layers;
use crate::util::{arr, expand_home, get_path, is_truthy, now_iso, stable_string, today};
use crate::yaml::to_yaml;
use crate::Doc;
use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde_json::{json, Map, Number, Value};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

pub struct ApplyOut {
    pub out_dir: PathBuf,
    pub written: Vec<String>,
    pub n_plist: usize,
    pub lock: Doc,
}

/// Paths apply may never write into, whatever the configuration says.
const PROTECTED: [&str; 8] = [
    "~/.claude",
    "~/.claude.json",
    "~/Library/LaunchAgents",
    "~/Library/LaunchDaemons",
    "/Library",
    "/etc",
    "/System",
    "~/.config/systemd",
];

/// Create the out root and refuse one that resolves into live config. Returns the real path.
///
/// The check runs twice: once before anything is created, on the path as it would resolve
/// (so a refused root is never made), and once on the canonical path afterwards (so a symlink
/// cannot carry the root into a protected place). Protected paths are compared both as written
/// and as they resolve, which also catches `/etc` being `/private/etc` on macOS.
pub fn assert_safe_out_root(out_root: &Path, home: &Path) -> Result<PathBuf> {
    let abs = if out_root.is_absolute() {
        out_root.to_path_buf()
    } else {
        std::env::current_dir()?.join(out_root)
    };
    refuse_protected(&resolve(&abs), home)?;
    std::fs::create_dir_all(&abs).with_context(|| format!("create {}", abs.display()))?;
    let real = std::fs::canonicalize(&abs).with_context(|| format!("resolve {}", abs.display()))?;
    refuse_protected(&real, home)?;
    Ok(real)
}

fn refuse_protected(p: &Path, home: &Path) -> Result<()> {
    for prot in PROTECTED {
        let lexical = normalize(Path::new(&expand_home(prot, home)));
        if p.starts_with(&lexical) || p.starts_with(resolve(&lexical)) {
            bail!("refusing: out/ resolves into protected path {prot}");
        }
    }
    Ok(())
}

/// Lexical clean-up: drop `.` and fold `..` onto its parent.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalize the longest existing ancestor of a path and re-append the rest.
fn resolve(p: &Path) -> PathBuf {
    let p = normalize(p);
    let mut base = p.clone();
    let mut rest: Vec<OsString> = Vec::new();
    loop {
        if let Ok(mut real) = std::fs::canonicalize(&base) {
            for r in rest.iter().rev() {
                real.push(r);
            }
            return real;
        }
        match (base.file_name().map(|f| f.to_os_string()), base.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name);
                base = parent.to_path_buf();
            }
            _ => return p,
        }
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let c = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| normalize(p));
    c(a) == c(b)
}

/// A single path component that cannot climb out of, or reach past, its directory.
fn is_plain_name(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains(['/', '\\', '\0'])
}

/// `meta.instance`, checked to be usable as a directory name.
pub(crate) fn instance_name(desired: &Doc) -> Result<String> {
    let name = js_str(get_path(desired, "meta.instance"));
    if get_path(desired, "meta.instance").is_none() || !is_plain_name(&name) {
        bail!("refusing: meta.instance {name:?} is not a plain directory name");
    }
    Ok(name)
}

/// The loops that get a launchd plist (launchd runner, not `managed: false`, a program), each
/// with its label (checked to be a plain file name) and whether it is enabled.
pub(crate) fn plist_loops(desired: &Doc) -> Result<Vec<(&Doc, String, bool)>> {
    let mut out = Vec::new();
    for l in arr(desired.get("loops")) {
        let launchd = l.get("runner").and_then(|v| v.as_str()) == Some("launchd");
        let unmanaged = l.get("managed") == Some(&Value::Bool(false));
        if !launchd || unmanaged || arr(l.get("program")).is_empty() {
            continue;
        }
        let label = js_str(l.get("id"));
        if !is_plain_name(&label) {
            bail!("refusing: loop id {label:?} is not a plain file name");
        }
        out.push((l, label, is_truthy(l.get("enabled"))));
    }
    Ok(out)
}

/// Files written below the out root. Every directory on the way is created one level at a
/// time and must not be a symlink, and an existing symlink is never overwritten, so nothing
/// planted inside out/ can redirect a write.
struct Writer<'a> {
    root: &'a Path,
    out_dir: PathBuf,
    instance: &'a str,
    written: Vec<String>,
}

impl Writer<'_> {
    fn write(&mut self, rel: &str, content: &str) -> Result<()> {
        let abs = self.out_dir.join(rel);
        let parent = abs
            .parent()
            .ok_or_else(|| anyhow!("refusing to write outside out/: {}", abs.display()))?;
        ensure_dir_below(self.root, parent)?;
        if let Ok(m) = std::fs::symlink_metadata(&abs) {
            if m.file_type().is_symlink() {
                bail!("refusing to overwrite symlink {}", abs.display());
            }
        }
        std::fs::write(&abs, content).with_context(|| format!("write {}", abs.display()))?;
        self.written.push(format!("{}/{}", self.instance, rel));
        Ok(())
    }
}

fn ensure_dir_below(root: &Path, dir: &Path) -> Result<()> {
    let outside = || anyhow!("refusing to write outside out/: {}", dir.display());
    let rel = dir.strip_prefix(root).map_err(|_| outside())?;
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        match c {
            Component::Normal(x) => cur.push(x),
            _ => return Err(outside()),
        }
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                bail!("refusing to follow symlink {}", cur.display())
            }
            Ok(m) if m.is_dir() => {}
            Ok(_) => bail!("{} exists and is not a directory", cur.display()),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                std::fs::create_dir(&cur).with_context(|| format!("create {}", cur.display()))?
            }
            Err(e) => return Err(e).with_context(|| format!("{}", cur.display())),
        }
    }
    Ok(())
}

/// `JSON.stringify(v, null, 2) + "\n"`.
pub(crate) fn pretty_json(v: &Doc) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".to_string());
    s.push('\n');
    s
}

/// Write claude/settings.json, claude/mcp-servers.json, claude/RULES.md, launchd plists,
/// setup.lock.json, setup.resolved.yaml and report.md under `<out_root>/<instance>/`.
pub fn render(
    desired: &Doc,
    out_root: &Path,
    home: &Path,
    policy_text: Option<&str>,
    plan_text: Option<&str>,
) -> Result<ApplyOut> {
    let name = instance_name(desired)?;
    let plists = plist_loops(desired)?;
    let root = assert_safe_out_root(out_root, home)?;
    let real = real_home();
    if !same_dir(&real, home) {
        // `home` may be a sandbox; the person's real config is protected as well.
        assert_safe_out_root(&root, &real)?;
    }
    let out_dir = root.join(&name);
    let mut w = Writer {
        root: &root,
        out_dir: out_dir.clone(),
        instance: &name,
        written: Vec::new(),
    };
    w.write(
        "claude/settings.json",
        &pretty_json(&claude_settings_doc(desired)),
    )?;
    w.write(
        "claude/mcp-servers.json",
        &pretty_json(&mcp_servers_doc(desired)),
    )?;
    w.write("claude/RULES.md", &rules_md(desired))?;
    for (l, label, enabled) in &plists {
        let sub = if *enabled { "" } else { "disabled/" };
        w.write(&format!("launchd/{sub}{label}.plist"), &plist_xml(l, home))?;
    }
    let lock = lock_doc(desired, &name);
    w.write("setup.lock.json", &pretty_json(&lock))?;
    w.write(
        "setup.resolved.yaml",
        &to_yaml(
            desired,
            &format!("# Fully resolved desired state for {name}. Generated; do not edit."),
        ),
    )?;
    let at = lock
        .get("rendered_at")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let report = report_md(
        &name,
        &at,
        desired,
        &w.written,
        plists.len(),
        plan_text,
        policy_text,
    );
    w.write("report.md", &report)?;
    Ok(ApplyOut {
        out_dir,
        written: w.written,
        n_plist: plists.len(),
        lock,
    })
}

/// `{ instance, rendered_at, sha256, counts }`, the hash over the key-sorted desired state and
/// the counts shaped like the reference's (`{ total, <collection>: n, ... }` per layer).
pub(crate) fn lock_doc(desired: &Doc, name: &str) -> Doc {
    let mut counts = Map::new();
    for (layer, c) in count_layers(desired) {
        let mut m = Map::new();
        m.insert("total".to_string(), json!(c.total));
        for (k, n) in c.per {
            m.insert(k, json!(n));
        }
        counts.insert(layer, Value::Object(m));
    }
    let digest = Sha256::digest(stable_string(desired).as_bytes());
    let sha: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    json!({ "instance": name, "rendered_at": now_iso(), "sha256": sha, "counts": counts })
}

fn report_md(
    name: &str,
    at: &str,
    desired: &Doc,
    written: &[String],
    n_plist: usize,
    plan: Option<&str>,
    policy: Option<&str>,
) -> String {
    let mut l: Vec<String> = vec![
        format!("# Apply report: {name}"),
        String::new(),
        format!("Rendered {at}. Nothing was installed, loaded or sent."),
        String::new(),
        "## Counts per layer".into(),
        String::new(),
        "| layer | items |".into(),
        "|---|---|".into(),
    ];
    for (k, v) in count_layers(desired) {
        l.push(format!("| {k} | {} |", v.total));
    }
    l.push(String::new());
    l.push("## Files".into());
    l.push(String::new());
    for f in written {
        l.push(format!("- out/{f}"));
    }
    l.push(format!("- out/{name}/report.md"));
    l.push(String::new());
    l.push(format!("launchd plists rendered: {n_plist} (enabled at top level, disabled under launchd/disabled/). Install by hand if wanted."));
    l.push(String::new());
    let mut section = |title: &str, body: &str| {
        for x in [title, "", "```", body, "```", ""] {
            l.push(x.to_string());
        }
    };
    if let Some(p) = plan.filter(|p| !p.is_empty()) {
        section("## Plan at apply time", p);
    }
    if let Some(p) = policy {
        section("## Policy results", p);
    }
    l.join("\n")
}

fn at<'a>(v: Option<&'a Doc>, k: &str) -> Option<&'a Doc> {
    v.and_then(|x| x.get(k))
}

/// `{ NAME: "${NAME}" }` for a list of names, with the placeholder name passed through `f`.
fn placeholders(names: &[Doc], f: impl Fn(&str) -> String) -> Value {
    let mut m = Map::new();
    for n in names {
        let n = js_str(Some(n));
        let v = format!("${{{}}}", f(&n));
        m.insert(n, Value::String(v));
    }
    Value::Object(m)
}

/// The Claude Code `settings.json` the desired state describes.
pub fn claude_settings_doc(desired: &Doc) -> Doc {
    let t = desired.get("tools");
    let mut hooks = Map::new();
    for h in arr(at(t, "hooks")) {
        let groups = hooks
            .entry(js_str(h.get("event")))
            .or_insert_with(|| json!([]));
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        let matcher = h.get("matcher");
        let i = match groups.iter().position(|g| g.get("matcher") == matcher) {
            Some(i) => i,
            None => {
                let mut g = Map::new();
                if let Some(m) = matcher {
                    g.insert("matcher".into(), m.clone());
                }
                g.insert("hooks".into(), json!([]));
                groups.push(Value::Object(g));
                groups.len() - 1
            }
        };
        let mut entry = Map::new();
        entry.insert("type".into(), json!("command"));
        if let Some(c) = h.get("command") {
            entry.insert("command".into(), c.clone());
        }
        if let Some(list) = groups[i].get_mut("hooks").and_then(|x| x.as_array_mut()) {
            list.push(Value::Object(entry));
        }
    }

    let cc = get_path(desired, "brain.claude_code");
    let mut out = Map::new();
    if is_truthy(at(cc, "model")) {
        out.insert("model".into(), at(cc, "model").cloned().unwrap_or_default());
    }
    if is_truthy(at(cc, "effort")) {
        out.insert(
            "effortLevel".into(),
            at(cc, "effort").cloned().unwrap_or_default(),
        );
    }
    let env = arr(at(cc, "env_names"));
    if !env.is_empty() {
        out.insert("env".into(), placeholders(env, str::to_string));
    }
    let tp = at(t, "permissions");
    let mut perms = Map::new();
    if is_truthy(at(tp, "default_mode")) {
        perms.insert(
            "defaultMode".into(),
            at(tp, "default_mode").cloned().unwrap_or_default(),
        );
    }
    for k in ["allow", "deny", "ask"] {
        let v = at(tp, k)
            .filter(|v| is_truthy(Some(v)))
            .cloned()
            .unwrap_or_else(|| json!([]));
        perms.insert(k.into(), v);
    }
    out.insert("permissions".into(), Value::Object(perms));
    let denied: Vec<Value> = arr(at(t, "denied_mcp_servers"))
        .iter()
        .map(|n| json!({ "serverName": n }))
        .collect();
    out.insert("deniedMcpServers".into(), Value::Array(denied));
    out.insert("hooks".into(), Value::Object(hooks));
    let mut plugins = Map::new();
    for p in arr(at(t, "plugins")) {
        if let Some(en) = p.get("enabled") {
            plugins.insert(js_str(p.get("id")), en.clone());
        }
    }
    out.insert("enabledPlugins".into(), Value::Object(plugins));
    Value::Object(out)
}

/// `enabled: false` servers and servers named in `tools.denied_mcp_servers` are never rendered.
pub(crate) fn mcp_excluded(server: &Doc, denied: &[Doc]) -> bool {
    server.get("enabled") == Some(&Value::Bool(false))
        || server.get("id").is_some_and(|id| denied.contains(id))
}

/// One `mcpServers` entry: the name (the id up to its first `@`) and the definition, with env
/// and header values as `${NAME}` placeholders (headers upper-cased, `-` to `_`).
pub(crate) fn mcp_entry(server: &Doc) -> (String, Doc) {
    let id = js_str(server.get("id"));
    let key = id.split('@').next().unwrap_or_default().to_string();
    let mut def = Map::new();
    match server.get("transport").and_then(|v| v.as_str()) {
        Some(tr @ ("http" | "sse")) => {
            def.insert("type".into(), json!(tr));
            if let Some(u) = server.get("url") {
                def.insert("url".into(), u.clone());
            }
        }
        _ => {
            if let Some(c) = server.get("command") {
                def.insert("command".into(), c.clone());
            }
            let args = server
                .get("args")
                .filter(|v| is_truthy(Some(v)))
                .cloned()
                .unwrap_or_else(|| json!([]));
            def.insert("args".into(), args);
        }
    }
    let env = arr(server.get("env_names"));
    if !env.is_empty() {
        def.insert("env".into(), placeholders(env, str::to_string));
    }
    let headers = arr(server.get("header_names"));
    if !headers.is_empty() {
        def.insert(
            "headers".into(),
            placeholders(headers, |n| n.to_uppercase().replace('-', "_")),
        );
    }
    (key, Value::Object(def))
}

/// `{ "mcpServers": { name: definition } }` for every server that is neither disabled nor denied.
pub fn mcp_servers_doc(desired: &Doc) -> Doc {
    let denied = arr(get_path(desired, "tools.denied_mcp_servers"));
    let mut out = Map::new();
    for server in arr(get_path(desired, "tools.mcp_servers")) {
        if mcp_excluded(server, denied) {
            continue;
        }
        let (key, def) = mcp_entry(server);
        out.insert(key, def);
    }
    json!({ "mcpServers": out })
}

/// Active rules as Markdown: hard then standing, grouped by scope (sorted), one bullet each.
pub fn rules_md(desired: &Doc) -> String {
    let rules: Vec<&Doc> = arr(get_path(desired, "instructions.rules"))
        .iter()
        .filter(|r| r.get("status").and_then(|v| v.as_str()) != Some("superseded"))
        .collect();
    let mut lines = vec![
        format!("# Rules ({})", js_str(get_path(desired, "meta.instance"))),
        String::new(),
        format!(
            "Rendered by setup-as-code on {} from setup.yaml. {} active rules.",
            today(),
            rules.len()
        ),
        String::new(),
    ];
    for sev in ["hard", "standing"] {
        let list: Vec<&Doc> = rules
            .iter()
            .copied()
            .filter(|r| r.get("severity").and_then(|v| v.as_str()) == Some(sev))
            .collect();
        let title = if sev == "hard" {
            "Hard rules"
        } else {
            "Standing rules"
        };
        lines.push(format!("## {title} ({})", list.len()));
        lines.push(String::new());
        // `[...new Set(scopes)].sort()`: first-seen distinct values, compared as strings by
        // UTF-16 code units, a missing scope last.
        let mut scopes: Vec<Option<&Doc>> = Vec::new();
        for r in &list {
            let sc = r.get("scope");
            if !scopes.contains(&sc) {
                scopes.push(sc);
            }
        }
        scopes.sort_by(|a, b| match (a, b) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(x), Some(y)) => js_str(Some(x))
                .encode_utf16()
                .cmp(js_str(Some(y)).encode_utf16()),
        });
        for sc in scopes {
            lines.push(format!("### {}", js_str(sc)));
            lines.push(String::new());
            for r in list.iter().filter(|r| r.get("scope") == sc) {
                let date = if is_truthy(r.get("date")) {
                    js_str(r.get("date"))
                } else {
                    "undated".to_string()
                };
                let (statement, reason) = (r.get("statement"), r.get("reason"));
                let text = if is_truthy(statement) {
                    js_str(statement)
                } else {
                    js_str(reason)
                };
                let why = if is_truthy(reason) && is_truthy(statement) && reason != statement {
                    format!(" _Why: {}_", js_str(reason))
                } else {
                    String::new()
                };
                lines.push(format!(
                    "- **{}** ({date}): {text}{why}",
                    js_str(r.get("id"))
                ));
            }
            lines.push(String::new());
        }
    }
    lines.join("\n")
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// A program argument or path: strings get `~` / `$HOME` expanded, anything else is printed.
fn expanded(v: &Doc, home: &Path) -> String {
    match v.as_str() {
        Some(s) => expand_home(s, home),
        None => js_str(Some(v)),
    }
}

fn kv(lines: &mut Vec<String>, key: &str, value: String) {
    lines.push(format!("  <key>{}</key>", xml(key)));
    lines.push(value);
}

fn cap_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// `hour=9,minute=30` (or `{ hour: 9, minute: 30 }`) as `(Hour, 9)`, `(Minute, 30)`.
fn calendar_pairs(c: &Doc) -> Vec<(String, String)> {
    match c {
        Value::String(s) => s
            .split(',')
            .map(|p| {
                let mut kv = p.split('=');
                let k = kv.next().unwrap_or_default();
                let v = kv.next().map(|v| js_f64(str_to_number(v)));
                (cap_first(k), v.unwrap_or_else(|| "NaN".to_string()))
            })
            .collect(),
        Value::Object(m) => m
            .iter()
            .map(|(k, v)| (cap_first(k), js_number(Some(v))))
            .collect(),
        _ => Vec::new(),
    }
}

/// A launchd user agent for one loop. Env values are `${NAME}` placeholders, never values.
pub fn plist_xml(loop_: &Doc, home: &Path) -> String {
    let mut lines: Vec<String> = vec![
        r#"<?xml version="1.0" encoding="UTF-8"?>"#.into(),
        r#"<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">"#.into(),
        r#"<plist version="1.0">"#.into(),
        "<dict>".into(),
    ];
    kv(
        &mut lines,
        "Label",
        format!("  <string>{}</string>", xml(&js_str(loop_.get("id")))),
    );
    let program = arr(loop_.get("program"));
    if !program.is_empty() {
        let mut v = vec!["  <array>".to_string()];
        for a in program {
            v.push(format!("    <string>{}</string>", xml(&expanded(a, home))));
        }
        v.push("  </array>".into());
        kv(&mut lines, "ProgramArguments", v.join("\n"));
    }
    if let Some(wd) = loop_.get("working_dir").filter(|v| is_truthy(Some(v))) {
        let v = format!("  <string>{}</string>", xml(&expanded(wd, home)));
        kv(&mut lines, "WorkingDirectory", v);
    }
    if let Some(log) = loop_.get("log").filter(|v| is_truthy(Some(v))) {
        let v = format!("  <string>{}</string>", xml(&expanded(log, home)));
        kv(&mut lines, "StandardOutPath", v.clone());
        kv(&mut lines, "StandardErrorPath", v);
    }
    let sched = loop_.get("schedule");
    if is_truthy(at(sched, "every_seconds")) {
        let v = format!(
            "  <integer>{}</integer>",
            js_number(at(sched, "every_seconds"))
        );
        kv(&mut lines, "StartInterval", v);
    }
    let calendar = arr(at(sched, "calendar"));
    if !calendar.is_empty() {
        let mut v = vec!["  <array>".to_string()];
        for c in calendar {
            v.push("    <dict>".into());
            for (k, n) in calendar_pairs(c) {
                v.push(format!(
                    "      <key>{}</key>\n      <integer>{n}</integer>",
                    xml(&k)
                ));
            }
            v.push("    </dict>".into());
        }
        v.push("  </array>".into());
        kv(&mut lines, "StartCalendarInterval", v.join("\n"));
    }
    if is_truthy(at(sched, "keep_alive")) {
        kv(&mut lines, "KeepAlive", "  <true/>".into());
    }
    let run_at_load = if is_truthy(at(sched, "run_at_load")) {
        "  <true/>"
    } else {
        "  <false/>"
    };
    kv(&mut lines, "RunAtLoad", run_at_load.into());
    let env = arr(loop_.get("env_names"));
    if !env.is_empty() {
        let mut v = vec!["  <dict>".to_string()];
        for n in env {
            let n = xml(&js_str(Some(n)));
            v.push(format!("    <key>{n}</key>\n    <string>${{{n}}}</string>"));
        }
        v.push("  </dict>".into());
        kv(&mut lines, "EnvironmentVariables", v.join("\n"));
    }
    lines.push("</dict>".into());
    lines.push("</plist>".into());
    lines.push(String::new());
    lines.join("\n")
}

/// `String(v)` as JavaScript prints it in a template literal (`undefined` when absent).
fn js_str(v: Option<&Doc>) -> String {
    match v {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => js_num(n),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::Null => String::new(),
                other => js_str(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

fn js_num(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        js_f64(n.as_f64().unwrap_or(f64::NAN))
    }
}

fn js_f64(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        let s = if f > 0.0 { "Infinity" } else { "-Infinity" };
        s.to_string()
    } else if f == f.trunc() && f.abs() < 1e21 {
        format!("{}", f as i128)
    } else {
        format!("{f}")
    }
}

/// `Number(s)` for a string: trimmed decimal, `0x`/`0o`/`0b` integers, `Infinity`, else NaN.
fn str_to_number(s: &str) -> f64 {
    static DECIMAL: OnceLock<Regex> = OnceLock::new();
    let t = s.trim();
    if t.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(d) = t.strip_prefix(prefix) {
            return u128::from_str_radix(d, radix)
                .map(|x| x as f64)
                .unwrap_or(f64::NAN);
        }
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let re =
        DECIMAL.get_or_init(|| Regex::new(r"^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$").unwrap());
    if re.is_match(t) {
        t.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        f64::NAN
    }
}

/// `String(Number(v))`.
fn js_number(v: Option<&Doc>) -> String {
    match v {
        None => "NaN".into(),
        Some(Value::Null) => "0".into(),
        Some(Value::Bool(b)) => u8::from(*b).to_string(),
        Some(Value::Number(n)) => js_num(n),
        Some(Value::String(s)) => js_f64(str_to_number(s)),
        Some(Value::Array(a)) if a.len() <= 1 => {
            // `Number([x])` is `Number(String(x))`, and `String([null])` is "".
            let first = a.first().filter(|x| !x.is_null());
            js_f64(str_to_number(
                &first.map(|x| js_str(Some(x))).unwrap_or_default(),
            ))
        }
        Some(_) => "NaN".into(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A synthetic setup with two rendered loops (one enabled, one disabled) and three that
    /// are not rendered (unmanaged, cron, no program).
    pub(crate) fn fixture() -> Doc {
        json!({
            "version": 1,
            "meta": { "instance": "alex" },
            "brain": { "claude_code": { "model": "opus", "effort": "high", "env_names": ["EXAMPLE_API_KEY"] } },
            "instructions": { "rules": [
                { "id": "no-spend", "scope": "money", "severity": "hard", "date": "2026-01-01",
                  "reason": "Money moves only by hand.", "statement": "No purchases by any agent." },
                { "id": "thread-replies", "scope": "chat", "severity": "standing", "date": null,
                  "reason": "Keeps channels readable." },
                { "id": "bot-only", "scope": "chat", "severity": "hard", "date": "2026-02-01",
                  "reason": "Posts go through the bot.", "statement": "Posts go through the bot." },
                { "id": "old-rule", "scope": "chat", "severity": "hard", "date": "2025-01-01",
                  "reason": "Replaced.", "status": "superseded" }
            ]},
            "tools": {
                "mcp_servers": [
                    { "id": "notes", "transport": "http", "url": "https://mcp.example.com/notes", "header_names": ["x-api-key"], "enabled": true },
                    { "id": "files@~/.claude.json", "transport": "stdio", "command": "npx", "args": ["-y", "example-files"], "env_names": ["FILES_ROOT"] },
                    { "id": "off", "transport": "stdio", "command": "off-server", "enabled": false },
                    { "id": "work-chat", "transport": "stdio", "command": "chat-server" }
                ],
                "plugins": [ { "id": "helper@example", "enabled": true }, { "id": "quiet@example", "enabled": false }, { "id": "unset@example" } ],
                "hooks": [
                    { "id": "guard", "event": "PreToolUse", "matcher": "Bash", "command": "~/bin/guard.sh" },
                    { "id": "log", "event": "PreToolUse", "matcher": "Bash", "command": "~/bin/log.sh" },
                    { "id": "done", "event": "Stop", "command": "~/bin/done.sh" }
                ],
                "permissions": { "default_mode": "acceptEdits", "allow": ["Read", "Bash(git status*)"], "deny": ["Bash(rm -rf*)"], "ask": [] },
                "denied_mcp_servers": ["work-chat"]
            },
            "loops": [
                { "id": "com.example.morning", "runner": "launchd", "enabled": true,
                  "program": ["/bin/sh", "-c", "~/bin/brief.sh && echo <done>"], "working_dir": "~/work", "log": "~/logs/morning.log",
                  "schedule": { "calendar": ["hour=9,minute=30", "weekday=1,hour=18"] }, "env_names": ["EXAMPLE_TOKEN"] },
                { "id": "com.example.sweep", "runner": "launchd", "enabled": false, "program": ["/usr/bin/true"],
                  "schedule": { "every_seconds": 3600, "keep_alive": true, "run_at_load": true } },
                { "id": "com.example.vendor", "runner": "launchd", "enabled": true, "managed": false, "program": ["/usr/bin/true"] },
                { "id": "com.example.nightly", "runner": "cron", "enabled": true, "program": ["/usr/bin/true"] },
                { "id": "com.example.placeholder", "runner": "launchd", "enabled": true }
            ]
        })
    }

    fn find_on_path(name: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    }

    pub(crate) fn err_of<T>(r: Result<T>) -> String {
        match r {
            Ok(_) => panic!("expected an error"),
            Err(e) => format!("{e:#}"),
        }
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    // Expected outputs below were produced by the reference framework/lib/apply.mjs on the
    // same fixture.

    #[test]
    fn apply_settings_doc_matches_reference() {
        let expected = r#"{
  "model": "opus",
  "effortLevel": "high",
  "env": {
    "EXAMPLE_API_KEY": "${EXAMPLE_API_KEY}"
  },
  "permissions": {
    "defaultMode": "acceptEdits",
    "allow": [
      "Read",
      "Bash(git status*)"
    ],
    "deny": [
      "Bash(rm -rf*)"
    ],
    "ask": []
  },
  "deniedMcpServers": [
    {
      "serverName": "work-chat"
    }
  ],
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          {
            "type": "command",
            "command": "~/bin/guard.sh"
          },
          {
            "type": "command",
            "command": "~/bin/log.sh"
          }
        ]
      }
    ],
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "~/bin/done.sh"
          }
        ]
      }
    ]
  },
  "enabledPlugins": {
    "helper@example": true,
    "quiet@example": false
  }
}
"#;
        assert_eq!(pretty_json(&claude_settings_doc(&fixture())), expected);
        let empty = claude_settings_doc(&json!({ "meta": { "instance": "x" } }));
        assert_eq!(
            serde_json::to_string(&empty).unwrap(),
            r#"{"permissions":{"allow":[],"deny":[],"ask":[]},"deniedMcpServers":[],"hooks":{},"enabledPlugins":{}}"#
        );
    }

    #[test]
    fn apply_mcp_doc_matches_reference_and_skips_denied_and_disabled() {
        let expected = r#"{
  "mcpServers": {
    "notes": {
      "type": "http",
      "url": "https://mcp.example.com/notes",
      "headers": {
        "x-api-key": "${X_API_KEY}"
      }
    },
    "files": {
      "command": "npx",
      "args": [
        "-y",
        "example-files"
      ],
      "env": {
        "FILES_ROOT": "${FILES_ROOT}"
      }
    }
  }
}
"#;
        assert_eq!(pretty_json(&mcp_servers_doc(&fixture())), expected);
    }

    #[test]
    fn apply_rules_md_matches_reference_layout() {
        let expected = format!(
            "# Rules (alex)

Rendered by setup-as-code on {} from setup.yaml. 3 active rules.

## Hard rules (2)

### chat

- **bot-only** (2026-02-01): Posts go through the bot.

### money

- **no-spend** (2026-01-01): No purchases by any agent. _Why: Money moves only by hand._

## Standing rules (1)

### chat

- **thread-replies** (undated): Keeps channels readable.
",
            today()
        );
        assert_eq!(rules_md(&fixture()), expected);
    }

    #[test]
    fn apply_plist_xml_matches_reference_layout() {
        let d = fixture();
        let home = Path::new("/Users/example");
        let morning = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.example.morning</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/sh</string>
    <string>-c</string>
    <string>/Users/example/bin/brief.sh &amp;&amp; echo &lt;done&gt;</string>
  </array>
  <key>WorkingDirectory</key>
  <string>/Users/example/work</string>
  <key>StandardOutPath</key>
  <string>/Users/example/logs/morning.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/example/logs/morning.log</string>
  <key>StartCalendarInterval</key>
  <array>
    <dict>
      <key>Hour</key>
      <integer>9</integer>
      <key>Minute</key>
      <integer>30</integer>
    </dict>
    <dict>
      <key>Weekday</key>
      <integer>1</integer>
      <key>Hour</key>
      <integer>18</integer>
    </dict>
  </array>
  <key>RunAtLoad</key>
  <false/>
  <key>EnvironmentVariables</key>
  <dict>
    <key>EXAMPLE_TOKEN</key>
    <string>${EXAMPLE_TOKEN}</string>
  </dict>
</dict>
</plist>
"#;
        let sweep = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.example.sweep</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/bin/true</string>
  </array>
  <key>StartInterval</key>
  <integer>3600</integer>
  <key>KeepAlive</key>
  <true/>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#;
        assert_eq!(plist_xml(&d["loops"][0], home), morning);
        assert_eq!(plist_xml(&d["loops"][1], home), sweep);
        // JavaScript coercions the reference relies on.
        let odd = json!({ "id": "x", "schedule": { "every_seconds": "90", "calendar": [{ "minute": "5" }] } });
        let xml = plist_xml(&odd, home);
        assert!(xml.contains("<key>StartInterval</key>\n  <integer>90</integer>"));
        assert!(xml.contains("<key>Minute</key>\n      <integer>5</integer>"));
        assert!(!xml.contains("ProgramArguments"));
    }

    #[test]
    fn apply_render_two_loop_setup_writes_plists_and_files_list() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let out = tmp.path().join("out");
        let r = render(&fixture(), &out, &home, None, None).unwrap();
        assert_eq!(
            r.written,
            vec![
                "alex/claude/settings.json",
                "alex/claude/mcp-servers.json",
                "alex/claude/RULES.md",
                "alex/launchd/com.example.morning.plist",
                "alex/launchd/disabled/com.example.sweep.plist",
                "alex/setup.lock.json",
                "alex/setup.resolved.yaml",
                "alex/report.md",
            ]
        );
        assert_eq!(r.n_plist, 2);
        let real_out = out.canonicalize().unwrap();
        assert_eq!(r.out_dir, real_out.join("alex"));
        for f in &r.written {
            assert!(real_out.join(f).is_file(), "{f}");
        }
        let launchd = r.out_dir.join("launchd");
        assert!(!launchd.join("com.example.sweep.plist").exists());
        for skipped in ["vendor", "nightly", "placeholder"] {
            let name = format!("com.example.{skipped}.plist");
            assert!(
                !launchd.join(&name).exists() && !launchd.join("disabled").join(&name).exists()
            );
        }
        assert_eq!(
            read(&launchd.join("com.example.morning.plist")),
            plist_xml(&fixture()["loops"][0], &home)
        );
        assert_eq!(
            read(&r.out_dir.join("claude/settings.json")),
            pretty_json(&claude_settings_doc(&fixture()))
        );
        let resolved = read(&r.out_dir.join("setup.resolved.yaml"));
        assert!(resolved
            .starts_with("# Fully resolved desired state for alex. Generated; do not edit.\n"));
        assert_eq!(crate::yaml::parse_yaml(&resolved).unwrap(), fixture());

        let Some(plutil) = find_on_path("plutil") else {
            eprintln!("plutil not on PATH: skipping the plutil -lint check");
            return;
        };
        for p in [
            launchd.join("com.example.morning.plist"),
            launchd.join("disabled/com.example.sweep.plist"),
        ] {
            let st = std::process::Command::new(&plutil)
                .arg("-lint")
                .arg(&p)
                .output()
                .unwrap();
            assert!(
                st.status.success(),
                "plutil -lint {}: {}",
                p.display(),
                String::from_utf8_lossy(&st.stdout)
            );
        }
    }

    #[test]
    fn apply_render_report_lists_files_and_optional_sections() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let out = tmp.path().join("out");
        let r = render(
            &fixture(),
            &out,
            &home,
            Some("  [PASS] hard     p1 (deny_mcp): ok"),
            Some("plan: nothing to do"),
        )
        .unwrap();
        let report = read(&r.out_dir.join("report.md"));
        assert!(report.starts_with("# Apply report: alex\n\nRendered "));
        assert!(report.contains(". Nothing was installed, loaded or sent.\n\n## Counts per layer\n\n| layer | items |\n|---|---|\n| identity | 0 |\n| brain | 0 |\n| instructions | 4 |\n| tools | 14 |\n| loops | 5 |\n"));
        assert!(report.contains("\n## Files\n\n- out/alex/claude/settings.json\n- out/alex/claude/mcp-servers.json\n- out/alex/claude/RULES.md\n- out/alex/launchd/com.example.morning.plist\n- out/alex/launchd/disabled/com.example.sweep.plist\n- out/alex/setup.lock.json\n- out/alex/setup.resolved.yaml\n- out/alex/report.md\n\nlaunchd plists rendered: 2 (enabled at top level, disabled under launchd/disabled/). Install by hand if wanted.\n\n"));
        assert!(report.ends_with("## Plan at apply time\n\n```\nplan: nothing to do\n```\n\n## Policy results\n\n```\n  [PASS] hard     p1 (deny_mcp): ok\n```\n"));

        let r2 = render(&fixture(), &tmp.path().join("out2"), &home, None, None).unwrap();
        let report2 = read(&r2.out_dir.join("report.md"));
        assert!(!report2.contains("## Plan") && !report2.contains("## Policy"));
        assert!(report2.ends_with("Install by hand if wanted.\n"));
    }

    #[test]
    fn apply_lock_hashes_the_stable_string_and_counts_layers() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let d = fixture();
        let r = render(&d, &tmp.path().join("out"), &home, None, None).unwrap();
        let expected: String = Sha256::digest(stable_string(&d).as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(r.lock["sha256"], json!(expected));
        assert_eq!(r.lock["instance"], json!("alex"));
        let keys: Vec<&String> = r.lock.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["instance", "rendered_at", "sha256", "counts"]);
        let tools = &r.lock["counts"]["tools"];
        assert_eq!(tools.as_object().unwrap().keys().next().unwrap(), "total");
        assert_eq!(
            *tools,
            json!({ "total": 14, "mcp_servers": 4, "skills": 0, "plugins": 3, "hooks": 3, "commands": 0,
                    "permissions.allow": 2, "permissions.deny": 1, "permissions.ask": 0, "denied_mcp_servers": 1 })
        );
        assert_eq!(r.lock["counts"]["loops"], json!({ "total": 5, "loops": 5 }));
        let on_disk: Value =
            serde_json::from_str(&read(&r.out_dir.join("setup.lock.json"))).unwrap();
        assert_eq!(on_disk, r.lock);
        let at = r.lock["rendered_at"].as_str().unwrap();
        assert!(Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$")
            .unwrap()
            .is_match(at));
    }

    #[test]
    fn apply_out_root_inside_fake_claude_dir_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let bad = home.join(".claude").join("out");
        let err = assert_safe_out_root(&bad, &home).unwrap_err().to_string();
        assert!(err.contains("protected path ~/.claude"), "{err}");
        assert!(!bad.exists(), "a refused out root must not be created");
        let agents = home.join("Library/LaunchAgents/out");
        let err = assert_safe_out_root(&agents, &home)
            .unwrap_err()
            .to_string();
        assert!(err.contains("~/Library/LaunchAgents"), "{err}");
        assert!(!home.join("Library").exists());
        assert!(render(&fixture(), &bad, &home, None, None).is_err());
        assert!(!bad.exists());

        // Component-wise: a sibling that merely shares the prefix is fine.
        let ok = home.join(".claude-notes/out");
        assert_eq!(
            assert_safe_out_root(&ok, &home).unwrap(),
            ok.canonicalize().unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn apply_out_root_reaching_a_protected_path_through_a_symlink_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::os::unix::fs::symlink(home.join(".claude"), tmp.path().join("sneaky")).unwrap();
        let err = assert_safe_out_root(&tmp.path().join("sneaky/out"), &home)
            .unwrap_err()
            .to_string();
        assert!(err.contains("protected path ~/.claude"), "{err}");
        assert!(!home.join(".claude/out").exists());
    }

    #[cfg(unix)]
    #[test]
    fn apply_render_refuses_planted_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let victim = tmp.path().join("victim.txt");
        std::fs::write(&victim, "keep").unwrap();
        let out = tmp.path().join("out");
        std::fs::create_dir_all(out.join("alex/claude")).unwrap();
        std::os::unix::fs::symlink(&victim, out.join("alex/claude/settings.json")).unwrap();
        let err = err_of(render(&fixture(), &out, &home, None, None));
        assert!(err.contains("refusing to overwrite symlink"), "{err}");
        assert_eq!(read(&victim), "keep");

        let elsewhere = tmp.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let out2 = tmp.path().join("out2");
        std::fs::create_dir_all(out2.join("alex")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, out2.join("alex/launchd")).unwrap();
        let err = err_of(render(&fixture(), &out2, &home, None, None));
        assert!(err.contains("refusing to follow symlink"), "{err}");
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[test]
    fn apply_render_refuses_unsafe_names_before_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let out = tmp.path().join("out");
        let mut d = fixture();
        d["loops"][0]["id"] = json!("../../escape");
        let err = err_of(render(&d, &out, &home, None, None));
        assert!(err.contains("not a plain file name"), "{err}");
        assert!(!out.exists());
        let mut d = fixture();
        d["meta"]["instance"] = json!("..");
        assert!(render(&d, &out, &home, None, None).is_err());
        assert!(render(&json!({ "version": 1 }), &out, &home, None, None).is_err());
        assert!(!out.exists());
    }
}
