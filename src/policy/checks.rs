//! The nine built-in policy checks, ported from the reference `framework/policy/checks.mjs`.
//!
//! Every check is pure: it reads the setup document and its params and never touches the
//! machine. Ids, docs, messages, evidence strings and the pass/warn/fail rules match the
//! reference word for word, including the way JavaScript renders an absent value
//! (`undefined`) inside a message. One deliberate difference: a list param written as a single
//! scalar (`servers: work`) counts as a one-item list, where the reference would iterate the
//! string's characters.

use super::{Check, CheckCtx, CheckResult};
use crate::secrets::find_secrets;
use crate::util::{arr, expand_home, get_path, glob_to_regex, is_truthy, str_list, tilde};
use crate::Doc;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::OnceLock;

type RunFn = fn(&Doc, &Doc, &CheckCtx) -> CheckResult;

/// Id, doc and implementation of every built-in check, in the reference's order.
const BUILTINS: [(&str, &str, RunFn); 9] = [
    (
        "deny_mcp",
        "Named MCP servers must be blocked (tools.denied_mcp_servers) and their tool patterns denied (tools.permissions.deny), never allowed, and never defined as an enabled server unless blocked.",
        deny_mcp,
    ),
    (
        "no_payment_tools",
        "No enabled tool, server, plugin, skill, permission or can_post account whose name looks payment/spend related, unless listed in allow.",
        no_payment_tools,
    ),
    (
        "forbidden_paths",
        "No hook, loop or MCP server command references a forbidden path, and the apply output dir is not inside one.",
        forbidden_paths,
    ),
    (
        "required_gate",
        "A named gate exists in brain.gates and covers every active channel of the given kinds (channel.gate, or gate.applies_to naming the kind, platform or channel id).",
        required_gate,
    ),
    (
        "channel_identity",
        "Every active channel of a kind posts only through allowed identities; no channel uses an account whose access is forbidden.",
        channel_identity,
    ),
    (
        "secrets_by_reference",
        "No secret-looking value anywhere in the setup; credentials appear only as references (file path, env var name, keychain item).",
        secrets_by_reference,
    ),
    (
        "loop_success_metric",
        "Every enabled (managed) loop declares a success_metric.",
        loop_success_metric,
    ),
    (
        "required_rules",
        "Listed rule ids exist in instructions.rules, are active, and are hard.",
        required_rules,
    ),
    (
        "forbidden_accounts_unused",
        "Accounts with access: forbidden are not referenced by any channel, loop or gate.",
        forbidden_accounts_unused,
    ),
];

struct Builtin {
    id: &'static str,
    doc: &'static str,
    run: RunFn,
}

impl Check for Builtin {
    fn id(&self) -> &str {
        self.id
    }
    fn doc(&self) -> &str {
        self.doc
    }
    fn run(&self, setup: &Doc, params: &Doc, ctx: &CheckCtx) -> CheckResult {
        (self.run)(setup, params, ctx)
    }
}

/// Every built-in check, in the reference's order.
pub fn all() -> Vec<Box<dyn Check>> {
    BUILTINS
        .iter()
        .map(|&(id, doc, run)| Box::new(Builtin { id, doc, run }) as Box<dyn Check>)
        .collect()
}

// ---- JavaScript value semantics that the reference's messages depend on

/// JavaScript `String(v)`: an absent value renders as `undefined`, null as `null`.
fn js(v: Option<&Doc>) -> String {
    match v {
        None => "undefined".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => match n.as_f64() {
            Some(f) if !n.is_i64() && !n.is_u64() && f.fract() == 0.0 && f.abs() < 1e21 => {
                format!("{f:.0}")
            }
            _ => n.to_string(),
        },
        Some(Value::String(x)) => x.clone(),
        Some(Value::Array(a)) => js_join(a, ","),
        Some(Value::Object(_)) => "[object Object]".to_string(),
    }
}

/// JavaScript `Array.prototype.join`: null items become empty strings.
fn js_join<'a>(items: impl IntoIterator<Item = &'a Doc>, sep: &str) -> String {
    items
        .into_iter()
        .map(|x| {
            if x.is_null() {
                String::new()
            } else {
                js(Some(x))
            }
        })
        .collect::<Vec<_>>()
        .join(sep)
}

/// JavaScript `String(v || '')`.
fn js_or_empty(v: Option<&Doc>) -> String {
    if is_truthy(v) {
        js(v)
    } else {
        String::new()
    }
}

/// JavaScript `v !== false`.
fn not_false(v: Option<&Doc>) -> bool {
    v != Some(&Value::Bool(false))
}

/// JavaScript `v === want` for a string.
fn is_str(v: Option<&Doc>, want: &str) -> bool {
    v.and_then(Value::as_str) == Some(want)
}

/// A list param. Absent or null is empty; a single scalar counts as a one-item list.
fn param_list<'a>(params: &'a Doc, key: &str) -> Vec<&'a Doc> {
    match params.get(key) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.iter().collect(),
        Some(other) => vec![other],
    }
}

fn entries(n: usize) -> &'static str {
    if n == 1 {
        "entry"
    } else {
        "entries"
    }
}

fn inactive_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            "(?i)halt|dead|killed|blocked|frozen|forbidden|paused|stopped|removed|superseded",
        )
        .unwrap()
    })
}

fn token_split_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new("[^a-z0-9]+").unwrap())
}

fn mcp_server_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new("^mcp__(.+?)__").unwrap())
}

/// A channel is active unless it says `active: false`, or its status, or the first `;`/`,`
/// separated part of its role, names a stopped state (halted, dead, killed, blocked, frozen,
/// forbidden, paused, stopped, removed, superseded).
pub fn is_active_channel(c: &Doc) -> bool {
    let role = js_or_empty(c.get("role"));
    let first_role = role.split([';', ',']).next().unwrap_or_default();
    not_false(c.get("active"))
        && !inactive_re().is_match(&js_or_empty(c.get("status")))
        && !inactive_re().is_match(first_role)
}

/// Is `pattern` covered by some entry of `list`: equal to it, or matched by an entry that is a
/// broader glob?
fn covered(pattern: &str, list: &[String]) -> bool {
    let probe = pattern.replace('*', "x");
    list.iter()
        .any(|e| e == pattern || glob_to_regex(e).is_match(&probe))
}

// ---- the checks

