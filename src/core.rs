//! Instance resolution, desired state, and the import orchestration that turns importer
//! fragments plus `overrides.yaml` and `policies.yaml` into one live document.

use crate::ctx::Ctx;
use crate::importers::load_importers;
use crate::layers::LAYERS;
use crate::util::{deep_merge, get_path, resolve_vars, set_path, str_list};
use crate::yaml::read_yaml;
use crate::Doc;
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Instance {
    pub dir: PathBuf,
    pub file: PathBuf,
}

fn absolute(cwd: &Path, p: &str) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

/// Find the instance folder: `--instance`, then `$SAC_INSTANCE`, then `./setup.yaml`, then the
/// only directory under `./instance/`.
pub fn resolve_instance(flag: Option<&str>, cwd: &Path) -> Result<Instance> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(f) = flag {
        cands.push(absolute(cwd, f));
    } else if let Some(e) = std::env::var("SAC_INSTANCE").ok().filter(|e| !e.is_empty()) {
        cands.push(absolute(cwd, &e));
    } else {
        if cwd.join("setup.yaml").exists() {
            cands.push(cwd.to_path_buf());
        }
        let idir = cwd.join("instance");
        if idir.is_dir() {
            let mut subs: Vec<PathBuf> = std::fs::read_dir(&idir)
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| p.is_dir())
                        .collect()
                })
                .unwrap_or_default();
            subs.sort();
            if subs.len() == 1 {
                cands.push(subs.remove(0));
            }
        }
    }
    let Some(dir) = cands.into_iter().next() else {
        bail!("no instance found: pass --instance <dir> (a folder holding setup.yaml), or run `init <dir>` first");
    };
    if dir
        .extension()
        .map(|e| e == "yaml" || e == "yml")
        .unwrap_or(false)
        && dir.is_file()
    {
        let parent = dir
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| cwd.to_path_buf());
        return Ok(Instance {
            dir: parent,
            file: dir,
        });
    }
    Ok(Instance {
        file: dir.join("setup.yaml"),
        dir,
    })
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub setup: Doc,
    pub overrides: Doc,
    pub policies: Doc,
}

fn optional_yaml(p: &Path) -> Result<Doc> {
    if p.exists() {
        read_yaml(p)
    } else {
        Ok(json!({}))
    }
}

/// Read `setup.yaml` (required), `overrides.yaml` and `policies.yaml` (optional).
pub fn load_instance(inst: &Instance) -> Result<Loaded> {
    if !inst.file.exists() {
        bail!("{} not found (run `import` or `init`)", inst.file.display());
    }
    Ok(Loaded {
        setup: read_yaml(&inst.file)?,
        overrides: optional_yaml(&inst.dir.join("overrides.yaml"))?,
        policies: optional_yaml(&inst.dir.join("policies.yaml"))?,
    })
}

/// `setup.yaml` with `policies.yaml` merged in, variables resolved and defaults applied.
pub fn desired_state(loaded: &Loaded) -> Doc {
    let pol = loaded
        .policies
        .get("safety")
        .map(|s| json!({ "safety": s }));
    let merged = deep_merge(Some(&loaded.setup), pol.as_ref());
    let vars = merged.get("vars").cloned().unwrap_or(json!({}));
    normalize(redact_secret_values(resolve_vars(&merged, &vars)))
}

/// Replace any secret-shaped string in a document with `<redacted>`. The desired state is
/// rendered into out/ and merged into live config, so a value that looks like a credential
/// must never travel with it.
fn redact_secret_values(v: Doc) -> Doc {
    match v {
        Value::String(s) => {
            if crate::secrets::find_secrets(&s).is_empty() {
                Value::String(s)
            } else {
                Value::String("<redacted>".to_string())
            }
        }
        Value::Array(a) => Value::Array(a.into_iter().map(redact_secret_values).collect()),
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, x)| (k, redact_secret_values(x)))
                .collect(),
        ),
        other => other,
    }
}

