//! `apply --live`: merge the desired state into a home directory. Only managed keys change,
//! every file about to change is copied to a backup first, and nothing is loaded, started or
//! sent: the `launchctl bootstrap` commands come back for the person to run by hand.
//!
//! Contract (spec section "`apply --live`"):
//! - `~/.claude/settings.json`: `permissions.allow|deny|ask`, `permissions.defaultMode`,
//!   `deniedMcpServers`, `hooks`, `enabledPlugins`, `model` and `effortLevel` come from
//!   setup.yaml. A managed key whose setup.yaml section is absent (or whose scalar is unset)
//!   keeps its live value; a present section, even an empty one, replaces it. `hooks` and
//!   `deniedMcpServers` are written only when the file lacks them, or with `force`, because
//!   setup.yaml models them lossily; an existing value that differs is kept and reported.
//!   Every other key stays as found, and `env` is never written at all: setup.yaml holds env
//!   names, not values, so the names that still need a value go in the report.
//! - `~/.claude.json`: user-scope `mcpServers` the file lacks are added (the file is created as
//!   `{"mcpServers":{}}` when absent). An existing server keeps its `env` and `headers` always,
//!   and its `command`, `args`, `url`, `type` unless `force`. Disabled and denied servers and
//!   `name@source` duplicates are never added. Project-scoped servers stay out of the user scope.
//! - launchd plists: `~/Library/LaunchAgents/<label>.plist`, disabled loops under `disabled/`,
//!   written when missing or with `force`. `launchctl` is never run.
//! - `~/.claude/setup-as-code/{RULES.md,setup.lock.json,report.md}`: regenerated every run.
//! - settings.json, .claude.json or a plist into which this run would put the `<redacted>`
//!   placeholder (where import dropped a secret-shaped value) is not written at all, whatever
//!   `force` says; it is reported with the paths of the placeholders. A placeholder the file
//!   already held at the same place does not count.
//! - A malformed existing settings.json or .claude.json stops the run before any backup or write.
//! - Containment: every path the run may write, and the backup directory, is resolved through
//!   symlinks first, and the run is refused when one lands outside the home. For a home that is
//!   not the real one, it is also refused when one lands in the real home (a sandbox that itself
//!   lives in the real home may write inside itself, never into the real home's protected
//!   config). Symlinks that stay inside the home keep working.
//! - Writes are atomic (temp file + rename) and go through a symlinked file to its target.

use crate::apply::{
    absolute_home, claude_settings_doc, code_list, instance_name, lock_doc, mcp_entry,
    mcp_excluded, mcp_project_scoped, plist_loops, plist_redacted_paths, plist_xml, pretty_json,
    protected_hit, redacted_paths, resolve, rules_md, REDACTED,
};
use crate::ctx::real_home;
use crate::util::{arr, get_path, now_iso, str_list};
use crate::Doc;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Map, Value};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

pub struct LiveOptions {
    pub home: PathBuf,
    pub yes: bool,
    pub force: bool,
}

/// `changed` holds `~/...` paths written; `skipped` holds `~/... (reason)` for files and MCP
/// servers left as they were.
pub struct LiveReport {
    pub backup_dir: Option<PathBuf>,
    pub changed: Vec<String>,
    pub skipped: Vec<String>,
    pub commands: Vec<String>,
}

const SETTINGS: &str = ".claude/settings.json";
const CLAUDE_JSON: &str = ".claude.json";
const AGENTS: &str = "Library/LaunchAgents";
const STATE: &str = ".claude/setup-as-code";
const BACKUPS: &str = ".setup-as-code/backups";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Create,
    Update,
    /// Exists and is only replaced with `force`.
    Keep,
    Unchanged,
    /// Would carry the `<redacted>` placeholder: never written.
    Redacted,
}

/// One file the run may write, by path relative to the home.
struct Target {
    rel: String,
    content: String,
    /// Replace an existing file only with `force` (plists).
    force_only: bool,
    /// Written on every run (the state files under `~/.claude/setup-as-code/`).
    always: bool,
    /// The existing file already holds this JSON (in any formatting): leave it alone.
    same_as_existing: bool,
    /// A new file gets mode 0600.
    private: bool,
    /// An enabled launchd plist: writing it yields a `launchctl bootstrap` command.
    launch: bool,
    /// Names written as `${NAME}` placeholders that still need a value.
    fill_in: Vec<String>,
    /// Where writing this file would put the `<redacted>` placeholder; any refuses the file.
    redacted: Vec<String>,
    action: Action,
}

impl Target {
    fn new(rel: impl Into<String>, content: String) -> Target {
        Target {
            rel: rel.into(),
            content,
            force_only: false,
            always: false,
            same_as_existing: false,
            private: false,
            launch: false,
            fill_in: Vec::new(),
            redacted: Vec::new(),
            action: Action::Create,
        }
    }

    fn state(rel: &str, content: String) -> Target {
        let mut t = Target::new(format!("{STATE}/{rel}"), content);
        t.always = true;
        t
    }

    fn writes(&self) -> bool {
        matches!(self.action, Action::Create | Action::Update)
    }
}

pub fn apply_live(desired: &Doc, opts: &LiveOptions) -> Result<LiveReport> {
    apply_live_in(desired, opts, &real_home())
}