fn deny_mcp(setup: &Doc, params: &Doc, _ctx: &CheckCtx) -> CheckResult {
    let servers = param_list(params, "servers");
    let patterns: Vec<String> = param_list(params, "tool_patterns")
        .into_iter()
        .map(|p| js(Some(p)))
        .collect();
    let denied = arr(get_path(setup, "tools.denied_mcp_servers"));
    let deny = str_list(get_path(setup, "tools.permissions.deny"));
    let allow = str_list(get_path(setup, "tools.permissions.allow"));
    let defined = arr(get_path(setup, "tools.mcp_servers"));
    let (mut bad, mut notes) = (Vec::new(), Vec::new());
    for &server in &servers {
        let name = js(Some(server));
        let blocked = denied.contains(server);
        if !blocked {
            bad.push(format!("server \"{name}\" not in denied_mcp_servers"));
        }
        let suffixed = format!("{name}@");
        for d in defined.iter().filter(|d| {
            d.get("id") == Some(server)
                || d.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| id.starts_with(&suffixed))
        }) {
            let source = if is_truthy(d.get("source")) {
                js(d.get("source"))
            } else {
                "setup".to_string()
            };
            if !blocked && not_false(d.get("enabled")) {
                bad.push(format!("server \"{name}\" defined and enabled in {source}"));
            } else {
                notes.push(format!(
                    "server \"{name}\" still defined in {source} but blocked"
                ));
            }
        }
    }
    let mut dead = Vec::new();
    for p in &patterns {
        let is_denied = covered(p, &deny);
        if !is_denied {
            bad.push(format!("tool pattern {p} missing from permissions.deny"));
        }
        let narrow = glob_to_regex(p);
        let probe = p.replace('*', "x");
        let hits: Vec<&str> = allow
            .iter()
            .map(String::as_str)
            .filter(|a| {
                narrow.is_match(a) || (a.starts_with("mcp__") && glob_to_regex(a).is_match(&probe))
            })
            .collect();
        if hits.is_empty() {
            continue;
        }
        // Claude Code evaluates deny before allow, so an allow entry under a denied pattern is
        // dead.
        if is_denied {
            dead.extend(
                hits.iter()
                    .map(|a| format!("dead allow entry (deny wins): {a}")),
            );
        } else {
            bad.push(format!("tool pattern {p} allowed by: {}", hits.join(", ")));
        }
    }
    if !bad.is_empty() {
        let msg = format!("{} way(s) the denied MCP surface is reachable", bad.len());
        return CheckResult::fail(msg, [bad, notes, dead].concat());
    }
    let msg = format!(
        "{} server(s) blocked, {} tool pattern(s) denied",
        servers.len(),
        patterns.len()
    );
    if dead.is_empty() {
        CheckResult::pass(msg, notes)
    } else {
        let msg = format!(
            "{msg}; {} dead allow {} to clean up",
            dead.len(),
            entries(dead.len())
        );
        CheckResult::warn(msg, [dead, notes].concat())
    }
}

const PAYMENT_WORDS: [&str; 21] = [
    "stripe",
    "paypal",
    "checkout",
    "purchase",
    "payment",
    "payments",
    "billing",
    "braintree",
    "adyen",
    "square",
    "coinbase",
    "venmo",
    "revolut",
    "invoice",
    "google-ads",
    "meta-ads",
    "ads-manager",
    "adspend",
    "buy",
    "shop-pay",
    "top-up",
];

fn no_payment_tools(setup: &Doc, params: &Doc, _ctx: &CheckCtx) -> CheckResult {
    let mut words: Vec<String> = PAYMENT_WORDS.iter().map(|w| w.to_string()).collect();
    words.extend(
        param_list(params, "patterns")
            .into_iter()
            .map(|w| js(Some(w)).to_lowercase()),
    );
    let allowed = param_list(params, "allow");
    // The first word that names a payment surface: a hyphenated word anywhere in the name,
    // any other word as a whole token. As in the reference, a first match that is the empty
    // word (an empty extra pattern) counts as no match.
    let hit = |name: &str| -> Option<String> {
        let n = name.to_lowercase();
        let tokens: HashSet<&str> = token_split_re().split(&n).collect();
        words
            .iter()
            .find(|w| {
                if w.contains('-') {
                    n.contains(w.as_str())
                } else {
                    tokens.contains(w.as_str())
                }
            })
            .filter(|w| !w.is_empty())
            .cloned()
    };
    let denied_servers = arr(get_path(setup, "tools.denied_mcp_servers"));
    let deny = str_list(get_path(setup, "tools.permissions.deny"));
    let (mut found, mut dormant) = (Vec::new(), Vec::new());
    // `state` is None for a live surface, or the reason it is dormant.
    let mut consider = |kind: &str, id: Option<&Doc>, name: &str, state: Option<String>| {
        if id.is_some_and(|id| allowed.contains(&id)) {
            return;
        }
        let Some(word) = hit(name) else {
            return;
        };
        let id = js(id);
        let id = if id.chars().count() > 120 {
            format!("{}...", id.chars().take(117).collect::<String>())
        } else {
            id
        };
        match state {
            None => found.push(format!("{kind} {id} (matches \"{word}\")")),
            Some(why) => dormant.push(format!("{kind} {id} (matches \"{word}\"; {why})")),
        }
    };
    let disabled = |on: bool| (!on).then(|| "disabled".to_string());

    let servers = arr(get_path(setup, "tools.mcp_servers"));
    let live = |s: &Doc| {
        not_false(s.get("enabled")) && !s.get("id").is_some_and(|id| denied_servers.contains(id))
    };
    let live_servers: HashSet<String> = servers
        .iter()
        .filter(|s| live(s))
        .map(|s| {
            let id = js(s.get("id"));
            id.split('@').next().unwrap_or_default().to_string()
        })
        .collect();
    for s in servers {
        let name = format!(
            "{} {} {}",
            js(s.get("id")),
            js_or_empty(s.get("command")),
            js_or_empty(s.get("url"))
        );
        consider("mcp", s.get("id"), &name, disabled(live(s)));
    }
    for p in arr(get_path(setup, "tools.plugins")) {
        let on = not_false(p.get("enabled"));
        consider("plugin", p.get("id"), &js(p.get("id")), disabled(on));
    }
    for s in arr(get_path(setup, "tools.skills")) {
        consider("skill", s.get("id"), &js(s.get("id")), None);
    }
    for a in str_list(get_path(setup, "tools.permissions.allow")) {
        // mcp__<server>__<tool>: only live if that server is currently defined and enabled.
        let server = mcp_server_re().captures(&a).map(|c| c[1].to_string());
        let state = if covered(&a, &deny) {
            Some("denied")
        } else if server.is_some_and(|v| {
            !live_servers.contains(&v) && !live_servers.contains(&v.replace('_', "-"))
        }) {
            Some("server not defined")
        } else if a.starts_with("Bash(") {
            Some("exact shell command allow")
        } else {
            None
        };
        let id = Value::String(a.clone());
        consider("allow", Some(&id), &a, state.map(str::to_string));
    }
    for acc in arr(get_path(setup, "identity.accounts")) {
        let scopes = match acc.get("scopes") {
            Some(Value::Array(a)) => js_join(a, " "),
            _ => String::new(),
        };
        let name = format!("{} {scopes}", js(acc.get("id")));
        let state = match acc.get("access") {
            Some(Value::Bool(true)) => None,
            v if is_str(v, "can_post") => None,
            v => Some(js(v)),
        };
        consider("account", acc.get("id"), &name, state);
    }

    if !found.is_empty() {
        let msg = format!("{} live payment-capable surface(s)", found.len());
        let evidence = found
            .into_iter()
            .chain(dormant.into_iter().map(|d| format!("dormant: {d}")))
            .collect();
        return CheckResult::fail(msg, evidence);
    }
    if !dormant.is_empty() {
        let msg = format!(
            "no live payment-capable tools; {} dormant {} to review",
            dormant.len(),
            entries(dormant.len())
        );
        return CheckResult::warn(msg, dormant);
    }
    CheckResult::pass("no payment-capable tools", Vec::new())
}