/// Collection and scalar paths a fragment actually returned (even when empty).
fn observed_paths(frag: &Doc) -> HashSet<String> {
    let mut seen = HashSet::new();
    for layer in LAYERS.iter() {
        for c in layer.collections {
            if get_path(frag, c.path)
                .map(|v| v.is_array())
                .unwrap_or(false)
            {
                seen.insert(c.path.to_string());
            }
        }
        for s in layer.scalars {
            if get_path(frag, s).is_some() {
                seen.insert((*s).to_string());
            }
        }
    }
    seen
}

fn strip_import_only(o: &Doc) -> Doc {
    match o {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(k, _)| !matches!(k.as_str(), "import" | "meta" | "version"))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        _ => json!({}),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ImportRun {
    pub id: String,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ImportResult {
    pub live: Doc,
    pub ran: Vec<ImportRun>,
    pub warnings: Vec<String>,
    pub observed: Vec<String>,
}

pub struct ImportArgs<'a> {
    pub inst: &'a Instance,
    pub setup: &'a Doc,
    pub overrides: &'a Doc,
    pub policies: &'a Doc,
    pub home: Option<&'a Path>,
    /// Collections no importer observed are carried over from here (usually the desired state).
    pub baseline: &'a Doc,
}

/// Run the importers and assemble the live document.
pub fn import_live(a: ImportArgs) -> Result<ImportResult> {
    // overrides.yaml wins over setup.yaml for the import block, so a freshly scaffolded
    // instance can set managed_prefixes and repo_roots in overrides before the first import.
    let cfg = match (a.setup.get("import"), a.overrides.get("import")) {
        (None, None) => json!({}),
        (sv, ov) => deep_merge(sv, ov),
    };
    let options = cfg.get("options").cloned().unwrap_or(json!({}));
    let ctx = Ctx::new(a.home, options.clone());
    let importers = load_importers(Some(&a.inst.dir))?;
    let wanted: Option<Vec<String>> = match cfg.get("importers") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s == "all" => None,
        Some(v) => Some(str_list(Some(v))),
    };

    let mut live = json!({});
    let mut ran = Vec::new();
    let mut observed: HashSet<String> = HashSet::new();
    for imp in &importers {
        let id = imp.id().to_string();
        if let Some(w) = &wanted {
            if !w.contains(&id) {
                continue;
            }
        }
        let mut opts = options.get(&id).cloned().unwrap_or(json!({}));
        if !opts.is_object() {
            opts = json!({});
        }
        if id == "file-state" {
            let declared = get_path(a.overrides, "data.state")
                .filter(|v| v.is_array())
                .or_else(|| get_path(a.setup, "data.state").filter(|v| v.is_array()))
                .cloned()
                .unwrap_or(json!([]));
            opts["declared"] = declared;
        }
        let detected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| imp.detect(&ctx)))
            .unwrap_or(false);
        if !detected {
            ran.push(ImportRun {
                id,
                status: "not-detected".into(),
                error: None,
            });
            continue;
        }
        match imp.run(&ctx, &opts) {
            Ok(mut frag) => {
                if let Some(o) = frag.as_object_mut() {
                    o.remove("_sources");
                }
                observed.extend(observed_paths(&frag));
                live = deep_merge(Some(&live), Some(&frag));
                ran.push(ImportRun {
                    id,
                    status: "ok".into(),
                    error: None,
                });
            }
            Err(e) => {
                ctx.warn(format!("{id}: {e}"));
                ran.push(ImportRun {
                    id,
                    status: "error".into(),
                    error: Some(e.to_string()),
                });
            }
        }
    }

    for layer in LAYERS.iter() {
        let paths = layer
            .collections
            .iter()
            .map(|c| c.path)
            .chain(layer.scalars.iter().copied());
        for p in paths {
            if observed.contains(p) {
                continue;
            }
            if let Some(v) = get_path(a.baseline, p) {
                set_path(&mut live, p, v.clone());
            }
        }
    }
    // Hand-declared objects under these four layers survive an import: a scalar object no
    // importer observed (identity.person, brain.budgets, ...) is deep-merged under what the
    // importers found, so `profile_refs` from memory never wipes `role` or `timezone`.
    for k in ["identity", "brain", "data", "infra"] {
        if let Some(Value::Object(o)) = a.baseline.get(k) {
            for (kk, v) in o {
                let path = format!("{k}.{kk}");
                if v.is_array() || observed.contains(path.as_str()) {
                    continue;
                }
                match (v, get_path(&live, &path)) {
                    (Value::Object(_), Some(found @ Value::Object(_))) => {
                        let merged = deep_merge(Some(v), Some(found));
                        set_path(&mut live, &path, merged);
                    }
                    (_, None) => set_path(&mut live, &path, v.clone()),
                    _ => {}
                }
            }
        }
    }

    let mut merged = deep_merge(Some(&live), Some(&strip_import_only(a.overrides)));
    if let Some(safety) = a.policies.get("safety") {
        merged = deep_merge(Some(&merged), Some(&json!({ "safety": safety })));
    }

    let mut meta = Map::new();
    let instance_name = a
        .setup
        .get("meta")
        .and_then(|m| m.get("instance"))
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            a.inst
                .dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "instance".to_string())
        });
    meta.insert("instance".to_string(), Value::String(instance_name));
    if let Some(Value::Object(m)) = a.setup.get("meta") {
        for (k, v) in m {
            meta.insert(k.clone(), v.clone());
        }
    }
    let mut head = Map::new();
    head.insert("version".to_string(), json!(1));
    head.insert("meta".to_string(), Value::Object(meta));
    if let Some(vars) = a.setup.get("vars") {
        head.insert("vars".to_string(), vars.clone());
    }
    if cfg.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
        head.insert("import".to_string(), cfg.clone());
    }
    let mut out = head.clone();
    if let Value::Object(m) = merged {
        for (k, v) in m {
            if !head.contains_key(&k) {
                out.insert(k, v);
            }
        }
    }
    let mut observed: Vec<String> = observed.into_iter().collect();
    observed.sort();
    Ok(ImportResult {
        live: normalize(Value::Object(out)),
        ran,
        warnings: ctx.warnings(),
        observed,
    })
}

