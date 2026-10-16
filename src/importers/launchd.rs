//! macOS launchd user agents: every job in `~/Library/LaunchAgents`, including parked files
//! (`*.plist.disabled-*`, `*.plist.killed-*`, a `disabled/` subfolder), labels the system has
//! marked disabled without a plist on disk, and live load state. Strictly read-only: plists are
//! parsed in-process, and `launchctl list` / `launchctl print-disabled` run only for the real
//! home on macOS. Nothing is ever loaded, unloaded or bootstrapped.
//!
//! `options.managed_prefixes`: label prefixes that belong to this person (others are vendor jobs).

use super::Importer;
use crate::ctx::Ctx;
use crate::util::{is_truthy, str_list};
use crate::Doc;
use anyhow::Result;
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

const DIR: &str = "~/Library/LaunchAgents";

pub struct Launchd;

/// Machine-wide launchd state. Gathered only for the real home; a sandbox home sees none of it,
/// so there every installed plist counts as enabled.
#[derive(Default)]
struct Probes {
    /// Importing the machine's real user (`ctx.is_real_home()`).
    real: bool,
    /// `launchctl list`: label to pid (`None` when the job is loaded but not running).
    loaded: HashMap<String, Option<i64>>,
    /// `launchctl print-disabled gui/<uid>`: label to "has a disabled override", in output order.
    disabled: IndexMap<String, bool>,
}

impl Probes {
    fn gather(ctx: &Ctx) -> Probes {
        let mut p = Probes {
            real: true,
            ..Probes::default()
        };
        if ctx.platform() != "darwin" {
            return p;
        }
        if let Some(list) = ctx.try_exec("launchctl", &["list"]) {
            p.loaded = parse_launchctl_list(&list);
        }
        if let Some(uid) = ctx.uid() {
            let domain = format!("gui/{uid}");
            if let Some(pd) = ctx.try_exec("launchctl", &["print-disabled", &domain]) {
                p.disabled = parse_print_disabled(&pd);
            }
        }
        p
    }
}

/// `launchctl list` rows (`PID Status Label`, header first) by label.
fn parse_launchctl_list(text: &str) -> HashMap<String, Option<i64>> {
    let mut out = HashMap::new();
    for line in text.split('\n').skip(1) {
        let mut parts = line.split_whitespace();
        let (pid, _status, label) = (parts.next(), parts.next(), parts.next());
        if let (Some(pid), Some(label)) = (pid, label) {
            let pid = if pid == "-" { None } else { pid.parse().ok() };
            out.insert(label.to_string(), pid);
        }
    }
    out
}

/// `"label" => disabled|enabled|true|false` lines; a later line for a label wins.
fn parse_print_disabled(text: &str) -> IndexMap<String, bool> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#""([^"]+)"\s*=>\s*(enabled|disabled|true|false)"#).expect("regex compiles")
    });
    let mut out = IndexMap::new();
    for c in re.captures_iter(text) {
        out.insert(c[1].to_string(), matches!(&c[2], "disabled" | "true"));
    }
    out
}

/// A plist value as a document. Dates become RFC 3339 strings, data becomes `"<data>"`, and a
/// real with no fractional part becomes an integer (as it would after a JSON round trip).
fn plist_to_doc(v: &plist::Value) -> Doc {
    match v {
        plist::Value::Array(a) => Value::Array(a.iter().map(plist_to_doc).collect()),
        plist::Value::Dictionary(d) => Value::Object(
            d.iter()
                .map(|(k, x)| (k.clone(), plist_to_doc(x)))
                .collect(),
        ),
        plist::Value::Boolean(b) => Value::Bool(*b),
        plist::Value::Data(_) => Value::String("<data>".into()),
        plist::Value::Date(d) => Value::String(d.to_xml_format()),
        plist::Value::Real(f) => {
            if f.fract() == 0.0 && f.abs() < 9.007_199_254_740_992e15 {
                json!(*f as i64)
            } else {
                serde_json::Number::from_f64(*f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null)
            }
        }
        plist::Value::Integer(i) => i
            .as_signed()
            .map(Value::from)
            .or_else(|| i.as_unsigned().map(Value::from))
            .unwrap_or(Value::Null),
        plist::Value::String(s) => Value::String(s.clone()),
        plist::Value::Uid(u) => Value::from(u.get()),
        _ => Value::Null,
    }
}