fn forbidden_paths(setup: &Doc, params: &Doc, ctx: &CheckCtx) -> CheckResult {
    let home = ctx.home.as_path();
    let norm = |p: &str| {
        tilde(&expand_home(p, home), home)
            .trim_end_matches('/')
            .to_string()
    };
    let forbidden: Vec<String> = param_list(params, "paths")
        .into_iter()
        .map(|p| norm(&js(Some(p))))
        .collect();
    let mut bad = Vec::new();
    let mut scan = |place: &str, v: Option<&Doc>| {
        let v = norm(&js_or_empty(v));
        for f in &forbidden {
            if v == *f
                || v.contains(&format!("{f}/"))
                || v.contains(&format!("{f} "))
                || v.ends_with(f.as_str())
            {
                bad.push(format!("{place} -> {f}"));
            }
        }
    };
    for h in arr(get_path(setup, "tools.hooks")) {
        scan(&format!("hook {}", js(h.get("id"))), h.get("command"));
    }
    for l in arr(setup.get("loops")) {
        if is_truthy(l.get("enabled")) {
            let id = js(l.get("id"));
            for a in arr(l.get("program")) {
                scan(&format!("loop {id}"), Some(a));
            }
            scan(&format!("loop {id} cwd"), l.get("working_dir"));
        }
    }
    for m in arr(get_path(setup, "tools.mcp_servers")) {
        if not_false(m.get("enabled")) {
            let id = js(m.get("id"));
            scan(&format!("mcp {id}"), m.get("command"));
            for a in arr(m.get("args")) {
                scan(&format!("mcp {id}"), Some(a));
            }
        }
    }
    if let Some(out) = ctx.out_root.as_ref().filter(|o| !o.as_os_str().is_empty()) {
        let out = Value::String(out.to_string_lossy().into_owned());
        scan("apply output dir", Some(&out));
    }
    if bad.is_empty() {
        let msg = format!("{} forbidden path(s) unreferenced", forbidden.len());
        CheckResult::pass(msg, Vec::new())
    } else {
        let msg = format!("{} reference(s) to forbidden paths", bad.len());
        CheckResult::fail(msg, bad)
    }
}

fn required_gate(setup: &Doc, params: &Doc, _ctx: &CheckCtx) -> CheckResult {
    let gate = params.get("gate");
    let gate_name = js(gate);
    let Some(g) = arr(get_path(setup, "brain.gates"))
        .iter()
        .find(|x| x.get("id") == gate)
    else {
        let msg = format!("gate \"{gate_name}\" is not defined in brain.gates");
        return CheckResult::fail(msg, Vec::new());
    };
    let applies: HashSet<String> = arr(g.get("applies_to"))
        .iter()
        .map(|x| js(Some(x)).to_lowercase())
        .collect();
    let named = |v: Option<&Doc>| {
        v.and_then(Value::as_str)
            .is_some_and(|x| applies.contains(x))
    };
    let mut missing = Vec::new();
    let mut n = 0;
    for kind in param_list(params, "channel_kinds") {
        let key = js(Some(kind));
        for c in arr(setup.get("channels").and_then(|ch| ch.get(key.as_str()))) {
            if !is_active_channel(c) {
                continue;
            }
            n += 1;
            let platforms: Vec<String> = match c.get("platform") {
                Some(Value::Array(a)) => a.iter().map(|p| js(Some(p)).to_lowercase()).collect(),
                p if is_truthy(p) => vec![js(p).to_lowercase()],
                _ => Vec::new(),
            };
            let ok = c.get("gate") == gate
                || named(Some(kind))
                || named(c.get("id"))
                || platforms.iter().any(|p| applies.contains(p));
            if !ok {
                missing.push(format!("{key}/{}", js(c.get("id"))));
            }
        }
    }
    if missing.is_empty() {
        let msg = format!("gate {gate_name} covers {n} active channel(s)");
        CheckResult::pass(msg, Vec::new())
    } else {
        let msg = format!(
            "{} active channel(s) bypass gate {gate_name}",
            missing.len()
        );
        CheckResult::fail(msg, missing)
    }
}

fn channel_identity(setup: &Doc, params: &Doc, _ctx: &CheckCtx) -> CheckResult {
    let kind = js(params.get("kind"));
    let allowed = param_list(params, "allowed");
    let accounts = arr(get_path(setup, "identity.accounts"));
    let (mut bad, mut warns) = (Vec::new(), Vec::new());
    for c in arr(setup.get("channels").and_then(|ch| ch.get(kind.as_str()))) {
        let id = js(c.get("id"));
        let identity = match c.get("identity") {
            Some(v) if is_truthy(Some(v)) && v.as_str() != Some("none") => v,
            _ => {
                if is_active_channel(c) {
                    warns.push(format!("{id}: no identity declared"));
                }
                continue;
            }
        };
        let identity_name = js(Some(identity));
        // A later account with the same id wins, as in the reference's Map.
        let account = accounts
            .iter()
            .rev()
            .find(|a| a.get("id") == Some(identity));
        if account.is_some_and(|a| is_str(a.get("access"), "forbidden")) {
            bad.push(format!("{id} uses forbidden account {identity_name}"));
        } else if is_active_channel(c) && !allowed.is_empty() && !allowed.contains(&identity) {
            bad.push(format!(
                "{id} posts as {identity_name} (allowed: {})",
                js_join(allowed.iter().copied(), ", ")
            ));
        }
        if account.is_none() {
            warns.push(format!(
                "{id}: identity {identity_name} not declared in identity.accounts"
            ));
        }
    }
    if !bad.is_empty() {
        let msg = format!("{} {kind} channel(s) use a disallowed identity", bad.len());
        return CheckResult::fail(msg, [bad, warns].concat());
    }
    if warns.is_empty() {
        let msg = format!("all {kind} channels post through allowed identities");
        CheckResult::pass(msg, Vec::new())
    } else {
        let msg = format!("{kind} identities ok, {} note(s)", warns.len());
        CheckResult::warn(msg, warns)
    }
}

fn secrets_by_reference(setup: &Doc, _params: &Doc, _ctx: &CheckCtx) -> CheckResult {
    fn walk(v: &Doc, path: &str, hits: &mut Vec<String>) {
        match v {
            Value::String(text) => {
                hits.extend(
                    find_secrets(text)
                        .iter()
                        .map(|h| format!("{path}: {}", h.id)),
                );
            }
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    walk(x, &format!("{path}[{i}]"), hits);
                }
            }
            Value::Object(o) => {
                for (k, x) in o {
                    let p = if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}.{k}")
                    };
                    walk(x, &p, hits);
                }
            }
            _ => {}
        }
    }
    let mut hits = Vec::new();
    walk(setup, "", &mut hits);
    if hits.is_empty() {
        CheckResult::pass("no secret values; references only", Vec::new())
    } else {
        let msg = format!("{} secret-looking value(s) in setup", hits.len());
        CheckResult::fail(msg, hits)
    }
}

fn loop_success_metric(setup: &Doc, params: &Doc, _ctx: &CheckCtx) -> CheckResult {
    // Defaults to true only when absent; an explicit null counts as false.
    let only_managed = match params.get("only_managed") {
        None => true,
        v => is_truthy(v),
    };
    let missing: Vec<String> = arr(setup.get("loops"))
        .iter()
        .filter(|l| {
            is_truthy(l.get("enabled"))
                && (!only_managed || not_false(l.get("managed")))
                && !is_truthy(l.get("success_metric"))
        })
        .map(|l| js(l.get("id")))
        .collect();
    if missing.is_empty() {
        CheckResult::pass("every enabled loop has a success metric", Vec::new())
    } else {
        let msg = format!("{} enabled loop(s) without a success metric", missing.len());
        CheckResult::fail(msg, missing)
    }
}