/// Apply defaults: loops get `runner: manual, enabled: false` (and `status: declared-only`
/// when hand-declared without a runner); a channel kind written as one object becomes a
/// one-item list; rules get `scope: global, severity: standing, date: null, reason: "",
/// status: active`.
pub fn normalize(doc: Doc) -> Doc {
    let Value::Object(mut s) = doc else {
        return doc;
    };
    if let Some(Value::Array(loops)) = s.get("loops").cloned() {
        let out: Vec<Doc> = loops
            .into_iter()
            .map(|l| {
                let Value::Object(l) = l else { return l };
                let mut m = Map::new();
                m.insert("runner".into(), json!("manual"));
                m.insert("enabled".into(), json!(false));
                for (k, v) in &l {
                    m.insert(k.clone(), v.clone());
                }
                if !l.contains_key("runner") {
                    let status = l
                        .get("status")
                        .filter(|v| !v.is_null())
                        .cloned()
                        .unwrap_or(json!("declared-only"));
                    m.insert("status".into(), status);
                }
                Value::Object(m)
            })
            .collect();
        s.insert("loops".into(), Value::Array(out));
    }
    if let Some(Value::Object(ch)) = s.get("channels").cloned() {
        let mut out = Map::new();
        for (k, v) in ch {
            match v {
                Value::Object(o) => {
                    let mut item = Map::new();
                    item.insert("id".into(), Value::String(k.clone()));
                    for (kk, vv) in o {
                        item.insert(kk, vv);
                    }
                    out.insert(k, Value::Array(vec![Value::Object(item)]));
                }
                other => {
                    out.insert(k, other);
                }
            }
        }
        s.insert("channels".into(), Value::Object(out));
    }
    if let Some(Value::Object(mut ins)) = s.get("instructions").cloned() {
        if let Some(Value::Array(rules)) = ins.get("rules").cloned() {
            let out: Vec<Doc> = rules
                .into_iter()
                .map(|r| {
                    let Value::Object(r) = r else { return r };
                    let mut m = Map::new();
                    m.insert("scope".into(), json!("global"));
                    m.insert("severity".into(), json!("standing"));
                    m.insert("date".into(), Value::Null);
                    m.insert("reason".into(), json!(""));
                    m.insert("status".into(), json!("active"));
                    for (k, v) in r {
                        m.insert(k, v);
                    }
                    Value::Object(m)
                })
                .collect();
            ins.insert("rules".into(), Value::Array(out));
            s.insert("instructions".into(), Value::Object(ins));
        }
    }
    Value::Object(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_applies_defaults() {
        let d = json!({
            "loops": [{ "id": "a" }, { "id": "b", "runner": "cron", "enabled": true }],
            "channels": { "todo": { "path": "~/t.md" }, "slack": [{ "id": "x" }] },
            "instructions": { "rules": [{ "id": "r", "severity": "hard" }] }
        });
        let n = normalize(d);
        assert_eq!(n["loops"][0]["runner"], json!("manual"));
        assert_eq!(n["loops"][0]["status"], json!("declared-only"));
        assert_eq!(n["loops"][1]["status"], Value::Null);
        assert_eq!(n["channels"]["todo"][0]["id"], json!("todo"));
        assert_eq!(n["instructions"]["rules"][0]["scope"], json!("global"));
        assert_eq!(n["instructions"]["rules"][0]["severity"], json!("hard"));
        assert_eq!(n["instructions"]["rules"][0]["date"], Value::Null);
    }

    #[test]
    fn resolve_instance_prefers_flag_then_single_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        std::fs::create_dir_all(cwd.join("instance/me")).unwrap();
        let r = resolve_instance(None, cwd).unwrap();
        assert_eq!(r.dir, cwd.join("instance/me"));
        assert_eq!(r.file, cwd.join("instance/me/setup.yaml"));
        let f = resolve_instance(Some("other"), cwd).unwrap();
        assert_eq!(f.dir, cwd.join("other"));
        std::fs::write(cwd.join("direct.yaml"), "version: 1\n").unwrap();
        let y = resolve_instance(Some("direct.yaml"), cwd).unwrap();
        assert_eq!(y.file, cwd.join("direct.yaml"));
        assert_eq!(y.dir, cwd.to_path_buf());
    }

    #[test]
    fn desired_state_merges_policies_and_resolves_vars() {
        let loaded = Loaded {
            setup: json!({ "version": 1, "meta": { "instance": "x" }, "vars": { "root": "/r" }, "projects": [{ "id": "p", "path": "${vars.root}/p" }] }),
            overrides: json!({}),
            policies: json!({ "safety": { "policies": [{ "id": "a", "check": "c", "severity": "hard" }] } }),
        };
        let d = desired_state(&loaded);
        assert_eq!(d["projects"][0]["path"], json!("/r/p"));
        assert_eq!(d["safety"]["policies"][0]["id"], json!("a"));
        let tok = ["xoxb-", "1234567890-abcdefghij"].concat();
        let leaky = Loaded {
            setup: json!({ "version": 1, "meta": { "instance": "x" }, "identity": { "accounts": [{ "id": "a", "kind": "k", "access": "read_only", "note": tok }] } }),
            overrides: json!({}),
            policies: json!({}),
        };
        assert_eq!(
            desired_state(&leaky)["identity"]["accounts"][0]["note"],
            json!("<redacted>")
        );
    }
}