/// `apply_live` with the real home passed in, so tests can stand a temp directory in for it.
pub(crate) fn apply_live_in(desired: &Doc, opts: &LiveOptions, real: &Path) -> Result<LiveReport> {
    if !opts.yes {
        bail!(
            "apply --live writes into {} and needs --yes",
            opts.home.display()
        );
    }
    if !opts.home.is_dir() {
        bail!(
            "apply --live: home {} is not a directory",
            opts.home.display()
        );
    }
    let home = absolute_home(&opts.home)?;
    let home = home.as_path();
    let name = instance_name(desired)?;
    let loops = plist_loops(desired)?;
    let plist_rel = |label: &str, enabled: bool| {
        if enabled {
            format!("{AGENTS}/{label}.plist")
        } else {
            format!("{AGENTS}/disabled/{label}.plist")
        }
    };

    // Every check and read happens before the first backup or write.
    let mut reach: Vec<String> = [SETTINGS, CLAUDE_JSON, BACKUPS]
        .into_iter()
        .map(String::from)
        .collect();
    reach.extend(loops.iter().map(|(_, label, on)| plist_rel(label, *on)));
    reach.extend(
        ["RULES.md", "setup.lock.json", "report.md"]
            .into_iter()
            .map(|f| format!("{STATE}/{f}")),
    );
    check_containment(home, real, &reach)?;
    let settings = read_object(home, SETTINGS)?;
    if settings
        .as_ref()
        .and_then(|m| m.get("permissions"))
        .is_some_and(|p| !p.is_object())
    {
        bail!("refusing: \"permissions\" in ~/{SETTINGS} is not an object; nothing was backed up or written");
    }
    let claude_json = read_object(home, CLAUDE_JSON)?;
    if claude_json
        .as_ref()
        .and_then(|m| m.get("mcpServers"))
        .is_some_and(|m| !m.is_object())
    {
        bail!("refusing: \"mcpServers\" in ~/{CLAUDE_JSON} is not an object; nothing was backed up or written");
    }

    let env_not_written: Vec<String> = str_list(get_path(desired, "brain.claude_code.env_names"))
        .into_iter()
        .filter(|n| {
            settings
                .as_ref()
                .and_then(|m| m.get("env"))
                .and_then(|e| e.get(n))
                .is_none()
        })
        .collect();
    let mut notes = Vec::new();
    let merged = merge_settings(
        settings.clone().unwrap_or_default(),
        &claude_settings_doc(desired),
        desired,
        opts.force,
        &mut notes,
    );
    // A JSON target over the file as found: only placeholders this run would add count.
    let json_target =
        |rel: &str, merged: Map<String, Value>, found: &Option<Map<String, Value>>| {
            let merged = Value::Object(merged);
            let found = found.clone().map(Value::Object);
            let mut t = Target::new(rel, pretty_json(&merged));
            t.redacted = redacted_paths(&merged, found.as_ref(), "");
            t.same_as_existing = found.as_ref() == Some(&merged);
            t
        };
    let mut targets = vec![json_target(SETTINGS, merged, &settings)];
    let (merged, added) = merge_claude_json(
        claude_json.clone().unwrap_or_default(),
        desired,
        opts.force,
        &mut notes,
    );
    let mut t = json_target(CLAUDE_JSON, merged, &claude_json);
    t.private = true;
    t.fill_in = added;
    targets.push(t);
    for (l, label, enabled) in &loops {
        let mut t = Target::new(plist_rel(label, *enabled), plist_xml(l, home));
        t.force_only = true;
        t.launch = *enabled;
        t.fill_in = str_list(l.get("env_names"));
        t.redacted = plist_redacted_paths(l);
        targets.push(t);
    }
    targets.push(Target::state("RULES.md", rules_md(desired)));
    targets.push(Target::state(
        "setup.lock.json",
        pretty_json(&lock_doc(desired, &name)),
    ));
    // Filled in below, once the outcome it describes is known.
    targets.push(Target::state("report.md", String::new()));
    for t in &mut targets {
        t.action = decide(home, t, opts.force)?;
        // Skipped whole: no copy with the placeholder, and no copy that leaves it out.
        if t.writes() && !t.redacted.is_empty() {
            t.action = Action::Redacted;
        }
    }

    let backup_dir = if targets.iter().any(|t| t.action == Action::Update) {
        Some(new_backup_dir(home)?)
    } else {
        None
    };
    let commands: Vec<String> = targets
        .iter()
        .filter(|t| t.launch && t.writes())
        .map(|t| {
            format!(
                "launchctl bootstrap gui/$UID {}",
                shell_quote(&home.join(&t.rel).to_string_lossy())
            )
        })
        .collect();
    let mut skipped: Vec<String> = targets
        .iter()
        .filter_map(|t| match t.action {
            Action::Keep => Some(format!("~/{} (exists; --force overwrites)", t.rel)),
            Action::Unchanged => Some(format!("~/{} (unchanged)", t.rel)),
            Action::Redacted => Some(format!(
                "~/{} (not written: setup.yaml would put the placeholder `{REDACTED}` at {}, where import dropped a secret-shaped value)",
                t.rel,
                code_list(&t.redacted)
            )),
            _ => None,
        })
        .collect();
    skipped.extend(notes);
    let text = live_report_md(
        &name,
        home,
        &now_iso(),
        &Outcome {
            targets: &targets,
            skipped: &skipped,
            backup_dir: backup_dir.as_deref(),
            commands: &commands,
            env_not_written: &env_not_written,
        },
    );
    if let Some(r) = targets.last_mut() {
        r.content = text;
    }

    if let Some(dir) = &backup_dir {
        for t in targets.iter().filter(|t| t.action == Action::Update) {
            let dst = dir.join(&t.rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            std::fs::copy(home.join(&t.rel), &dst)
                .with_context(|| format!("back up ~/{} to {}", t.rel, dst.display()))?;
        }
    }
    for t in targets.iter().filter(|t| t.writes()) {
        write_atomic(&home.join(&t.rel), t.content.as_bytes(), t.private)?;
    }
    Ok(LiveReport {
        backup_dir,
        changed: targets
            .iter()
            .filter(|t| t.writes())
            .map(|t| format!("~/{}", t.rel))
            .collect(),
        skipped,
        commands,
    })
}

/// An existing file as a JSON object, `None` when absent. Anything unreadable or not a JSON
/// object refuses the whole run.
fn read_object(home: &Path, rel: &str) -> Result<Option<Map<String, Value>>> {
    let text = match std::fs::read_to_string(home.join(rel)) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            bail!("refusing: cannot read ~/{rel} ({e}); nothing was backed up or written")
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(m)) => Ok(Some(m)),
        Ok(_) => bail!("refusing: ~/{rel} is not a JSON object; nothing was backed up or written"),
        Err(e) => {
            bail!("refusing: ~/{rel} is not valid JSON ({e}); nothing was backed up or written")
        }
    }
}

/// Refuse the run when a path it may write resolves, through symlinks, outside the home; or,
/// for a home that is not the real one, into the real home. A sandbox that itself lives in the
/// real home may write inside itself, but never into the real home's protected config. A
/// dangling symlink on the way is refused too, since its target cannot be checked.
fn check_containment(home: &Path, real: &Path, rels: &[String]) -> Result<()> {
    let real = absolute_home(real)?;
    let sandbox = real != home;
    let nested = home.starts_with(&real);
    for rel in rels {
        let mut on_the_way = home.to_path_buf();
        for part in Path::new(rel).components() {
            on_the_way.push(part);
            let is_link = std::fs::symlink_metadata(&on_the_way)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            if is_link && std::fs::canonicalize(&on_the_way).is_err() {
                bail!(
                    "refusing: {} is a dangling symlink; nothing was backed up or written",
                    on_the_way.display()
                );
            }
        }
        let p = resolve(&home.join(rel));
        if !p.starts_with(home) {
            bail!(
                "refusing: ~/{rel} resolves to {}, outside the home {}; nothing was backed up or written",
                p.display(),
                home.display()
            );
        }
        if sandbox && p.starts_with(&real) && !nested {
            bail!(
                "refusing: ~/{rel} resolves to {}, inside the real home; nothing was backed up or written",
                p.display()
            );
        }
        if sandbox {
            if let Some(prot) = protected_hit(&p, &real) {
                bail!(
                    "refusing: ~/{rel} resolves into the real home's protected path {prot}; nothing was backed up or written"
                );
            }
        }
    }
    Ok(())
}