fn required_rules(setup: &Doc, params: &Doc, _ctx: &CheckCtx) -> CheckResult {
    let rules = arr(get_path(setup, "instructions.rules"));
    let ids = param_list(params, "ids");
    let mut bad = Vec::new();
    for &id in &ids {
        let name = js(Some(id));
        // A later rule with the same id wins, as in the reference's Map.
        match rules.iter().rev().find(|r| r.get("id") == Some(id)) {
            None => bad.push(format!("{name}: missing")),
            Some(r) if is_str(r.get("status"), "superseded") => {
                bad.push(format!("{name}: superseded"));
            }
            Some(r) if !is_str(r.get("severity"), "hard") => bad.push(format!(
                "{name}: severity {}, expected hard",
                js(r.get("severity"))
            )),
            Some(_) => {}
        }
    }
    if bad.is_empty() {
        let msg = format!("{} required hard rule(s) in force", ids.len());
        CheckResult::pass(msg, Vec::new())
    } else {
        let msg = format!("{} required rule(s) not in force", bad.len());
        CheckResult::fail(msg, bad)
    }
}

fn forbidden_accounts_unused(setup: &Doc, _params: &Doc, _ctx: &CheckCtx) -> CheckResult {
    let forbidden: Vec<Option<&Doc>> = arr(get_path(setup, "identity.accounts"))
        .iter()
        .filter(|a| is_str(a.get("access"), "forbidden"))
        .map(|a| a.get("id"))
        .collect();
    if forbidden.is_empty() {
        return CheckResult::skip("no forbidden accounts declared");
    }
    let mut refs = Vec::new();
    if let Some(Value::Object(channels)) = setup.get("channels") {
        for (kind, list) in channels {
            for c in arr(Some(list)) {
                if forbidden.contains(&c.get("identity")) {
                    refs.push(format!("channel {kind}/{}", js(c.get("id"))));
                }
            }
        }
    }
    for l in arr(setup.get("loops")) {
        if forbidden.contains(&l.get("identity")) {
            refs.push(format!("loop {}", js(l.get("id"))));
        }
    }
    if refs.is_empty() {
        let msg = format!("{} forbidden account(s) unreferenced", forbidden.len());
        CheckResult::pass(msg, Vec::new())
    } else {
        let msg = format!("{} reference(s) to forbidden accounts", refs.len());
        CheckResult::fail(msg, refs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{CheckCtx, CheckResult, Status};
    use serde_json::{json, Value};
    use std::path::PathBuf;

    /// Assembled at run time so this file passes the secret lint.
    fn fake_token() -> String {
        ["xox", "b-1234567890-abcdefghijklmnop"].concat()
    }

    fn ctx() -> CheckCtx {
        CheckCtx {
            home: PathBuf::from("/home/user"),
            out_root: None,
        }
    }

    /// The reference selftest's fixture.
    fn base() -> Value {
        json!({
            "version": 1,
            "meta": { "instance": "fixture" },
            "identity": { "accounts": [
                { "id": "bot", "kind": "chat_bot", "access": "can_post" },
                { "id": "work", "kind": "chat_user", "access": "forbidden" }
            ] },
            "brain": { "gates": [{ "id": "gate", "applies_to": ["email"] }] },
            "tools": {
                "mcp_servers": [], "plugins": [], "skills": [],
                "permissions": { "allow": [], "deny": ["mcp__work__*"], "ask": [] },
                "denied_mcp_servers": ["work"], "hooks": []
            },
            "loops": [],
            "channels": {
                "slack": [{ "id": "home", "identity": "bot" }],
                "email": [{ "id": "mail", "identity": "bot" }],
                "social": []
            },
            "instructions": { "rules": [
                { "id": "money", "scope": "money", "severity": "hard", "date": "2026-01-01", "reason": "x" }
            ] }
        })
    }

    fn check(id: &str, setup: &Value, params: Value, ctx: &CheckCtx) -> CheckResult {
        let all = all();
        let c = all
            .iter()
            .find(|c| c.id() == id)
            .unwrap_or_else(|| panic!("no built-in check {id}"));
        c.run(setup, &params, ctx)
    }

    fn run(id: &str, setup: &Value, params: Value) -> Status {
        check(id, setup, params, &ctx()).status
    }

    fn push(list: &mut Value, item: Value) {
        list.as_array_mut().expect("a list").push(item);
    }

    fn assert_result(r: &CheckResult, status: Status, message: &str, evidence: &[&str]) {
        assert_eq!(r.status, status, "status of {r:?}");
        assert_eq!(r.message, message);
        assert_eq!(r.evidence, evidence);
    }

    #[test]
    fn nine_checks_in_reference_order() {
        let ids: Vec<String> = all().iter().map(|c| c.id().to_string()).collect();
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
                "forbidden_accounts_unused"
            ]
        );
        assert!(all().iter().all(|c| !c.doc().is_empty()));
    }

    // ---- the reference selftest's check cases, one to one

    #[test]
    fn deny_mcp_passes_when_blocked_and_denied() {
        let p = json!({ "servers": ["work"], "tool_patterns": ["mcp__work__*"] });
        assert_eq!(run("deny_mcp", &base(), p), Status::Pass);
    }

    #[test]
    fn deny_mcp_fails_when_server_not_blocked() {
        let mut s = base();
        s["tools"]["denied_mcp_servers"] = json!([]);
        assert_eq!(
            run("deny_mcp", &s, json!({ "servers": ["work"] })),
            Status::Fail
        );
    }

    #[test]
    fn deny_mcp_fails_when_tool_allowed_and_not_denied() {
        let mut s = base();
        s["tools"]["permissions"]["deny"] = json!([]);
        s["tools"]["permissions"]["allow"] = json!(["mcp__work__send"]);
        let p = json!({ "servers": ["work"], "tool_patterns": ["mcp__work__*"] });
        assert_eq!(run("deny_mcp", &s, p), Status::Fail);
    }

    #[test]
    fn deny_mcp_warns_on_dead_allow_under_deny() {
        let mut s = base();
        s["tools"]["permissions"]["allow"] = json!(["mcp__work__send"]);
        let p = json!({ "servers": ["work"], "tool_patterns": ["mcp__work__*"] });
        assert_eq!(run("deny_mcp", &s, p), Status::Warn);
    }

    #[test]
    fn no_payment_tools_fails_on_enabled_stripe_server() {
        let mut s = base();
        s["tools"]["mcp_servers"] = json!([{ "id": "stripe", "enabled": true }]);
        assert_eq!(run("no_payment_tools", &s, json!({})), Status::Fail);
    }

    #[test]
    fn no_payment_tools_fails_on_live_allow_for_a_defined_pay_server() {
        let mut s = base();
        s["tools"]["mcp_servers"] = json!([{ "id": "billing", "enabled": true }]);
        s["tools"]["permissions"]["allow"] = json!(["mcp__billing__charge"]);
        assert_eq!(run("no_payment_tools", &s, json!({})), Status::Fail);
    }

    #[test]
    fn no_payment_tools_warns_on_disabled_server() {
        let mut s = base();
        s["tools"]["mcp_servers"] = json!([{ "id": "stripe", "enabled": false }]);
        assert_eq!(run("no_payment_tools", &s, json!({})), Status::Warn);
    }

    #[test]
    fn no_payment_tools_respects_allow() {
        let mut s = base();
        s["tools"]["mcp_servers"] = json!([{ "id": "stripe", "enabled": true }]);
        assert_eq!(
            run("no_payment_tools", &s, json!({ "allow": ["stripe"] })),
            Status::Pass
        );
    }

    #[test]
    fn no_payment_tools_passes_clean() {
        assert_eq!(run("no_payment_tools", &base(), json!({})), Status::Pass);
    }

    #[test]
    fn forbidden_paths_fails_on_hook_pointing_at_path() {
        let mut s = base();
        s["tools"]["hooks"] =
            json!([{ "id": "h", "event": "PreToolUse", "command": "~/.ssh/run.sh" }]);
        assert_eq!(
            run("forbidden_paths", &s, json!({ "paths": ["~/.ssh"] })),
            Status::Fail
        );
    }

    #[test]
    fn forbidden_paths_fails_on_enabled_loop_cwd() {
        let mut s = base();
        s["loops"] = json!([{ "id": "l", "runner": "cron", "enabled": true, "working_dir": "/home/user/secret-dir" }]);
        assert_eq!(
            run("forbidden_paths", &s, json!({ "paths": ["~/secret-dir"] })),
            Status::Fail
        );
    }

    #[test]
    fn forbidden_paths_passes_clean() {
        assert_eq!(
            run("forbidden_paths", &base(), json!({ "paths": ["~/.ssh"] })),
            Status::Pass
        );
    }

    #[test]
    fn required_gate_passes_when_gate_covers_kind() {
        let p = json!({ "gate": "gate", "channel_kinds": ["email"] });
        assert_eq!(run("required_gate", &base(), p), Status::Pass);
    }

    #[test]
    fn required_gate_fails_on_ungated_social() {
        let mut s = base();
        s["channels"]["social"] = json!([{ "id": "x", "platform": "x" }]);
        let p = json!({ "gate": "gate", "channel_kinds": ["email", "social"] });
        assert_eq!(run("required_gate", &s, p), Status::Fail);
    }

    #[test]
    fn required_gate_ignores_halted_channels() {
        let mut s = base();
        s["channels"]["social"] = json!([{ "id": "x", "status": "HALTED" }]);
        let p = json!({ "gate": "gate", "channel_kinds": ["social"] });
        assert_eq!(run("required_gate", &s, p), Status::Pass);
    }

    #[test]
    fn required_gate_fails_when_gate_missing() {
        let p = json!({ "gate": "nope", "channel_kinds": ["email"] });
        assert_eq!(run("required_gate", &base(), p), Status::Fail);
    }

    #[test]
    fn channel_identity_fails_on_wrong_identity() {
        let mut s = base();
        push(
            &mut s["channels"]["slack"],
            json!({ "id": "b", "identity": "other" }),
        );
        let p = json!({ "kind": "slack", "allowed": ["bot"] });
        assert_eq!(run("channel_identity", &s, p), Status::Fail);
    }

    #[test]
    fn channel_identity_fails_on_forbidden_account() {
        let mut s = base();
        push(
            &mut s["channels"]["slack"],
            json!({ "id": "w", "identity": "work", "status": "forbidden" }),
        );
        let p = json!({ "kind": "slack", "allowed": ["bot"] });
        assert_eq!(run("channel_identity", &s, p), Status::Fail);
    }

    #[test]
    fn channel_identity_passes_clean() {
        let p = json!({ "kind": "slack", "allowed": ["bot"] });
        assert_eq!(run("channel_identity", &base(), p), Status::Pass);
    }

    #[test]
    fn secrets_by_reference_fails_on_token_value() {
        let mut s = base();
        s["identity"]["accounts"][0]["note"] = json!(fake_token());
        assert_eq!(run("secrets_by_reference", &s, json!({})), Status::Fail);
    }

    #[test]
    fn secrets_by_reference_passes_on_path_refs() {
        let mut s = base();
        s["identity"]["accounts"][0]["credential_ref"] = json!("~/.bot_token");
        assert_eq!(run("secrets_by_reference", &s, json!({})), Status::Pass);
    }

    #[test]
    fn loop_success_metric_fails_on_enabled_loop_without_metric() {
        let mut s = base();
        s["loops"] = json!([{ "id": "l", "runner": "cron", "enabled": true }]);
        assert_eq!(run("loop_success_metric", &s, json!({})), Status::Fail);
    }

    #[test]
    fn loop_success_metric_ignores_disabled() {
        let mut s = base();
        s["loops"] = json!([{ "id": "l", "runner": "cron", "enabled": false }]);
        assert_eq!(run("loop_success_metric", &s, json!({})), Status::Pass);
    }

    #[test]
    fn required_rules_fails_on_missing_or_soft_rule() {
        let mut s = base();
        assert_eq!(
            run("required_rules", &s, json!({ "ids": ["money", "nope"] })),
            Status::Fail
        );
        s["instructions"]["rules"][0]["severity"] = json!("standing");
        assert_eq!(
            run("required_rules", &s, json!({ "ids": ["money"] })),
            Status::Fail
        );
    }

    #[test]
    fn required_rules_passes() {
        assert_eq!(
            run("required_rules", &base(), json!({ "ids": ["money"] })),
            Status::Pass
        );
    }

    #[test]
    fn forbidden_accounts_unused_fails_when_referenced() {
        let mut s = base();
        push(
            &mut s["channels"]["email"],
            json!({ "id": "w", "identity": "work" }),
        );
        assert_eq!(
            run("forbidden_accounts_unused", &s, json!({})),
            Status::Fail
        );
    }

    // ---- cases the brief adds

    #[test]
    fn required_gate_covers_a_channel_by_platform() {
        let mut s = base();
        s["brain"]["gates"][0]["applies_to"] = json!(["email", "X"]);
        s["channels"]["social"] = json!([{ "id": "xacct", "platform": "x" }]);
        let p = json!({ "gate": "gate", "channel_kinds": ["email", "social"] });
        let r = check("required_gate", &s, p, &ctx());
        assert_result(
            &r,
            Status::Pass,
            "gate gate covers 2 active channel(s)",
            &[],
        );
    }

    #[test]
    fn channel_identity_warns_on_undeclared_identity() {
        let mut s = base();
        push(
            &mut s["channels"]["slack"],
            json!({ "id": "b", "identity": "ghost" }),
        );
        let r = check(
            "channel_identity",
            &s,
            json!({ "kind": "slack", "allowed": [] }),
            &ctx(),
        );
        assert_result(
            &r,
            Status::Warn,
            "slack identities ok, 1 note(s)",
            &["b: identity ghost not declared in identity.accounts"],
        );
    }

    #[test]
    fn forbidden_paths_fails_when_out_root_is_inside_a_forbidden_path() {
        let c = CheckCtx {
            home: PathBuf::from("/home/user"),
            out_root: Some(PathBuf::from("/home/user/.claude/out")),
        };
        let r = check(
            "forbidden_paths",
            &base(),
            json!({ "paths": ["~/.claude"] }),
            &c,
        );
        assert_result(
            &r,
            Status::Fail,
            "1 reference(s) to forbidden paths",
            &["apply output dir -> ~/.claude"],
        );
        let outside = CheckCtx {
            home: PathBuf::from("/home/user"),
            out_root: Some(PathBuf::from("/home/user/proj/out")),
        };
        let r = check(
            "forbidden_paths",
            &base(),
            json!({ "paths": ["~/.claude"] }),
            &outside,
        );
        assert_result(&r, Status::Pass, "1 forbidden path(s) unreferenced", &[]);
    }

    // ---- exact wording, checked against the reference implementation

    #[test]
    fn deny_mcp_wording() {
        let p = json!({ "servers": ["work"], "tool_patterns": ["mcp__work__*"] });
        let r = check("deny_mcp", &base(), p.clone(), &ctx());
        assert_result(
            &r,
            Status::Pass,
            "1 server(s) blocked, 1 tool pattern(s) denied",
            &[],
        );

        let mut s = base();
        s["tools"]["permissions"]["allow"] =
            json!(["mcp__work__send", "mcp__work__read", "Bash(ls)"]);
        let r = check("deny_mcp", &s, p.clone(), &ctx());
        assert_result(
            &r,
            Status::Warn,
            "1 server(s) blocked, 1 tool pattern(s) denied; 2 dead allow entries to clean up",
            &[
                "dead allow entry (deny wins): mcp__work__send",
                "dead allow entry (deny wins): mcp__work__read",
            ],
        );

        let mut s = base();
        s["tools"]["permissions"]["allow"] = json!(["mcp__work__send"]);
        let r = check("deny_mcp", &s, p.clone(), &ctx());
        assert_result(
            &r,
            Status::Warn,
            "1 server(s) blocked, 1 tool pattern(s) denied; 1 dead allow entry to clean up",
            &["dead allow entry (deny wins): mcp__work__send"],
        );

        let mut s = base();
        s["tools"]["permissions"]["deny"] = json!([]);
        s["tools"]["permissions"]["allow"] = json!(["mcp__work__send"]);
        let r = check("deny_mcp", &s, p.clone(), &ctx());
        assert_result(
            &r,
            Status::Fail,
            "2 way(s) the denied MCP surface is reachable",
            &[
                "tool pattern mcp__work__* missing from permissions.deny",
                "tool pattern mcp__work__* allowed by: mcp__work__send",
            ],
        );
    }

    #[test]
    fn deny_mcp_sees_plugin_suffixed_and_disabled_definitions() {
        let p = json!({ "servers": ["work"], "tool_patterns": ["mcp__work__*"] });
        let mut s = base();
        s["tools"]["denied_mcp_servers"] = json!([]);
        s["tools"]["mcp_servers"] =
            json!([{ "id": "work@market", "enabled": true, "source": "plugin" }]);
        let r = check("deny_mcp", &s, p.clone(), &ctx());
        assert_result(
            &r,
            Status::Fail,
            "2 way(s) the denied MCP surface is reachable",
            &[
                "server \"work\" not in denied_mcp_servers",
                "server \"work\" defined and enabled in plugin",
            ],
        );

        let mut s = base();
        s["tools"]["mcp_servers"] = json!([
            { "id": "work", "enabled": true, "source": "~/.claude.json" },
            { "id": "work@x" }
        ]);
        let r = check("deny_mcp", &s, p.clone(), &ctx());
        assert_result(
            &r,
            Status::Pass,
            "1 server(s) blocked, 1 tool pattern(s) denied",
            &[
                "server \"work\" still defined in ~/.claude.json but blocked",
                "server \"work\" still defined in setup but blocked",
            ],
        );

        let mut s = base();
        s["tools"]["denied_mcp_servers"] = json!([]);
        s["tools"]["mcp_servers"] = json!([{ "id": "work", "enabled": false }]);
        let r = check("deny_mcp", &s, json!({ "servers": ["work"] }), &ctx());
        assert_result(
            &r,
            Status::Fail,
            "1 way(s) the denied MCP surface is reachable",
            &[
                "server \"work\" not in denied_mcp_servers",
                "server \"work\" still defined in setup but blocked",
            ],
        );
    }

    #[test]
    fn deny_mcp_globs_are_case_insensitive_and_cover_narrower_patterns() {
        let mut s = base();
        s["tools"]["permissions"]["deny"] = json!(["mcp__*"]);
        let p =
            json!({ "servers": ["work"], "tool_patterns": ["mcp__work__*", "mcp__work__post"] });
        let r = check("deny_mcp", &s, p, &ctx());
        assert_result(
            &r,
            Status::Pass,
            "1 server(s) blocked, 2 tool pattern(s) denied",
            &[],
        );

        let mut s = base();
        s["tools"]["permissions"]["deny"] = json!(["MCP__WORK__*"]);
        s["tools"]["permissions"]["allow"] = json!(["mcp__Work__Send"]);
        let p = json!({ "servers": ["work"], "tool_patterns": ["mcp__work__*"] });
        let r = check("deny_mcp", &s, p, &ctx());
        assert_eq!(r.status, Status::Warn);
        assert_eq!(
            r.evidence,
            ["dead allow entry (deny wins): mcp__Work__Send"]
        );

        let mut s = base();
        s["tools"]["permissions"]["deny"] = json!([]);
        s["tools"]["permissions"]["allow"] = json!(["mcp__work__*"]);
        let p = json!({ "servers": ["work"], "tool_patterns": ["mcp__work__post"] });
        let r = check("deny_mcp", &s, p, &ctx());
        assert_eq!(
            r.evidence,
            [
                "tool pattern mcp__work__post missing from permissions.deny",
                "tool pattern mcp__work__post allowed by: mcp__work__*"
            ]
        );

        let r = check("deny_mcp", &base(), json!({}), &ctx());
        assert_result(
            &r,
            Status::Pass,
            "0 server(s) blocked, 0 tool pattern(s) denied",
            &[],
        );
    }

    #[test]
    fn no_payment_tools_wording_and_states() {
        let mut s = base();
        s["tools"]["mcp_servers"] = json!([
            { "id": "shop", "enabled": true, "url": "https://example.com/google-ads/mcp" },
            { "id": "pay", "enabled": true, "command": "npx", "args": ["x"] },
            { "id": "square-up", "enabled": true },
            { "id": "quiet", "enabled": true, "command": "node ~/bin/buyer.js" },
            { "id": "work", "enabled": true, "command": "stripe-cli" },
            { "id": "paypal@plugins", "enabled": true }
        ]);
        s["tools"]["plugins"] = json!([
            { "id": "billing-helper@market", "enabled": true },
            { "id": "checkout@m", "enabled": false }
        ]);
        s["tools"]["skills"] = json!([{ "id": "Top-Up-Wallet" }, { "id": "buy" }]);
        s["tools"]["permissions"]["allow"] = json!([
            "mcp__paypal__send",
            "mcp__stripe__charge",
            "Bash(stripe login)",
            "mcp__work__payment",
            "WebFetch(domain:stripe.com)",
            "mcp__square_up__pay",
            "mcp__shop__buy"
        ]);
        push(
            &mut s["identity"]["accounts"],
            json!({ "id": "coinbase", "kind": "exchange", "access": "read_only" }),
        );
        push(
            &mut s["identity"]["accounts"],
            json!({ "id": "venmo", "kind": "pay" }),
        );
        push(
            &mut s["identity"]["accounts"],
            json!({ "id": "ads", "kind": "ads", "access": "can_post", "scopes": ["adspend", "reports"] }),
        );
        let r = check(
            "no_payment_tools",
            &s,
            json!({ "patterns": ["Quiet", "buyer"] }),
            &ctx(),
        );
        assert_result(
            &r,
            Status::Fail,
            "12 live payment-capable surface(s)",
            &[
                "mcp shop (matches \"google-ads\")",
                "mcp square-up (matches \"square\")",
                "mcp quiet (matches \"quiet\")",
                "mcp paypal@plugins (matches \"paypal\")",
                "plugin billing-helper@market (matches \"billing\")",
                "skill Top-Up-Wallet (matches \"top-up\")",
                "skill buy (matches \"buy\")",
                "allow mcp__paypal__send (matches \"paypal\")",
                "allow WebFetch(domain:stripe.com) (matches \"stripe\")",
                "allow mcp__square_up__pay (matches \"square\")",
                "allow mcp__shop__buy (matches \"buy\")",
                "account ads (matches \"adspend\")",
                "dormant: mcp work (matches \"stripe\"; disabled)",
                "dormant: plugin checkout@m (matches \"checkout\"; disabled)",
                "dormant: allow mcp__stripe__charge (matches \"stripe\"; server not defined)",
                "dormant: allow Bash(stripe login) (matches \"stripe\"; exact shell command allow)",
                "dormant: allow mcp__work__payment (matches \"payment\"; denied)",
                "dormant: account coinbase (matches \"coinbase\"; read_only)",
                "dormant: account venmo (matches \"venmo\"; undefined)",
            ],
        );
    }

    #[test]
    fn no_payment_tools_dormant_only_and_long_ids() {
        let mut s = base();
        s["tools"]["plugins"] = json!([{ "id": "stripe", "enabled": false }]);
        let r = check("no_payment_tools", &s, json!({}), &ctx());
        assert_result(
            &r,
            Status::Warn,
            "no live payment-capable tools; 1 dormant entry to review",
            &["plugin stripe (matches \"stripe\"; disabled)"],
        );

        let mut s = base();
        let long = format!("Bash({} stripe)", "x".repeat(130));
        s["tools"]["permissions"]["allow"] = json!([long]);
        let r = check("no_payment_tools", &s, json!({}), &ctx());
        let shown = format!("{}...", &long[..117]);
        assert_eq!(
            r.evidence,
            [format!(
                "allow {shown} (matches \"stripe\"; exact shell command allow)"
            )]
        );

        let mut s = base();
        s["tools"]["skills"] = json!([{ "id": "café-stripe" }, { "id": "ÉLAN payment" }]);
        let r = check("no_payment_tools", &s, json!({}), &ctx());
        assert_eq!(
            r.evidence,
            [
                "skill café-stripe (matches \"stripe\")",
                "skill ÉLAN payment (matches \"payment\")"
            ]
        );

        let mut s = base();
        s["tools"]["permissions"]["allow"] = json!(["mcp__stripe__x"]);
        let r = check(
            "no_payment_tools",
            &s,
            json!({ "allow": ["mcp__stripe__x"] }),
            &ctx(),
        );
        assert_result(&r, Status::Pass, "no payment-capable tools", &[]);
    }

    #[test]
    fn no_payment_tools_ignores_an_empty_pattern() {
        // The reference's first matching word is "" (falsy), which it treats as no match.
        let mut s = base();
        s["tools"]["skills"] = json!([{ "id": "-a-" }, { "id": "a--b" }]);
        let r = check("no_payment_tools", &s, json!({ "patterns": [""] }), &ctx());
        assert_result(&r, Status::Pass, "no payment-capable tools", &[]);
    }

    #[test]
    fn forbidden_paths_scans_hooks_enabled_loops_and_enabled_servers() {
        let mut s = base();
        s["tools"]["hooks"] = json!([
            { "id": "h1", "command": "bash $HOME/.ssh/x.sh" },
            { "id": "h2", "command": "cat /home/user/.sshx" },
            { "id": "h3" }
        ]);
        s["loops"] = json!([
            { "id": "l1", "enabled": true, "program": ["/bin/sh", "/home/user/.ssh", "--dir=~/.ssh/"], "working_dir": "~" },
            { "id": "l2", "enabled": false, "program": ["/home/user/.ssh/a"] },
            { "id": "l3", "enabled": "yes", "program": ["~/.aws/credentials"] }
        ]);
        s["tools"]["mcp_servers"] = json!([
            { "id": "m1", "command": "${HOME}/.ssh/bin", "args": ["--key", "~/.ssh/id"] },
            { "id": "m2", "enabled": false, "command": "~/.ssh/x" }
        ]);
        let c = CheckCtx {
            home: PathBuf::from("/home/user"),
            out_root: Some(PathBuf::from("/tmp/out")),
        };
        let r = check(
            "forbidden_paths",
            &s,
            json!({ "paths": ["~/.ssh/", "~/.aws", "/etc"] }),
            &c,
        );
        assert_result(
            &r,
            Status::Fail,
            "6 reference(s) to forbidden paths",
            &[
                "hook h1 -> ~/.ssh",
                "loop l1 -> ~/.ssh",
                "loop l1 -> ~/.ssh",
                "loop l3 -> ~/.aws",
                "mcp m1 -> ~/.ssh",
                "mcp m1 -> ~/.ssh",
            ],
        );
        let r = check("forbidden_paths", &base(), json!({}), &c);
        assert_result(&r, Status::Pass, "0 forbidden path(s) unreferenced", &[]);
    }

    #[test]
    fn required_gate_wording_and_activity_rules() {
        let mut s = base();
        s["brain"]["gates"] = json!([
            { "id": "gate", "applies_to": ["MAIL2", "Reddit"] },
            { "id": "other", "applies_to": ["social"] }
        ]);
        s["channels"]["email"] = json!([
            { "id": "mail", "identity": "bot" },
            { "id": "mail2" },
            { "id": "mail3", "gate": "gate" },
            { "id": "mail4", "active": false },
            { "id": "mail5", "role": "paused; resumes later" },
            { "id": "mail6", "role": "primary, killed later" }
        ]);
        s["channels"]["social"] = json!([
            { "id": "r", "platform": ["REDDIT", "x"] },
            { "id": "y", "platform": "YouTube" },
            { "id": "z", "status": "frozen until spring" }
        ]);
        let p = json!({ "gate": "gate", "channel_kinds": ["email", "social", "sms"] });
        let r = check("required_gate", &s, p, &ctx());
        assert_result(
            &r,
            Status::Fail,
            "3 active channel(s) bypass gate gate",
            &["email/mail", "email/mail6", "social/y"],
        );

        let r = check(
            "required_gate",
            &base(),
            json!({ "gate": "nope", "channel_kinds": ["email"] }),
            &ctx(),
        );
        assert_result(
            &r,
            Status::Fail,
            "gate \"nope\" is not defined in brain.gates",
            &[],
        );
        let r = check(
            "required_gate",
            &base(),
            json!({ "channel_kinds": ["email"] }),
            &ctx(),
        );
        assert_eq!(
            r.message,
            "gate \"undefined\" is not defined in brain.gates"
        );

        // The kind is matched as written; only applies_to is lower-cased.
        let r = check(
            "required_gate",
            &base(),
            json!({ "gate": "gate", "channel_kinds": ["Email"] }),
            &ctx(),
        );
        assert_result(
            &r,
            Status::Pass,
            "gate gate covers 0 active channel(s)",
            &[],
        );
    }

    #[test]
    fn channel_identity_wording() {
        let mut s = base();
        s["channels"]["slack"] = json!([
            { "id": "home", "identity": "bot" },
            { "id": "n1" },
            { "id": "n2", "identity": "none" },
            { "id": "n3", "identity": "none", "status": "halted" },
            { "id": "p1", "identity": "person", "status": "stopped" },
            { "id": "p2", "identity": "person" },
            { "id": "w1", "identity": "work", "active": false }
        ]);
        let r = check(
            "channel_identity",
            &s,
            json!({ "kind": "slack", "allowed": ["bot", "helper"] }),
            &ctx(),
        );
        assert_result(
            &r,
            Status::Fail,
            "2 slack channel(s) use a disallowed identity",
            &[
                "p2 posts as person (allowed: bot, helper)",
                "w1 uses forbidden account work",
                "n1: no identity declared",
                "n2: no identity declared",
                "p1: identity person not declared in identity.accounts",
                "p2: identity person not declared in identity.accounts",
            ],
        );

        // A later account with the same id wins, as in the reference's Map.
        let mut s = base();
        push(
            &mut s["identity"]["accounts"],
            json!({ "id": "bot", "access": "forbidden" }),
        );
        let r = check(
            "channel_identity",
            &s,
            json!({ "kind": "slack", "allowed": ["bot"] }),
            &ctx(),
        );
        assert_result(
            &r,
            Status::Fail,
            "1 slack channel(s) use a disallowed identity",
            &["home uses forbidden account bot"],
        );

        let r = check("channel_identity", &base(), json!({}), &ctx());
        assert_result(
            &r,
            Status::Pass,
            "all undefined channels post through allowed identities",
            &[],
        );
    }

    #[test]
    fn secrets_by_reference_reports_paths_and_pattern_ids() {
        let mut s = base();
        s["tools"]["mcp_servers"] = json!([{
            "id": "a",
            "args": ["--token", fake_token(), "ok"],
            "url": format!("https://user:{}@example.com/x", "hunter22pass")
        }]);
        let key = ["AKIA", "ABCDEFGHIJKLMNOP"].concat();
        s["loops"] = json!([{ "id": "l", "program": [["nested", key]] }]);
        s["meta"]["notes"] = json!(format!("Bearer {} and {}", "a".repeat(30), fake_token()));
        let r = check("secrets_by_reference", &s, json!({}), &ctx());
        assert_result(
            &r,
            Status::Fail,
            "5 secret-looking value(s) in setup",
            &[
                "meta.notes: slack-token",
                "meta.notes: bearer-header",
                "tools.mcp_servers[0].args[1]: slack-token",
                "tools.mcp_servers[0].url: url-credentials",
                "loops[0].program[0][1]: aws-access-key",
            ],
        );
        let r = check("secrets_by_reference", &base(), json!({}), &ctx());
        assert_result(&r, Status::Pass, "no secret values; references only", &[]);
    }

    #[test]
    fn loop_success_metric_managed_rules() {
        let mut s = base();
        s["loops"] = json!([
            { "id": "a", "enabled": true, "managed": false },
            { "id": "b", "enabled": true, "success_metric": "x" },
            { "id": "c", "enabled": true, "success_metric": "" },
            { "enabled": true }
        ]);
        let r = check("loop_success_metric", &s, json!({}), &ctx());
        assert_result(
            &r,
            Status::Fail,
            "2 enabled loop(s) without a success metric",
            &["c", "undefined"],
        );

        let mut s = base();
        s["loops"] = json!([{ "id": "a", "enabled": true, "managed": false }]);
        for p in [
            json!({ "only_managed": false }),
            json!({ "only_managed": null }),
        ] {
            let r = check("loop_success_metric", &s, p, &ctx());
            assert_result(
                &r,
                Status::Fail,
                "1 enabled loop(s) without a success metric",
                &["a"],
            );
        }
        let r = check("loop_success_metric", &s, json!({}), &ctx());
        assert_result(
            &r,
            Status::Pass,
            "every enabled loop has a success metric",
            &[],
        );
    }

    #[test]
    fn required_rules_wording() {
        let mut s = base();
        for r in [
            json!({ "id": "old", "severity": "hard", "status": "superseded" }),
            json!({ "id": "nosev" }),
            json!({ "id": "money", "severity": "standing" }),
        ] {
            push(&mut s["instructions"]["rules"], r);
        }
        let p = json!({ "ids": ["money", "old", "nosev", "gone"] });
        let r = check("required_rules", &s, p, &ctx());
        assert_result(
            &r,
            Status::Fail,
            "4 required rule(s) not in force",
            &[
                "money: severity standing, expected hard",
                "old: superseded",
                "nosev: severity undefined, expected hard",
                "gone: missing",
            ],
        );
        let r = check(
            "required_rules",
            &base(),
            json!({ "ids": ["money"] }),
            &ctx(),
        );
        assert_result(&r, Status::Pass, "1 required hard rule(s) in force", &[]);
        let r = check("required_rules", &base(), json!({}), &ctx());
        assert_result(&r, Status::Pass, "0 required hard rule(s) in force", &[]);
    }

    #[test]
    fn forbidden_accounts_unused_wording() {
        let mut s = base();
        s["identity"]["accounts"] = json!([{ "id": "bot", "access": "can_post" }]);
        let r = check("forbidden_accounts_unused", &s, json!({}), &ctx());
        assert_result(&r, Status::Skip, "no forbidden accounts declared", &[]);

        let r = check("forbidden_accounts_unused", &base(), json!({}), &ctx());
        assert_result(&r, Status::Pass, "1 forbidden account(s) unreferenced", &[]);

        let mut s = base();
        s["loops"] = json!([{ "id": "l", "identity": "work" }, { "id": "k", "identity": "bot" }]);
        s["channels"]["custom"] = json!([{ "id": "c", "identity": "work" }]);
        push(
            &mut s["identity"]["accounts"],
            json!({ "id": "old", "access": "forbidden" }),
        );
        let r = check("forbidden_accounts_unused", &s, json!({}), &ctx());
        assert_result(
            &r,
            Status::Fail,
            "2 reference(s) to forbidden accounts",
            &["channel custom/c", "loop l"],
        );
    }

    #[test]
    fn active_channel_rules() {
        assert!(is_active_channel(&json!({ "id": "a" })));
        assert!(!is_active_channel(&json!({ "active": false })));
        assert!(is_active_channel(&json!({ "active": "no" })));
        assert!(!is_active_channel(&json!({ "status": "Superseded by b" })));
        assert!(!is_active_channel(
            &json!({ "role": "blocked; was primary" })
        ));
        assert!(is_active_channel(
            &json!({ "role": "primary; once blocked" })
        ));
        assert!(is_active_channel(
            &json!({ "role": "primary, later removed" })
        ));
    }

    #[test]
    fn scalar_list_params_count_as_one_item() {
        // Written `servers: work` instead of `servers: [work]`.
        let mut s = base();
        s["tools"]["denied_mcp_servers"] = json!([]);
        let r = check("deny_mcp", &s, json!({ "servers": "work" }), &ctx());
        assert_eq!(r.evidence, ["server \"work\" not in denied_mcp_servers"]);

        let mut s = base();
        s["channels"]["social"] = json!([{ "id": "x" }]);
        let p = json!({ "gate": "gate", "channel_kinds": "social" });
        let r = check("required_gate", &s, p, &ctx());
        assert_result(
            &r,
            Status::Fail,
            "1 active channel(s) bypass gate gate",
            &["social/x"],
        );
    }
}