/// Read one plist file of any encoding; `None` when it cannot be parsed.
fn read_plist(path: &Path) -> Option<Doc> {
    plist::Value::from_file(path).ok().map(|v| plist_to_doc(&v))
}

/// JavaScript `String(v)` for the scalar values a plist can hold.
fn js_string(v: &Doc) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 1e21 => format!("{f:.0}"),
            _ => n.to_string(),
        },
        Value::Array(a) => a
            .iter()
            .map(|x| {
                if x.is_null() {
                    String::new()
                } else {
                    js_string(x)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// One `StartCalendarInterval` dict as `hour=9,minute=0`, keys in file order.
fn calendar_entry(c: &Doc) -> String {
    match c {
        Value::Object(o) => o
            .iter()
            .map(|(k, v)| format!("{}={}", k.to_lowercase(), js_string(v)))
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    }
}

fn schedule(ctx: &Ctx, j: &Doc) -> Doc {
    let mut s = Map::new();
    if is_truthy(j.get("StartInterval")) {
        s.insert("every_seconds".into(), j["StartInterval"].clone());
    }
    if is_truthy(j.get("StartCalendarInterval")) {
        let cal: Vec<Doc> = match &j["StartCalendarInterval"] {
            Value::Array(a) => a.iter().map(|c| Value::String(calendar_entry(c))).collect(),
            one => vec![Value::String(calendar_entry(one))],
        };
        s.insert("calendar".into(), Value::Array(cal));
    }
    if is_truthy(j.get("KeepAlive")) {
        s.insert("keep_alive".into(), Value::Bool(true));
    }
    if is_truthy(j.get("RunAtLoad")) {
        s.insert("run_at_load".into(), Value::Bool(true));
    }
    if is_truthy(j.get("WatchPaths")) {
        s.insert("watch_paths".into(), ctx.scrub(&j["WatchPaths"]));
    }
    Value::Object(s)
}

/// `ProgramArguments`, else `[Program]`, as strings, scrubbed.
fn program(ctx: &Ctx, j: &Doc) -> Doc {
    let args: Vec<Doc> = match j.get("ProgramArguments") {
        Some(Value::Array(a)) => a.iter().map(|x| Value::String(js_string(x))).collect(),
        Some(v) if is_truthy(Some(v)) => vec![Value::String(js_string(v))],
        _ => match j.get("Program") {
            Some(p) if is_truthy(Some(p)) => vec![Value::String(js_string(p))],
            _ => Vec::new(),
        },
    };
    ctx.scrub(&Value::Array(args))
}

fn env_names(j: &Doc) -> Doc {
    let mut keys: Vec<String> = match j.get("EnvironmentVariables") {
        Some(Value::Object(o)) => o.keys().cloned().collect(),
        _ => Vec::new(),
    };
    keys.sort();
    Value::Array(keys.into_iter().map(Value::String).collect())
}

fn managed_prefixes(opts: &Doc) -> Vec<String> {
    match opts.get("managed_prefixes") {
        Some(Value::String(p)) if !p.is_empty() => vec![p.clone()],
        v => str_list(v),
    }
}

/// Primary collation weight of a character: whitespace, then punctuation and symbols in ICU
/// root order, then digits, then letters without case. Non-ASCII sorts after all of ASCII.
fn primary(c: char) -> u32 {
    const ORDER: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789";
    if let Some(i) = ORDER.find(c) {
        return i as u32 + 1;
    }
    if c.is_ascii_alphabetic() {
        return 100 + (c.to_ascii_lowercase() as u32 - 'a' as u32);
    }
    if c.is_ascii_control() {
        return 0;
    }
    1000 + c as u32
}

/// `String.prototype.localeCompare` (ICU, English) for ASCII text: compare without case first,
/// then lowercase before uppercase from the left, then by bytes.
fn locale_cmp(a: &str, b: &str) -> Ordering {
    let upper = |c: char| c.is_ascii_uppercase();
    a.chars()
        .map(primary)
        .cmp(b.chars().map(primary))
        .then_with(|| a.chars().map(upper).cmp(b.chars().map(upper)))
        .then_with(|| a.cmp(b))
}

/// One file of a job: where it is and how good a description of the job it is.
struct JobFile {
    rel: String,
    /// 0 installed plist, 1 parked copy (disabled/killed/renamed), 2 backup copy.
    rank: u8,
    /// What follows `.plist` in the file name, without the leading dot.
    suffix: String,
    parked: bool,
    plist: Option<Doc>,
}

fn status_of(best: &JobFile) -> String {
    static DATE_TAIL: OnceLock<Regex> = OnceLock::new();
    match best.rank {
        0 => "installed".into(),
        2 => "backup-only".into(),
        _ if best.parked && best.suffix.is_empty() => "parked".into(),
        _ => {
            let re =
                DATE_TAIL.get_or_init(|| Regex::new(r"-?[0-9]{6,8}$").expect("regex compiles"));
            let base = re.replace(&best.suffix, "");
            if base.is_empty() {
                "parked:renamed".into()
            } else {
                format!("parked:{base}")
            }
        }
    }
}

fn scan(ctx: &Ctx, opts: &Doc, probes: &Probes) -> Doc {
    let prefixes = managed_prefixes(opts);
    let owned = |label: &str| prefixes.iter().any(|p| label.starts_with(p.as_str()));

    let disabled_dir = format!("{DIR}/disabled");
    let files = ctx
        .list(DIR)
        .into_iter()
        .map(|f| (format!("{DIR}/{f}"), f, false))
        .chain(
            ctx.list(&disabled_dir)
                .into_iter()
                .map(|f| (format!("{disabled_dir}/{f}"), f, true)),
        );

    // Group every file by label, then describe the job from its best file:
    // installed plist > parked copy (disabled/killed/renamed) > backup copy.
    let mut groups: IndexMap<String, Vec<JobFile>> = IndexMap::new();
    for (rel, f, parked) in files {
        let Some(at) = f.find(".plist") else { continue };
        let after = &f[at + ".plist".len()..];
        let suffix = after.strip_prefix('.').unwrap_or(after).to_string();
        let rank = if f.ends_with(".plist") && !parked {
            0
        } else if suffix.starts_with("bak") {
            2
        } else {
            1
        };
        let plist = read_plist(&ctx.p(&rel));
        let label = plist
            .as_ref()
            .and_then(|j| j.get("Label"))
            .and_then(|l| l.as_str())
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| f[..at].to_string());
        groups.entry(label).or_default().push(JobFile {
            rel,
            rank,
            suffix,
            parked,
            plist,
        });
    }

    let mut jobs: IndexMap<String, Map<String, Value>> = IndexMap::new();
    for (label, mut list) in groups {
        list.sort_by_key(|x| x.rank);
        let best = &list[0];
        let mut job = Map::new();
        job.insert("id".into(), json!(label));
        job.insert("runner".into(), json!("launchd"));
        job.insert("enabled".into(), json!(false));
        job.insert("status".into(), json!(status_of(best)));
        job.insert(
            "managed".into(),
            json!(prefixes.is_empty() || owned(&label)),
        );
        match &best.plist {
            Some(j) => {
                job.insert("schedule".into(), schedule(ctx, j));
                job.insert("program".into(), program(ctx, j));
                if let Some(wd) = j.get("WorkingDirectory").and_then(|v| v.as_str()) {
                    if !wd.is_empty() {
                        job.insert("working_dir".into(), json!(ctx.tilde(wd)));
                    }
                }
                if let Some(log) = j.get("StandardOutPath").and_then(|v| v.as_str()) {
                    if !log.is_empty() {
                        job.insert("log".into(), json!(ctx.tilde(log)));
                    }
                }
                job.insert("env_names".into(), env_names(j));
            }
            None => {
                job.insert("schedule".into(), json!({}));
                job.insert("program".into(), json!([]));
                job.insert("env_names".into(), json!([]));
            }
        }
        let rels: Vec<&str> = list.iter().map(|x| x.rel.as_str()).collect();
        job.insert("files".into(), json!(rels));
        jobs.insert(label, job);
    }

    // Labels launchd has an override for but no plist on disk: removed jobs.
    for (label, &is_disabled) in &probes.disabled {
        if jobs.contains_key(label) || (!prefixes.is_empty() && !owned(label)) {
            continue;
        }
        let status = if is_disabled {
            "removed:override-disabled"
        } else {
            "removed:override-enabled"
        };
        let job = json!({
            "id": label, "runner": "launchd", "enabled": false, "status": status, "managed": true,
            "schedule": {}, "program": [], "env_names": [], "files": []
        });
        if let Value::Object(m) = job {
            jobs.insert(label.clone(), m);
        }
    }

    for (label, job) in jobs.iter_mut() {
        let override_disabled = probes.disabled.get(label) == Some(&true);
        if job.get("status").and_then(|s| s.as_str()) == Some("installed") {
            let loaded = probes.loaded.get(label);
            let enabled = !override_disabled && (!probes.real || loaded.is_some());
            let status = if override_disabled {
                "installed:override-disabled"
            } else if let Some(pid) = loaded {
                if pid.is_some_and(|p| p != 0) {
                    "running"
                } else {
                    "loaded"
                }
            } else if probes.real {
                "installed:not-loaded"
            } else {
                "installed"
            };
            job.insert("enabled".into(), json!(enabled));
            job.insert("status".into(), json!(status));
        }
        if job
            .get("files")
            .is_some_and(|f| f.as_array().is_some_and(|a| a.is_empty()))
        {
            job.shift_remove("files");
        }
    }

    let mut loops: Vec<Doc> = jobs.into_values().map(Value::Object).collect();
    loops.sort_by(|a, b| {
        locale_cmp(
            a["id"].as_str().unwrap_or(""),
            b["id"].as_str().unwrap_or(""),
        )
    });
    json!({ "loops": loops, "_sources": [DIR] })
}

impl Importer for Launchd {
    fn id(&self) -> &str {
        "launchd"
    }
    fn layers(&self) -> Vec<&str> {
        vec!["loops"]
    }
    fn description(&self) -> &str {
        "launchd user agents incl. disabled/parked/removed ones (read-only)."
    }
    fn detect(&self, ctx: &Ctx) -> bool {
        ctx.platform() == "darwin" && ctx.is_dir(DIR)
    }
    fn run(&self, ctx: &Ctx, opts: &Doc) -> Result<Doc> {
        let probes = if ctx.is_real_home() {
            Probes::gather(ctx)
        } else {
            Probes::default()
        };
        Ok(scan(ctx, opts, &probes))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::{Path, PathBuf};

    struct Home {
        _tmp: tempfile::TempDir,
        ctx: Ctx,
        la: PathBuf,
        /// The sandbox home as an absolute string, for writing fixture contents.
        h: String,
    }

    fn home() -> Home {
        let tmp = tempfile::tempdir().unwrap();
        let la = tmp.path().join("Library/LaunchAgents");
        std::fs::create_dir_all(la.join("disabled")).unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        let h = ctx.home().to_string_lossy().into_owned();
        Home {
            _tmp: tmp,
            ctx,
            la,
            h,
        }
    }

    fn write_plist(path: &Path, body: &str) {
        std::fs::write(
            path,
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                 <plist version=\"1.0\">\n<dict>\n{body}\n</dict>\n</plist>\n"
            ),
        )
        .unwrap();
    }

    fn labelled(label: &str) -> String {
        format!(
            "<key>Label</key><string>{label}</string>\n\
             <key>ProgramArguments</key><array><string>/usr/bin/true</string></array>"
        )
    }

    fn loops(ctx: &Ctx, opts: Doc) -> Vec<Doc> {
        Launchd.run(ctx, &opts).unwrap()["loops"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn by_id<'a>(loops: &'a [Doc], id: &str) -> &'a Doc {
        loops
            .iter()
            .find(|l| l["id"] == id)
            .unwrap_or_else(|| panic!("no loop {id} in {loops:?}"))
    }

    #[test]
    fn calendar_job_with_working_dir_and_log() {
        let t = home();
        let h = &t.h;
        write_plist(
            &t.la.join("com.example.job.plist"),
            &format!(
                "<key>Label</key><string>com.example.job</string>\n\
                 <key>ProgramArguments</key><array><string>{h}/bin/job.sh</string><string>--quiet</string></array>\n\
                 <key>RunAtLoad</key><true/>\n\
                 <key>StandardOutPath</key><string>{h}/logs/job.log</string>\n\
                 <key>StandardErrorPath</key><string>{h}/logs/job.err</string>\n\
                 <key>StartCalendarInterval</key><array>\n\
                 <dict><key>Hour</key><integer>9</integer><key>Minute</key><integer>0</integer></dict>\n\
                 <dict><key>Hour</key><integer>17</integer><key>Minute</key><integer>0</integer></dict>\n\
                 </array>\n\
                 <key>WorkingDirectory</key><string>{h}/work</string>"
            ),
        );
        let out = Launchd.run(&t.ctx, &json!({})).unwrap();
        assert_eq!(out["_sources"], json!(["~/Library/LaunchAgents"]));
        let job = &out["loops"][0];
        assert_eq!(
            job,
            &json!({
                "id": "com.example.job",
                "runner": "launchd",
                "enabled": true,
                "status": "installed",
                "managed": true,
                "schedule": { "calendar": ["hour=9,minute=0", "hour=17,minute=0"], "run_at_load": true },
                "program": ["~/bin/job.sh", "--quiet"],
                "working_dir": "~/work",
                "log": "~/logs/job.log",
                "env_names": [],
                "files": ["~/Library/LaunchAgents/com.example.job.plist"]
            })
        );
        let keys: Vec<&str> = job
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "id",
                "runner",
                "enabled",
                "status",
                "managed",
                "schedule",
                "program",
                "working_dir",
                "log",
                "env_names",
                "files"
            ]
        );
    }

    #[test]
    fn interval_keep_alive_watch_paths_and_env_names() {
        let t = home();
        let h = &t.h;
        write_plist(
            &t.la.join("com.example.poll.plist"),
            &format!(
                "<key>Label</key><string>com.example.poll</string>\n\
                 <key>Program</key><string>{h}/bin/poll</string>\n\
                 <key>StartInterval</key><integer>300</integer>\n\
                 <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>\n\
                 <key>RunAtLoad</key><false/>\n\
                 <key>WatchPaths</key><array><string>{h}/inbox</string></array>\n\
                 <key>EnvironmentVariables</key><dict>\n\
                 <key>ZED_MODE</key><string>value-never-copied</string>\n\
                 <key>API_HOST</key><string>value-never-copied</string>\n\
                 </dict>"
            ),
        );
        let all = loops(&t.ctx, json!({}));
        let job = by_id(&all, "com.example.poll");
        assert_eq!(
            job["schedule"],
            json!({ "every_seconds": 300, "keep_alive": true, "watch_paths": ["~/inbox"] })
        );
        assert_eq!(job["program"], json!(["~/bin/poll"]));
        assert_eq!(job["env_names"], json!(["API_HOST", "ZED_MODE"]));
        assert!(job.get("working_dir").is_none() && job.get("log").is_none());
        assert!(!serde_json::to_string(&all)
            .unwrap()
            .contains("value-never-copied"));
    }

    #[test]
    fn parked_backup_and_disabled_folder_files() {
        let t = home();
        write_plist(
            &t.la.join("com.example.old.plist.killed-20260913"),
            &labelled("com.example.old"),
        );
        write_plist(
            &t.la.join("disabled/com.example.y.plist"),
            &labelled("com.example.y"),
        );
        write_plist(
            &t.la.join("com.example.z.plist.bak"),
            &labelled("com.example.z"),
        );
        write_plist(
            &t.la.join("com.example.r.plist.20260101"),
            &labelled("com.example.r"),
        );
        write_plist(
            &t.la.join("com.example.d.plist.disabled"),
            &labelled("com.example.d"),
        );
        write_plist(
            &t.la.join("disabled/com.example.q.plist.off"),
            &labelled("com.example.q"),
        );
        let all = loops(&t.ctx, json!({}));
        let status = |id: &str| by_id(&all, id)["status"].clone();
        assert_eq!(status("com.example.old"), json!("parked:killed"));
        assert_eq!(status("com.example.y"), json!("parked"));
        assert_eq!(status("com.example.z"), json!("backup-only"));
        assert_eq!(status("com.example.r"), json!("parked:renamed"));
        assert_eq!(status("com.example.d"), json!("parked:disabled"));
        assert_eq!(status("com.example.q"), json!("parked:off"));
        assert!(all.iter().all(|l| l["enabled"] == json!(false)));
        assert_eq!(
            by_id(&all, "com.example.y")["files"],
            json!(["~/Library/LaunchAgents/disabled/com.example.y.plist"])
        );
    }

    #[test]
    fn files_group_by_label_and_the_best_file_wins() {
        let t = home();
        write_plist(
            &t.la.join("com.example.job.plist.bak"),
            &labelled("com.example.job"),
        );
        write_plist(
            &t.la.join("disabled/com.example.job.plist"),
            &labelled("com.example.job"),
        );
        write_plist(
            &t.la.join("com.example.job.plist"),
            &labelled("com.example.job"),
        );
        let all = loops(&t.ctx, json!({}));
        assert_eq!(all.len(), 1);
        assert_eq!(all[0]["status"], json!("installed"));
        assert_eq!(
            all[0]["files"],
            json!([
                "~/Library/LaunchAgents/com.example.job.plist",
                "~/Library/LaunchAgents/disabled/com.example.job.plist",
                "~/Library/LaunchAgents/com.example.job.plist.bak"
            ])
        );
    }

    #[test]
    fn missing_label_or_unreadable_plist_falls_back_to_the_file_name() {
        let t = home();
        write_plist(
            &t.la.join("com.example.nolabel.plist"),
            "<key>ProgramArguments</key><array><string>/usr/bin/true</string><string>7</string></array>",
        );
        std::fs::write(
            t.la.join("com.example.broken.plist.killed-20260913"),
            "not a plist",
        )
        .unwrap();
        std::fs::write(t.la.join("README.txt"), "ignored").unwrap();
        let all = loops(&t.ctx, json!({}));
        assert_eq!(all.len(), 2);
        let nolabel = by_id(&all, "com.example.nolabel");
        assert_eq!(nolabel["status"], json!("installed"));
        assert_eq!(nolabel["program"], json!(["/usr/bin/true", "7"]));
        let broken = by_id(&all, "com.example.broken");
        assert_eq!(broken["status"], json!("parked:killed"));
        assert_eq!(broken["schedule"], json!({}));
        assert_eq!(broken["program"], json!([]));
    }

    #[test]
    fn binary_plists_parse() {
        let t = home();
        let mut d = plist::Dictionary::new();
        d.insert(
            "Label".into(),
            plist::Value::String("com.example.bin".into()),
        );
        d.insert(
            "ProgramArguments".into(),
            plist::Value::Array(vec![
                plist::Value::String(format!("{}/bin/tool", t.h)),
                plist::Value::String("run".into()),
            ]),
        );
        d.insert("StartInterval".into(), plist::Value::Integer(600i64.into()));
        plist::Value::Dictionary(d)
            .to_file_binary(t.la.join("com.example.bin.plist"))
            .unwrap();
        let all = loops(&t.ctx, json!({}));
        let job = by_id(&all, "com.example.bin");
        assert_eq!(job["program"], json!(["~/bin/tool", "run"]));
        assert_eq!(job["schedule"], json!({ "every_seconds": 600 }));
    }

    #[test]
    fn sandbox_home_installed_loops_are_enabled() {
        let t = home();
        write_plist(
            &t.la.join("com.example.a.plist"),
            &labelled("com.example.a"),
        );
        write_plist(
            &t.la.join("com.example.b.plist"),
            &labelled("com.example.b"),
        );
        let all = loops(&t.ctx, json!({}));
        assert_eq!(all.len(), 2);
        for l in &all {
            assert_eq!(l["enabled"], json!(true));
            assert_eq!(l["status"], json!("installed"));
        }
    }

    #[test]
    fn managed_prefixes_separate_vendor_jobs() {
        let t = home();
        write_plist(
            &t.la.join("com.example.job.plist"),
            &labelled("com.example.job"),
        );
        write_plist(
            &t.la.join("org.vendor.updater.plist"),
            &labelled("org.vendor.updater"),
        );
        let all = loops(&t.ctx, json!({ "managed_prefixes": ["com.example."] }));
        assert_eq!(by_id(&all, "com.example.job")["managed"], json!(true));
        assert_eq!(by_id(&all, "org.vendor.updater")["managed"], json!(false));
        let all = loops(&t.ctx, json!({}));
        assert!(all.iter().all(|l| l["managed"] == json!(true)));
    }

    #[test]
    fn live_launchctl_state_sets_status_on_the_real_home() {
        let t = home();
        for id in [
            "com.example.run",
            "com.example.idle",
            "com.example.off",
            "com.example.cold",
        ] {
            write_plist(&t.la.join(format!("{id}.plist")), &labelled(id));
        }
        let probes = Probes {
            real: true,
            loaded: parse_launchctl_list(
                "PID\tStatus\tLabel\n123\t0\tcom.example.run\n-\t78\tcom.example.idle\n",
            ),
            disabled: parse_print_disabled(
                "disabled services = {\n\
                 \t\"com.example.off\" => disabled\n\
                 \t\"com.example.gone\" => true\n\
                 \t\"com.example.back\" => enabled\n\
                 \t\"org.vendor.gone\" => disabled\n\
                 }\n",
            ),
        };
        let out = scan(
            &t.ctx,
            &json!({ "managed_prefixes": ["com.example."] }),
            &probes,
        );
        let all = out["loops"].as_array().unwrap().clone();
        let st = |id: &str| {
            let l = by_id(&all, id);
            (l["status"].clone(), l["enabled"].clone())
        };
        assert_eq!(st("com.example.run"), (json!("running"), json!(true)));
        assert_eq!(st("com.example.idle"), (json!("loaded"), json!(true)));
        assert_eq!(
            st("com.example.off"),
            (json!("installed:override-disabled"), json!(false))
        );
        assert_eq!(
            st("com.example.cold"),
            (json!("installed:not-loaded"), json!(false))
        );
        assert_eq!(
            by_id(&all, "com.example.gone"),
            &json!({
                "id": "com.example.gone", "runner": "launchd", "enabled": false,
                "status": "removed:override-disabled", "managed": true,
                "schedule": {}, "program": [], "env_names": []
            })
        );
        assert_eq!(
            st("com.example.back"),
            (json!("removed:override-enabled"), json!(false))
        );
        assert!(all.iter().all(|l| l["id"] != "org.vendor.gone"));
        let ids: Vec<&str> = all.iter().map(|l| l["id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            [
                "com.example.back",
                "com.example.cold",
                "com.example.gone",
                "com.example.idle",
                "com.example.off",
                "com.example.run"
            ]
        );
    }

    #[test]
    fn launchctl_outputs_parse() {
        let l = parse_launchctl_list(
            "PID\tStatus\tLabel\n  42\t0\tcom.example.a\n-\t0\tcom.example.b\n0\t1\tcom.example.c\nbroken-line\n\n",
        );
        assert_eq!(l.get("com.example.a"), Some(&Some(42)));
        assert_eq!(l.get("com.example.b"), Some(&None));
        assert_eq!(l.get("com.example.c"), Some(&Some(0)));
        assert_eq!(l.len(), 3);
        let d = parse_print_disabled(
            "disabled services = {\n\t\"a.one\" => false\n\t\"a.two\" => true\n\t\"a.one\" => disabled\n}\n\
             login item associations = {\n\t\"a.three\" => \"x\"\n}\n",
        );
        assert_eq!(
            d.into_iter().collect::<Vec<_>>(),
            vec![("a.one".to_string(), true), ("a.two".to_string(), true)]
        );
    }

    #[test]
    fn ids_sort_like_locale_compare() {
        let t = home();
        for (file, label) in [
            ("1.plist", "com.example.a"),
            ("2.plist", "com.example-a"),
            ("3.plist", "com.example_a"),
            ("4.plist", "com.example.B"),
        ] {
            write_plist(&t.la.join(file), &labelled(label));
        }
        let ids: Vec<String> = loops(&t.ctx, json!({}))
            .iter()
            .map(|l| l["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            ids,
            [
                "com.example_a",
                "com.example-a",
                "com.example.a",
                "com.example.B"
            ]
        );
        // The order Node's `localeCompare` (ICU, en) gives for the same strings.
        let mut v = vec![
            "com.example9",
            "aA",
            "a.b",
            "Com.example.a",
            "a1",
            "com.example.B",
            "a_b",
            "AA",
            "com.example.ab",
            "a b",
            "com.example.a.b",
            "a-",
            "ab",
            "com.Example.a",
            "a-b",
            "Aa",
            "com.example10",
            "A",
            "com.example.b",
            "aa",
            "com.example-a",
            "a",
            "com.example.A",
            "com.example_a",
            "com.example.a",
        ];
        v.sort_by(|a, b| locale_cmp(a, b));
        assert_eq!(
            v,
            [
                "a",
                "A",
                "a b",
                "a_b",
                "a-",
                "a-b",
                "a.b",
                "a1",
                "aa",
                "aA",
                "Aa",
                "AA",
                "ab",
                "com.example_a",
                "com.example-a",
                "com.example.a",
                "com.example.A",
                "com.Example.a",
                "Com.example.a",
                "com.example.a.b",
                "com.example.ab",
                "com.example.b",
                "com.example.B",
                "com.example10",
                "com.example9"
            ]
        );
    }

    #[test]
    fn plist_values_convert_to_documents() {
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
            <key>When</key><date>2026-09-13T08:30:00Z</date>\
            <key>Blob</key><data>AAEC</data>\
            <key>Whole</key><real>60.0</real>\
            <key>Half</key><real>0.5</real>\
            <key>Neg</key><integer>-3</integer>\
            <key>On</key><true/>\
            <key>List</key><array><string>x</string><dict><key>K</key><integer>1</integer></dict></array>\
            </dict></plist>";
        let v = plist::Value::from_reader_xml(xml.as_bytes()).unwrap();
        let d = plist_to_doc(&v);
        assert_eq!(
            d,
            json!({
                "When": "2026-09-13T08:30:00Z",
                "Blob": "<data>",
                "Whole": 60,
                "Half": 0.5,
                "Neg": -3,
                "On": true,
                "List": ["x", { "K": 1 }]
            })
        );
        assert_eq!(
            plist_to_doc(&plist::Value::Uid(plist::Uid::new(7))),
            json!(7)
        );
    }

    #[test]
    fn secret_looking_arguments_are_redacted() {
        let t = home();
        let fake = format!("{}={}", "token", "a".repeat(30));
        write_plist(
            &t.la.join("com.example.sec.plist"),
            &format!(
                "<key>Label</key><string>com.example.sec</string>\n\
                 <key>ProgramArguments</key><array><string>/usr/bin/env</string><string>{fake}</string></array>"
            ),
        );
        let all = loops(&t.ctx, json!({}));
        assert_eq!(
            by_id(&all, "com.example.sec")["program"],
            json!(["/usr/bin/env", "<redacted>"])
        );
        assert_eq!(t.ctx.warnings().len(), 1);
    }

    #[test]
    fn detect_needs_macos_and_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let ctx = Ctx::new(Some(tmp.path()), json!({}));
        assert!(!Launchd.detect(&ctx));
        assert_eq!(Launchd.run(&ctx, &json!({})).unwrap()["loops"], json!([]));
        std::fs::create_dir_all(tmp.path().join("Library/LaunchAgents")).unwrap();
        assert_eq!(Launchd.detect(&ctx), cfg!(target_os = "macos"));
    }
}