/// The rendered settings' managed keys over the existing file. A key whose setup.yaml section
/// is absent keeps its live value, as does an unset `model`, `effortLevel` or `defaultMode`; a
/// present section replaces it, even when empty. `hooks` and `deniedMcpServers` are written
/// only when the file lacks them, or with `force`: setup.yaml models them lossily (every hook
/// becomes a command hook, every denial a server name), so an existing value that differs is
/// kept and noted.
fn merge_settings(
    mut cur: Map<String, Value>,
    want: &Doc,
    desired: &Doc,
    force: bool,
    notes: &mut Vec<String>,
) -> Map<String, Value> {
    let declared = |path: &str| get_path(desired, path).is_some();
    for k in ["model", "effortLevel"] {
        if let Some(v) = want.get(k) {
            cur.insert(k.to_string(), v.clone());
        }
    }
    let wp = want.get("permissions");
    let mut perms: Vec<(&str, &Value)> = Vec::new();
    if let Some(v) = wp.and_then(|w| w.get("defaultMode")) {
        perms.push(("defaultMode", v));
    }
    for k in ["allow", "deny", "ask"] {
        if declared(&format!("tools.permissions.{k}")) {
            if let Some(v) = wp.and_then(|w| w.get(k)) {
                perms.push((k, v));
            }
        }
    }
    if !perms.is_empty() {
        let p = cur
            .entry("permissions")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(p) = p.as_object_mut() {
            for (k, v) in perms {
                p.insert(k.to_string(), v.clone());
            }
        }
    }
    for (k, section, lossy) in [
        ("deniedMcpServers", "tools.denied_mcp_servers", true),
        ("hooks", "tools.hooks", true),
        ("enabledPlugins", "tools.plugins", false),
    ] {
        let Some(v) = want.get(k).filter(|_| declared(section)) else {
            continue;
        };
        match cur.get(k) {
            Some(existing) if lossy && !force && existing != v => notes.push(format!(
                "~/{SETTINGS} {k} (exists and differs; kept, because setup.yaml's model of it is lossy; --force regenerates it)"
            )),
            _ => {
                cur.insert(k.to_string(), v.clone());
            }
        }
    }
    cur
}

/// Add the desired user-scope servers `~/.claude.json` lacks; with `force`, also rewrite the
/// transport of existing ones whose transport differs. Returns the document and the env and
/// header names the added servers expect.
fn merge_claude_json(
    mut doc: Map<String, Value>,
    desired: &Doc,
    force: bool,
    notes: &mut Vec<String>,
) -> (Map<String, Value>, Vec<String>) {
    let mut servers = match doc.get("mcpServers") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    let mut names = Vec::new();
    let denied = arr(get_path(desired, "tools.denied_mcp_servers"));
    for server in arr(get_path(desired, "tools.mcp_servers")) {
        if mcp_excluded(server, denied) {
            if mcp_project_scoped(server) {
                let id = server.get("id").and_then(|v| v.as_str()).unwrap_or("?");
                notes.push(format!(
                    "~/{CLAUDE_JSON} mcpServers.{id} (project-scoped in setup.yaml; not added to the user scope)"
                ));
            }
            continue;
        }
        let (key, def) = mcp_entry(server);
        let at = format!("~/{CLAUDE_JSON} mcpServers.{key}");
        if !installable(&def) {
            notes.push(format!(
                "{at} (no command or url to install; connectors and plugin servers come from their provider)"
            ));
            continue;
        }
        if let Some(cur) = servers.get_mut(&key) {
            if !transport_differs(cur, &def) {
                continue;
            }
            if force {
                rewrite_transport(cur, &def);
            } else {
                notes.push(format!(
                    "{at} (exists and differs; --force rewrites command, args, url, type)"
                ));
            }
        } else {
            names.extend(placeholder_names(&def));
            servers.insert(key, def);
        }
    }
    doc.insert("mcpServers".to_string(), Value::Object(servers));
    (doc, names)
}

fn installable(def: &Value) -> bool {
    let set = |k: &str| {
        def.get(k)
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
    };
    set("command") || set("url")
}

/// The `NAME`s of a server's `${NAME}` env and header placeholders.
fn placeholder_names(def: &Value) -> Vec<String> {
    ["env", "headers"]
        .iter()
        .filter_map(|k| def.get(*k).and_then(|m| m.as_object()))
        .flat_map(|m| m.values())
        .filter_map(|v| v.as_str()?.strip_prefix("${")?.strip_suffix('}'))
        .map(str::to_string)
        .collect()
}

/// A server's transport as Claude Code reads it: `type`, else `http` for a bare url, else stdio.
fn kind(v: &Value) -> &str {
    match v.get("type").and_then(|t| t.as_str()) {
        Some(t) => t,
        None if v.get("url").is_some() && v.get("command").is_none() => "http",
        None => "stdio",
    }
}

fn transport_differs(cur: &Value, want: &Value) -> bool {
    let args = |v: &Value| v.get("args").cloned().unwrap_or_else(|| json!([]));
    kind(cur) != kind(want)
        || cur.get("url") != want.get("url")
        || cur.get("command") != want.get("command")
        || (kind(want) == "stdio" && args(cur) != args(want))
}

/// Set `command`, `args`, `url`, `type` to the desired ones; `env` and `headers` stay.
fn rewrite_transport(cur: &mut Value, want: &Value) {
    let Some(m) = cur.as_object_mut() else {
        *cur = want.clone();
        return;
    };
    for k in ["type", "url", "command", "args"] {
        match want.get(k) {
            Some(v) => {
                m.insert(k.to_string(), v.clone());
            }
            // A stdio server: an explicit "stdio" type stays, a stale http/sse one goes.
            None if k == "type" => {
                if matches!(m.get("type").and_then(|t| t.as_str()), Some("http" | "sse")) {
                    m.shift_remove("type");
                }
            }
            None => {
                m.shift_remove(k);
            }
        }
    }
}

/// What happens to one target: created when missing; otherwise state files are replaced,
/// equivalent JSON is left alone, plists wait for `force`, and anything else is replaced only
/// when its bytes differ. A directory or an unreadable file in the way refuses the run.
fn decide(home: &Path, t: &Target, force: bool) -> Result<Action> {
    let abs = home.join(&t.rel);
    let meta = match std::fs::symlink_metadata(&abs) {
        Ok(m) => m,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Action::Create),
        Err(e) => {
            bail!(
                "refusing: cannot inspect ~/{} ({e}); nothing was backed up or written",
                t.rel
            )
        }
    };
    if meta.is_dir() {
        bail!(
            "refusing: ~/{} is a directory; nothing was backed up or written",
            t.rel
        );
    }
    if t.always {
        return Ok(Action::Update);
    }
    if t.same_as_existing {
        return Ok(Action::Unchanged);
    }
    if t.force_only && !force {
        return Ok(Action::Keep);
    }
    let cur = match std::fs::read(&abs) {
        Ok(b) => b,
        Err(e) => {
            bail!(
                "refusing: cannot read ~/{} ({e}); nothing was backed up or written",
                t.rel
            )
        }
    };
    Ok(if cur == t.content.as_bytes() {
        Action::Unchanged
    } else {
        Action::Update
    })
}

/// `<home>/.setup-as-code/backups/<UTC YYYYMMDDTHHMMSSZ>`, mode 0700, with `-N` appended when a
/// run in the same second already took the name.
fn new_backup_dir(home: &Path) -> Result<PathBuf> {
    let base = home.join(BACKUPS);
    std::fs::create_dir_all(&base).with_context(|| format!("create {}", base.display()))?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    for n in 0..1000 {
        let dir = if n == 0 {
            base.join(&stamp)
        } else {
            base.join(format!("{stamp}-{n}"))
        };
        match std::fs::create_dir(&dir) {
            Ok(()) => {
                set_mode(&dir, 0o700).with_context(|| format!("chmod {}", dir.display()))?;
                return Ok(dir);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("create {}", dir.display())),
        }
    }
    bail!("no free backup directory name under {}", base.display())
}

/// Replace a file atomically: a sibling temp file (0600 while it is written), the old file's
/// permissions (0600 for a new private file), then a rename. A symlinked file is written
/// through to its target, the way an editor would, so a dotfiles link stays a link.
fn write_atomic(path: &Path, bytes: &[u8], private: bool) -> Result<()> {
    let target = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path)
            .with_context(|| format!("resolve symlink {}", path.display()))?,
        _ => path.to_path_buf(),
    };
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", target.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.sac-{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let existing = std::fs::metadata(&target).ok();
    let res = replace_with(&tmp, &target, bytes, existing, private);
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res.with_context(|| format!("write {}", target.display()))
}

fn replace_with(
    tmp: &Path,
    target: &Path,
    bytes: &[u8],
    existing: Option<std::fs::Metadata>,
    private: bool,
) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    if existing.is_some() || private {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    if let Some(m) = existing {
        std::fs::set_permissions(tmp, m.permissions())?;
    }
    std::fs::rename(tmp, target)
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> std::io::Result<()> {
    Ok(())
}

/// Quote a path for a POSIX shell when it holds anything beyond a safe set.
fn shell_quote(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "/._-+@%:,=".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

struct Outcome<'a> {
    targets: &'a [Target],
    skipped: &'a [String],
    backup_dir: Option<&'a Path>,
    commands: &'a [String],
    env_not_written: &'a [String],
}

fn live_report_md(name: &str, home: &Path, at: &str, o: &Outcome) -> String {
    let mut l: Vec<String> = vec![
        format!("# Live apply report: {name}"),
        String::new(),
        format!(
            "Applied {at} into {}. Nothing was loaded, started or sent.",
            home.display()
        ),
        String::new(),
        "## Written".into(),
        String::new(),
    ];
    for t in o.targets.iter().filter(|t| t.writes()) {
        let how = if t.action == Action::Create {
            "created"
        } else {
            "updated"
        };
        l.push(format!("- ~/{} ({how})", t.rel));
    }
    l.push(String::new());
    if !o.skipped.is_empty() {
        l.push("## Kept".into());
        l.push(String::new());
        l.extend(o.skipped.iter().map(|s| format!("- {s}")));
        l.push(String::new());
    }
    l.push("## Backups".into());
    l.push(String::new());
    l.push(match o.backup_dir {
        Some(d) => format!(
            "Every file above that already existed was copied to {} before the first write.",
            d.display()
        ),
        None => "No existing file changed, so nothing was backed up.".into(),
    });
    l.push(String::new());
    if !o.commands.is_empty() {
        l.extend(
            [
                "## Start the launchd jobs by hand",
                "",
                "Nothing was loaded. Run these when ready:",
                "",
                "```",
            ]
            .map(String::from),
        );
        l.extend(o.commands.iter().cloned());
        l.extend(
            [
                "```",
                "",
                "A job that is already loaded needs `launchctl bootout gui/$UID/<label>` first.",
                "",
            ]
            .map(String::from),
        );
    }
    let mut fill: Vec<String> = Vec::new();
    if !o.env_not_written.is_empty() {
        fill.push(format!(
            "~/{SETTINGS} `env` (not written; set these yourself): {}",
            o.env_not_written.join(", ")
        ));
    }
    for t in o
        .targets
        .iter()
        .filter(|t| t.writes() && !t.fill_in.is_empty())
    {
        fill.push(format!("~/{}: {}", t.rel, t.fill_in.join(", ")));
    }
    if !fill.is_empty() {
        l.extend(
            [
                "## Values to fill in",
                "",
                "setup.yaml holds names only. The `${NAME}` placeholders and env names below still need a value:",
                "",
            ]
            .map(String::from),
        );
        l.extend(fill.iter().map(|s| format!("- {s}")));
        l.push(String::new());
    }
    l.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apply::tests::{err_of, fixture};
    use crate::apply::{mcp_servers_doc, plist_xml};
    use serde_json::{json, Value};
    use std::path::Path;

    fn opts(home: &Path, yes: bool, force: bool) -> LiveOptions {
        LiveOptions {
            home: home.to_path_buf(),
            yes,
            force,
        }
    }

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    fn read_json(p: &Path) -> Value {
        serde_json::from_str(&read(p)).unwrap()
    }

    fn entries(dir: &Path) -> usize {
        std::fs::read_dir(dir).map(|d| d.count()).unwrap_or(0)
    }

    fn files_below(dir: &Path) -> Vec<String> {
        let mut out: Vec<String> = walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| {
                e.path()
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        out.sort();
        out
    }

    fn agents(home: &Path) -> std::path::PathBuf {
        home.join("Library/LaunchAgents")
    }

    /// `apply_live` with a stand-in real home next to the given one, so no test looks at the
    /// owner's home.
    fn live(d: &Doc, o: &LiveOptions) -> Result<LiveReport> {
        let real = o.home.parent().unwrap().join("stand-in-real-home");
        apply_live_in(d, o, &real)
    }

    fn canon(p: &Path) -> PathBuf {
        p.canonicalize().unwrap()
    }

    #[test]
    fn live_refuses_without_yes() {
        let home = tempfile::tempdir().unwrap();
        let err = err_of(apply_live(&fixture(), &opts(home.path(), false, false)));
        assert!(err.contains("needs --yes"), "{err}");
        assert_eq!(
            entries(home.path()),
            0,
            "nothing may be written without --yes"
        );
    }

    #[test]
    fn live_into_empty_home_creates_files_and_no_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        let d = fixture();
        let r = live(&d, &opts(home, true, false)).unwrap();

        let settings = read_json(&home.join(".claude/settings.json"));
        assert_eq!(settings["model"], json!("opus"));
        assert_eq!(settings["effortLevel"], json!("high"));
        assert_eq!(
            settings["permissions"],
            json!({ "defaultMode": "acceptEdits", "allow": ["Read", "Bash(git status*)"], "deny": ["Bash(rm -rf*)"], "ask": [] })
        );
        assert_eq!(
            settings["deniedMcpServers"],
            json!([{ "serverName": "work-chat" }])
        );
        assert_eq!(
            settings["hooks"]["PreToolUse"][0]["hooks"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            settings["enabledPlugins"],
            json!({ "helper@example": true, "quiet@example": false })
        );
        assert!(
            settings.get("env").is_none(),
            "env values are unknown, so env is never written"
        );

        let claude_json = read_json(&home.join(".claude.json"));
        assert_eq!(claude_json, mcp_servers_doc(&d));
        assert!(claude_json["mcpServers"].get("off").is_none());
        assert!(claude_json["mcpServers"].get("work-chat").is_none());

        let morning = agents(home).join("com.example.morning.plist");
        assert_eq!(read(&morning), plist_xml(&d["loops"][0], home));
        assert!(agents(home)
            .join("disabled/com.example.sweep.plist")
            .is_file());
        assert_eq!(files_below(&agents(home)).len(), 2);

        let state = home.join(".claude/setup-as-code");
        assert!(read(&state.join("RULES.md")).starts_with("# Rules (alex)\n"));
        assert_eq!(
            read_json(&state.join("setup.lock.json"))["instance"],
            json!("alex")
        );
        let report = read(&state.join("report.md"));
        assert!(report.starts_with("# Live apply report: alex\n"));
        assert!(report.contains("Nothing was loaded, started or sent."));

        assert!(r.backup_dir.is_none());
        assert!(!home.join(".setup-as-code").exists());
        let cmd = format!(
            "launchctl bootstrap gui/$UID {}",
            shell_quote(&morning.to_string_lossy())
        );
        assert_eq!(r.commands, vec![cmd.clone()]);
        assert!(report.contains(&cmd));
        for f in [
            "~/.claude/settings.json",
            "~/.claude.json",
            "~/Library/LaunchAgents/com.example.morning.plist",
            "~/Library/LaunchAgents/disabled/com.example.sweep.plist",
            "~/.claude/setup-as-code/RULES.md",
            "~/.claude/setup-as-code/setup.lock.json",
            "~/.claude/setup-as-code/report.md",
        ] {
            assert!(
                r.changed.iter().any(|c| c == f),
                "{f} not in {:?}",
                r.changed
            );
        }
        assert_eq!(r.changed.len(), 7);
        // Names that still need a value are listed, never values.
        for name in [
            "X_API_KEY",
            "FILES_ROOT",
            "EXAMPLE_TOKEN",
            "EXAMPLE_API_KEY",
        ] {
            assert!(report.contains(name), "{name} missing from the report");
        }
    }

    #[test]
    fn live_merge_keeps_env_and_unmanaged_keys_and_replaces_allow() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let before = r#"{
  "env": { "X": "secret" },
  "theme": "dark",
  "model": "sonnet",
  "permissions": { "allow": ["Bash(*)"], "additionalDirectories": ["~/notes"] },
  "hooks": { "Stop": [ { "hooks": [ { "type": "command", "command": "old.sh" } ] } ] }
}"#;
        std::fs::write(home.join(".claude/settings.json"), before).unwrap();
        let r = live(&fixture(), &opts(home, true, false)).unwrap();

        let s = read_json(&home.join(".claude/settings.json"));
        assert_eq!(s["env"], json!({ "X": "secret" }));
        assert_eq!(s["theme"], json!("dark"));
        assert_eq!(s["model"], json!("opus"));
        assert_eq!(
            s["permissions"]["allow"],
            json!(["Read", "Bash(git status*)"])
        );
        assert_eq!(
            s["permissions"]["additionalDirectories"],
            json!(["~/notes"])
        );
        assert_eq!(s["permissions"]["defaultMode"], json!("acceptEdits"));
        // `hooks` exists and differs, so without --force it stays as found.
        assert_eq!(
            s["hooks"],
            json!({ "Stop": [ { "hooks": [ { "type": "command", "command": "old.sh" } ] } ] })
        );
        let keys: Vec<&String> = s.as_object().unwrap().keys().collect();
        assert_eq!(&keys[..4], ["env", "theme", "model", "permissions"]);

        let dir = r.backup_dir.expect("settings.json existed and changed");
        assert_eq!(dir.parent().unwrap(), home.join(".setup-as-code/backups"));
        let stamp = dir.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            regex::Regex::new(r"^\d{8}T\d{6}Z$")
                .unwrap()
                .is_match(&stamp),
            "{stamp}"
        );
        assert_eq!(read(&dir.join(".claude/settings.json")), before);
        assert_eq!(files_below(&dir), vec![".claude/settings.json"]);
    }

    #[test]
    fn live_existing_mcp_server_keeps_env_and_command_unless_forced() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        let before = r#"{
  "numStartups": 3,
  "mcpServers": {
    "files": { "type": "stdio", "command": "old-files", "args": ["--old"], "env": { "FILES_ROOT": "keep-me" } }
  },
  "projects": {}
}"#;
        std::fs::write(home.join(".claude.json"), before).unwrap();

        let r = live(&fixture(), &opts(home, true, false)).unwrap();
        let cj = read_json(&home.join(".claude.json"));
        assert_eq!(cj["numStartups"], json!(3));
        assert_eq!(cj["projects"], json!({}));
        assert_eq!(
            cj["mcpServers"]["files"],
            json!({ "type": "stdio", "command": "old-files", "args": ["--old"], "env": { "FILES_ROOT": "keep-me" } })
        );
        assert_eq!(
            cj["mcpServers"]["notes"],
            mcp_servers_doc(&fixture())["mcpServers"]["notes"]
        );
        assert!(
            r.skipped.iter().any(|s| s.contains("mcpServers.files")),
            "{:?}",
            r.skipped
        );
        let first_backup = r.backup_dir.expect(".claude.json existed and changed");
        assert_eq!(read(&first_backup.join(".claude.json")), before);

        let between = read(&home.join(".claude.json"));
        let r = live(&fixture(), &opts(home, true, true)).unwrap();
        let cj = read_json(&home.join(".claude.json"));
        assert_eq!(
            cj["mcpServers"]["files"],
            json!({ "type": "stdio", "command": "npx", "args": ["-y", "example-files"], "env": { "FILES_ROOT": "keep-me" } })
        );
        assert_eq!(cj["numStartups"], json!(3));
        let second_backup = r.backup_dir.expect("forced rewrite changes .claude.json");
        assert_ne!(first_backup, second_backup);
        assert_eq!(read(&second_backup.join(".claude.json")), between);
    }

    #[test]
    fn live_refuses_malformed_settings_json() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude/settings.json"), "{ not json").unwrap();
        let err = err_of(live(&fixture(), &opts(home, true, true)));
        assert!(
            err.contains("~/.claude/settings.json is not valid JSON"),
            "{err}"
        );
        assert_eq!(read(&home.join(".claude/settings.json")), "{ not json");
        assert!(!home.join(".setup-as-code").exists());
        assert!(!home.join(".claude.json").exists());
        assert!(!home.join("Library").exists());
        assert_eq!(entries(&home.join(".claude")), 1);
    }

    #[test]
    fn live_refuses_malformed_claude_json() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        std::fs::write(home.join(".claude.json"), "[1, 2").unwrap();
        let err = err_of(live(&fixture(), &opts(home, true, false)));
        assert!(err.contains("~/.claude.json is not valid JSON"), "{err}");
        assert_eq!(read(&home.join(".claude.json")), "[1, 2");
        assert_eq!(entries(home), 1, "no backup and no other write");

        std::fs::write(home.join(".claude.json"), r#"{ "mcpServers": [] }"#).unwrap();
        let err = err_of(live(&fixture(), &opts(home, true, false)));
        assert!(err.contains("mcpServers"), "{err}");
        assert_eq!(entries(home), 1);
    }

    #[test]
    fn live_writes_disabled_loop_plist_under_disabled_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        let d = fixture();
        let r = live(&d, &opts(home, true, false)).unwrap();
        let disabled = agents(home).join("disabled/com.example.sweep.plist");
        assert_eq!(read(&disabled), plist_xml(&d["loops"][1], home));
        assert!(!agents(home).join("com.example.sweep.plist").exists());
        assert!(!agents(home)
            .join("disabled/com.example.morning.plist")
            .exists());
        assert!(r.commands.iter().all(|c| !c.contains("sweep")));
    }

    #[test]
    fn live_keeps_an_existing_plist_unless_forced() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        std::fs::create_dir_all(agents(home)).unwrap();
        let morning = agents(home).join("com.example.morning.plist");
        std::fs::write(&morning, "old plist").unwrap();

        let r = live(&fixture(), &opts(home, true, false)).unwrap();
        assert_eq!(read(&morning), "old plist");
        assert!(r.commands.is_empty(), "{:?}", r.commands);
        assert!(r
            .skipped
            .iter()
            .any(|s| s.starts_with("~/Library/LaunchAgents/com.example.morning.plist")));
        assert!(r.backup_dir.is_none(), "nothing that existed changed");

        let r = live(&fixture(), &opts(home, true, true)).unwrap();
        assert_eq!(read(&morning), plist_xml(&fixture()["loops"][0], home));
        assert_eq!(r.commands.len(), 1);
        let backup = r.backup_dir.unwrap();
        assert_eq!(
            read(&backup.join("Library/LaunchAgents/com.example.morning.plist")),
            "old plist"
        );
    }

    #[test]
    fn live_second_run_backs_up_only_what_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        live(&fixture(), &opts(home, true, false)).unwrap();
        let r = live(&fixture(), &opts(home, true, false)).unwrap();
        for unchanged in ["~/.claude/settings.json", "~/.claude.json"] {
            assert!(!r.changed.iter().any(|c| c == unchanged), "{unchanged}");
            assert!(r.skipped.iter().any(|s| s.starts_with(unchanged)));
        }
        let backup = r.backup_dir.expect("the report is rewritten every run");
        let saved = files_below(&backup);
        assert!(saved.contains(&".claude/setup-as-code/report.md".to_string()));
        assert!(!saved.contains(&".claude/settings.json".to_string()));
        assert!(!saved.contains(&".claude.json".to_string()));
        assert!(r.commands.is_empty(), "existing plists are kept");
    }

    #[test]
    fn live_writes_the_state_files_every_run() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        live(&fixture(), &opts(home, true, false)).unwrap();
        let r = live(&fixture(), &opts(home, true, false)).unwrap();
        for f in ["RULES.md", "setup.lock.json", "report.md"] {
            let p = format!("~/.claude/setup-as-code/{f}");
            assert!(r.changed.contains(&p), "{p} not in {:?}", r.changed);
        }
        let report = read(&home.join(".claude/setup-as-code/report.md"));
        assert!(report.contains("- ~/.claude/settings.json (unchanged)"));
        assert!(report.contains("- ~/.claude/setup-as-code/report.md (updated)"));
    }

    #[test]
    fn live_leaves_an_equivalent_json_file_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        live(&fixture(), &opts(home, true, false)).unwrap();
        let path = home.join(".claude/settings.json");
        let compact = serde_json::to_string(&read_json(&path)).unwrap();
        std::fs::write(&path, &compact).unwrap();
        let r = live(&fixture(), &opts(home, true, false)).unwrap();
        assert_eq!(read(&path), compact, "same JSON must not be reformatted");
        assert!(r
            .skipped
            .contains(&"~/.claude/settings.json (unchanged)".to_string()));
    }

    #[test]
    fn live_refuses_unsafe_loop_label_before_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut d = fixture();
        d["loops"][0]["id"] = json!("../../.ssh/evil");
        let err = err_of(live(&d, &opts(tmp.path(), true, false)));
        assert!(err.contains("not a plain file name"), "{err}");
        assert_eq!(entries(tmp.path()), 0);
    }

    #[test]
    fn live_leaves_project_scoped_and_shapeless_servers_out_of_the_user_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        let mut d = fixture();
        d["tools"]["mcp_servers"]
            .as_array_mut()
            .unwrap()
            .extend([
                json!({ "id": "proj", "source": "~/.claude.json#projects/~/work", "transport": "stdio", "command": "proj-server" }),
                json!({ "id": "linked", "transport": "connector" }),
            ]);
        let r = live(&d, &opts(home, true, false)).unwrap();
        let servers = read_json(&home.join(".claude.json"))["mcpServers"].clone();
        assert!(servers.get("proj").is_none());
        assert!(servers.get("linked").is_none());
        assert!(servers.get("files").is_some());
        assert!(r.skipped.iter().any(|s| s.contains("mcpServers.proj")));
        assert!(r.skipped.iter().any(|s| s.contains("mcpServers.linked")));
    }

    #[test]
    fn live_never_lets_an_at_suffixed_duplicate_touch_its_namesake() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        let d = json!({
            "version": 1,
            "meta": { "instance": "alex" },
            "tools": { "mcp_servers": [
                { "id": "notes", "transport": "http", "url": "https://mcp.example.com/notes", "header_names": ["Authorization"] },
                { "id": "notes@~/.claude/mcp.json", "source": "~/.claude/mcp.json", "transport": "stdio", "command": "notes-local" }
            ]}
        });
        for force in [false, true] {
            let r = live(&d, &opts(home, true, force)).unwrap();
            let servers = read_json(&home.join(".claude.json"))["mcpServers"].clone();
            assert_eq!(servers, mcp_servers_doc(&d)["mcpServers"], "force={force}");
            assert_eq!(
                servers["notes"]["headers"]["Authorization"],
                json!("${AUTHORIZATION}")
            );
            assert!(
                !r.skipped.iter().any(|s| s.contains("differs")),
                "{:?}",
                r.skipped
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn live_follows_symlinks_that_stay_inside_the_home() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path()).join("home");
        // A dotfiles checkout inside the home: ~/.claude/settings.json is a link into it, and
        // so is the whole ~/Library/LaunchAgents directory.
        let dotfiles = home.join("dotfiles");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(dotfiles.join("agents")).unwrap();
        std::fs::create_dir_all(home.join("Library")).unwrap();
        std::fs::write(dotfiles.join("settings.json"), r#"{ "theme": "dark" }"#).unwrap();
        let link = home.join(".claude/settings.json");
        symlink(dotfiles.join("settings.json"), &link).unwrap();
        symlink(dotfiles.join("agents"), agents(home)).unwrap();

        let r = live(&fixture(), &opts(home, true, false)).unwrap();
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        let s = read_json(&dotfiles.join("settings.json"));
        assert_eq!(s["theme"], json!("dark"));
        assert_eq!(s["model"], json!("opus"));
        assert!(dotfiles.join("agents/com.example.morning.plist").is_file());
        assert!(dotfiles
            .join("agents/disabled/com.example.sweep.plist")
            .is_file());
        let backup = r.backup_dir.unwrap();
        assert_eq!(
            read(&backup.join(".claude/settings.json")),
            r#"{ "theme": "dark" }"#
        );
    }

    /// Run against a home holding one planted symlink that leads outside it; the run must be
    /// refused with nothing written, inside the home or out.
    #[cfg(unix)]
    fn assert_refused_with_link(at: &str, outside_is_dir: bool) {
        let tmp = tempfile::tempdir().unwrap();
        let root = canon(tmp.path());
        let home = root.join("home");
        let outside = root.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let target = if outside_is_dir {
            outside.clone()
        } else {
            std::fs::write(outside.join("file.json"), r#"{ "theme": "dark" }"#).unwrap();
            outside.join("file.json")
        };
        let link = home.join(at);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let before = files_below(&home);

        let err = err_of(live(&fixture(), &opts(&home, true, true)));
        assert!(err.contains("outside the home"), "{at}: {err}");
        assert_eq!(files_below(&home), before, "{at}: the home changed");
        for created in [".claude.json", ".setup-as-code"] {
            if at != created {
                let absent = std::fs::symlink_metadata(home.join(created)).is_err();
                assert!(absent, "{at}: {created} was created");
            }
        }
        let left: Vec<String> = files_below(&outside);
        let expected: Vec<String> = if outside_is_dir {
            Vec::new()
        } else {
            vec!["file.json".to_string()]
        };
        assert_eq!(left, expected, "{at}: something was written outside");
        if !outside_is_dir {
            assert_eq!(read(&target), r#"{ "theme": "dark" }"#);
        }
    }

    #[cfg(unix)]
    #[test]
    fn live_refuses_a_claude_dir_symlinked_outside_the_home() {
        assert_refused_with_link(".claude", true);
    }

    #[cfg(unix)]
    #[test]
    fn live_refuses_files_and_dirs_symlinked_outside_the_home() {
        assert_refused_with_link(".claude/settings.json", false);
        assert_refused_with_link(".claude.json", false);
        assert_refused_with_link("Library", true);
        assert_refused_with_link(".claude/setup-as-code", true);
        assert_refused_with_link(".setup-as-code", true);
    }

    #[cfg(unix)]
    #[test]
    fn live_refuses_a_dangling_symlink_on_the_way() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path()).join("home");
        std::fs::create_dir_all(home).unwrap();
        std::os::unix::fs::symlink(home.join("missing"), home.join(".claude")).unwrap();
        let err = err_of(live(&fixture(), &opts(home, true, false)));
        assert!(err.contains("dangling symlink"), "{err}");
        assert_eq!(entries(home), 1);
    }

    #[cfg(unix)]
    #[test]
    fn live_keeps_a_sandbox_out_of_the_real_home() {
        let tmp = tempfile::tempdir().unwrap();
        let root = canon(tmp.path());

        // The given home contains the real one, and a link inside it leads there.
        let outer = root.join("outer");
        let real = outer.join("me");
        std::fs::create_dir_all(real.join(".claude")).unwrap();
        std::os::unix::fs::symlink(real.join(".claude"), outer.join(".claude")).unwrap();
        let err = err_of(apply_live_in(&fixture(), &opts(&outer, true, false), &real));
        assert!(err.contains("inside the real home"), "{err}");
        assert_eq!(entries(&real.join(".claude")), 0);
        assert!(!outer.join(".claude.json").exists());

        // A sandbox that lives inside the real home may write inside itself...
        let real = root.join("me2");
        let sandbox = real.join("sandbox");
        std::fs::create_dir_all(&sandbox).unwrap();
        let r = apply_live_in(&fixture(), &opts(&sandbox, true, false), &real).unwrap();
        assert!(sandbox.join(".claude/settings.json").is_file());
        assert_eq!(entries(&real), 1, "only the sandbox itself");
        assert_eq!(r.changed.len(), 7);

        // ...but never into the real home's protected config.
        let real = root.join("me3");
        let sandbox = real.join(".claude/sandbox");
        std::fs::create_dir_all(&sandbox).unwrap();
        let err = err_of(apply_live_in(
            &fixture(),
            &opts(&sandbox, true, false),
            &real,
        ));
        assert!(
            err.contains("real home's protected path ~/.claude"),
            "{err}"
        );
        assert_eq!(entries(&sandbox), 0);
    }

    #[test]
    fn live_and_render_both_skip_a_project_scoped_server() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        // `proj` has a unique bare name, so no `@` rule applies: only its source scopes it.
        let d = json!({
            "version": 1,
            "meta": { "instance": "alex" },
            "tools": { "mcp_servers": [
                { "id": "notes", "source": "~/.claude.json", "transport": "http", "url": "https://mcp.example.com/notes" },
                { "id": "proj", "source": "~/.claude.json#projects/~/work", "transport": "stdio", "command": "proj-server" }
            ]}
        });
        let rendered = mcp_servers_doc(&d);
        assert!(rendered["mcpServers"].get("proj").is_none(), "{rendered}");
        assert!(rendered["mcpServers"].get("notes").is_some(), "{rendered}");

        let r = live(&d, &opts(home, true, false)).unwrap();
        let servers = read_json(&home.join(".claude.json"))["mcpServers"].clone();
        assert_eq!(servers, rendered["mcpServers"]);
        let note = "~/.claude.json mcpServers.proj (project-scoped in setup.yaml; not added to the user scope)";
        assert!(r.skipped.contains(&note.to_string()), "{:?}", r.skipped);
    }

    #[test]
    fn live_absent_manifest_sections_leave_live_keys_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let before = r#"{
  "model": "sonnet",
  "permissions": { "defaultMode": "plan", "allow": ["Bash(ls*)"], "deny": ["Bash(rm -rf*)"] },
  "deniedMcpServers": [{ "serverName": "bad" }],
  "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "done.sh" }] }] },
  "enabledPlugins": { "helper@example": true }
}"#;
        let path = home.join(".claude/settings.json");
        std::fs::write(&path, before).unwrap();

        // No `tools` (and no `brain`) at all: every managed key keeps its live value.
        let bare = json!({ "version": 1, "meta": { "instance": "alex" } });
        let r = live(&bare, &opts(home, true, false)).unwrap();
        assert_eq!(read(&path), before);
        assert!(r
            .skipped
            .contains(&"~/.claude/settings.json (unchanged)".to_string()));

        // A present section replaces its key, even when empty (`hooks` only with --force,
        // since setup.yaml models hooks lossily); absent ones stay, with --force too.
        let some = json!({ "version": 1, "meta": { "instance": "alex" },
                           "tools": { "hooks": [], "permissions": { "allow": ["Read"] } } });
        live(&some, &opts(home, true, false)).unwrap();
        let s = read_json(&path);
        assert_eq!(
            s["hooks"],
            json!({ "Stop": [{ "hooks": [{ "type": "command", "command": "done.sh" }] }] })
        );
        assert_eq!(s["permissions"]["allow"], json!(["Read"]));
        live(&some, &opts(home, true, true)).unwrap();
        let s = read_json(&path);
        assert_eq!(s["hooks"], json!({}));
        assert_eq!(s["permissions"]["allow"], json!(["Read"]));
        assert_eq!(s["permissions"]["deny"], json!(["Bash(rm -rf*)"]));
        assert_eq!(s["permissions"]["defaultMode"], json!("plan"));
        assert_eq!(s["deniedMcpServers"], json!([{ "serverName": "bad" }]));
        assert_eq!(s["enabledPlugins"], json!({ "helper@example": true }));
        assert_eq!(s["model"], json!("sonnet"));
    }

    /// The note live leaves for a lossy settings key it kept.
    fn lossy_note(key: &str) -> String {
        format!("~/.claude/settings.json {key} (exists and differs; kept, because setup.yaml's model of it is lossy; --force regenerates it)")
    }

    #[test]
    fn live_keeps_existing_hooks_and_denied_servers_unless_forced() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        // What setup.yaml cannot represent: a prompt hook with a timeout, a denial by URL.
        let hooks = json!({ "Stop": [{ "hooks": [{ "type": "prompt", "prompt": "Check the work.", "timeout": 30 }] }] });
        let denied = json!([{ "serverUrl": "https://chat.example.com/*" }]);
        let path = home.join(".claude/settings.json");
        let before = json!({ "model": "sonnet", "hooks": hooks, "deniedMcpServers": denied });
        std::fs::write(&path, pretty_json(&before)).unwrap();
        let want = crate::apply::claude_settings_doc(&fixture());

        let r = live(&fixture(), &opts(home, true, false)).unwrap();
        let s = read_json(&path);
        assert_eq!(s["hooks"], hooks);
        assert_eq!(s["deniedMcpServers"], denied);
        // The other managed keys still come from setup.yaml.
        assert_eq!(s["model"], json!("opus"));
        assert_eq!(s["permissions"], want["permissions"]);
        for key in ["hooks", "deniedMcpServers"] {
            assert!(r.skipped.contains(&lossy_note(key)), "{:?}", r.skipped);
        }
        let report = read(&home.join(".claude/setup-as-code/report.md"));
        assert!(
            report.contains(&format!("- {}", lossy_note("hooks"))),
            "{report}"
        );

        let r = live(&fixture(), &opts(home, true, true)).unwrap();
        let s = read_json(&path);
        assert_eq!(s["hooks"], want["hooks"]);
        assert_eq!(s["deniedMcpServers"], want["deniedMcpServers"]);
        assert!(
            !r.skipped.iter().any(|x| x.contains("lossy")),
            "{:?}",
            r.skipped
        );
        let backup = r.backup_dir.expect("the forced run rewrote settings.json");
        assert_eq!(
            read_json(&backup.join(".claude/settings.json"))["hooks"],
            hooks
        );

        // Once they match setup.yaml there is nothing to keep and nothing to say.
        let r = live(&fixture(), &opts(home, true, false)).unwrap();
        assert!(
            !r.skipped.iter().any(|x| x.contains("lossy")),
            "{:?}",
            r.skipped
        );
        assert!(r
            .skipped
            .contains(&"~/.claude/settings.json (unchanged)".to_string()));
    }

    #[test]
    fn live_writes_hooks_and_denied_servers_a_settings_file_lacks() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let path = home.join(".claude/settings.json");
        std::fs::write(&path, r#"{ "theme": "dark" }"#).unwrap();
        let r = live(&fixture(), &opts(home, true, false)).unwrap();
        let s = read_json(&path);
        let want = crate::apply::claude_settings_doc(&fixture());
        assert_eq!(s["hooks"], want["hooks"]);
        assert_eq!(s["deniedMcpServers"], want["deniedMcpServers"]);
        assert_eq!(s["theme"], json!("dark"));
        assert!(
            !r.skipped.iter().any(|x| x.contains("lossy")),
            "{:?}",
            r.skipped
        );
    }

    /// The note live leaves for a file it refused because of the placeholder.
    fn placeholder_note(rel: &str, at: &str) -> String {
        format!("~/{rel} (not written: setup.yaml would put the placeholder `<redacted>` at `{at}`, where import dropped a secret-shaped value)")
    }

    #[test]
    fn live_never_writes_the_redacted_placeholder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        let d = crate::apply::tests::fixture_with_placeholders();
        let r = live(&d, &opts(home, true, false)).unwrap();
        // Each file the placeholder would reach is skipped whole, never written without it.
        assert!(!home.join(".claude/settings.json").exists());
        assert!(!home.join(".claude.json").exists());
        assert!(!agents(home).join("com.example.morning.plist").exists());
        assert!(agents(home)
            .join("disabled/com.example.sweep.plist")
            .is_file());
        assert!(r.commands.is_empty(), "{:?}", r.commands);
        for note in [
            placeholder_note(".claude/settings.json", "permissions.allow[2]"),
            placeholder_note(".claude.json", "mcpServers.files.args[1]"),
            placeholder_note(
                "Library/LaunchAgents/com.example.morning.plist",
                "program[1]",
            ),
        ] {
            assert!(r.skipped.contains(&note), "{note}\n{:?}", r.skipped);
        }
        let mut changed = r.changed.clone();
        changed.sort();
        assert_eq!(
            changed,
            [
                "~/.claude/setup-as-code/RULES.md",
                "~/.claude/setup-as-code/report.md",
                "~/.claude/setup-as-code/setup.lock.json",
                "~/Library/LaunchAgents/disabled/com.example.sweep.plist",
            ]
        );
        // Only the report, which names the refused files, mentions the placeholder.
        let report = read(&home.join(".claude/setup-as-code/report.md"));
        assert!(
            report.contains(&format!(
                "- {}",
                placeholder_note(".claude.json", "mcpServers.files.args[1]")
            )),
            "{report}"
        );
        for f in files_below(home) {
            let text = read(&home.join(&f));
            let carries = text.contains("<redacted>") || text.contains("&lt;redacted&gt;");
            assert_eq!(carries, f == ".claude/setup-as-code/report.md", "{f}");
        }
    }

    #[test]
    fn live_leaves_existing_files_whole_when_the_placeholder_would_land_in_them() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(agents(home)).unwrap();
        let settings = r#"{ "theme": "dark" }"#;
        let claude_json = r#"{ "numStartups": 3 }"#;
        let morning = agents(home).join("com.example.morning.plist");
        std::fs::write(home.join(".claude/settings.json"), settings).unwrap();
        std::fs::write(home.join(".claude.json"), claude_json).unwrap();
        std::fs::write(&morning, "old plist").unwrap();

        let d = crate::apply::tests::fixture_with_placeholders();
        let r = live(&d, &opts(home, true, true)).unwrap();
        assert_eq!(read(&home.join(".claude/settings.json")), settings);
        assert_eq!(read(&home.join(".claude.json")), claude_json);
        assert_eq!(read(&morning), "old plist");
        assert!(r.commands.is_empty(), "{:?}", r.commands);
        assert!(r.backup_dir.is_none(), "nothing that existed was written");
        assert!(r.skipped.contains(&placeholder_note(
            "Library/LaunchAgents/com.example.morning.plist",
            "program[1]"
        )));
    }

    #[test]
    fn live_writes_a_file_that_already_held_the_placeholder() {
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        // The word sits in the file already (say, a past prompt); this run adds none.
        let before =
            json!({ "projects": { "/w": { "history": [{ "display": "why <redacted>?" }] } } });
        std::fs::write(home.join(".claude.json"), pretty_json(&before)).unwrap();
        let r = live(&fixture(), &opts(home, true, false)).unwrap();
        let cj = read_json(&home.join(".claude.json"));
        assert_eq!(cj["projects"], before["projects"]);
        assert_eq!(cj["mcpServers"], mcp_servers_doc(&fixture())["mcpServers"]);
        assert!(r.changed.contains(&"~/.claude.json".to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn live_new_claude_json_is_private_and_backups_are_closed() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let home = &canon(tmp.path());
        live(&fixture(), &opts(home, true, false)).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&home.join(".claude.json")), 0o600);
        let r = live(&fixture(), &opts(home, true, false)).unwrap();
        assert_eq!(mode(&r.backup_dir.unwrap()), 0o700);
    }
}
