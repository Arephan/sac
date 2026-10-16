//! Safety policies: a `Check` runs against a whole setup and returns pass/fail/warn/skip with
//! evidence; `run_policies` applies every `safety.policies[]` entry; hard failures fail `verify`.

use crate::util::s;
use crate::Doc;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub mod checks;
pub mod external;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Pass,
    Fail,
    Warn,
    Skip,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Warn => "WARN",
            Status::Skip => "SKIP",
        }
    }
    pub fn parse(s: &str) -> Option<Status> {
        match s {
            "pass" => Some(Status::Pass),
            "fail" => Some(Status::Fail),
            "warn" => Some(Status::Warn),
            "skip" => Some(Status::Skip),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckResult {
    pub status: Status,
    pub message: String,
    pub evidence: Vec<String>,
}

impl CheckResult {
    pub fn pass(message: impl Into<String>, evidence: Vec<String>) -> Self {
        CheckResult {
            status: Status::Pass,
            message: message.into(),
            evidence,
        }
    }
    pub fn fail(message: impl Into<String>, evidence: Vec<String>) -> Self {
        CheckResult {
            status: Status::Fail,
            message: message.into(),
            evidence,
        }
    }
    pub fn warn(message: impl Into<String>, evidence: Vec<String>) -> Self {
        CheckResult {
            status: Status::Warn,
            message: message.into(),
            evidence,
        }
    }
    pub fn skip(message: impl Into<String>) -> Self {
        CheckResult {
            status: Status::Skip,
            message: message.into(),
            evidence: Vec::new(),
        }
    }
}

/// What a check may know about the machine without touching it.
#[derive(Debug, Clone)]
pub struct CheckCtx {
    pub home: PathBuf,
    pub out_root: Option<PathBuf>,
}

pub trait Check {
    /// snake_case, matches `safety.policies[].check`.
    fn id(&self) -> &str;
    fn doc(&self) -> &str;
    /// Pure: reads `setup` and `params`, never the machine.
    fn run(&self, setup: &Doc, params: &Doc, ctx: &CheckCtx) -> CheckResult;
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PolicyResult {
    pub id: String,
    pub check: String,
    pub severity: String,
    pub rule: Option<String>,
    pub status: Status,
    pub message: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PolicyReport {
    pub results: Vec<PolicyResult>,
    /// True when no `hard` policy failed.
    pub ok: bool,
}

impl PolicyReport {
    pub fn hard_fails(&self) -> Vec<&PolicyResult> {
        self.results
            .iter()
            .filter(|r| r.status == Status::Fail && r.severity == "hard")
            .collect()
    }
    pub fn soft_fails(&self) -> Vec<&PolicyResult> {
        self.results
            .iter()
            .filter(|r| r.status == Status::Fail && r.severity != "hard")
            .collect()
    }
}

/// The nine built-in checks.
pub fn builtin_checks() -> Vec<Box<dyn Check>> {
    checks::all()
}

/// Built-ins plus every executable in `<instance>/checks/` (sorted by name); an external check
/// whose id equals a built-in replaces it.
pub fn load_checks(instance_dir: Option<&Path>) -> Result<Vec<Box<dyn Check>>> {
    let mut list = builtin_checks();
    if let Some(dir) = instance_dir.map(|d| d.join("checks")) {
        if dir.is_dir() {
            for c in external::load_dir(&dir)? {
                match list.iter().position(|x| x.id() == c.id()) {
                    Some(i) => list[i] = c,
                    None => list.push(c),
                }
            }
        }
    }
    Ok(list)
}

/// Run every policy in `setup.safety.policies` against `setup`.
pub fn run_policies(setup: &Doc, checks: &[Box<dyn Check>], ctx: &CheckCtx) -> PolicyReport {
    let empty = serde_json::json!({});
    let mut results = Vec::new();
    for p in crate::util::arr(crate::util::get_path(setup, "safety.policies")) {
        let check_id = s(p.get("check")).to_string();
        let params = p.get("params").unwrap_or(&empty);
        let r = match checks.iter().find(|c| c.id() == check_id) {
            None => CheckResult::fail(format!("unknown check \"{check_id}\""), Vec::new()),
            Some(c) => {
                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    c.run(setup, params, ctx)
                })) {
                    Ok(r) => r,
                    Err(_) => CheckResult::fail("check crashed: panic", Vec::new()),
                }
            }
        };
        results.push(PolicyResult {
            id: s(p.get("id")).to_string(),
            check: check_id,
            severity: s(p.get("severity")).to_string(),
            rule: p
                .get("rule")
                .and_then(|x| x.as_str())
                .map(|x| x.to_string()),
            status: r.status,
            message: r.message,
            evidence: r.evidence,
        });
    }
    let ok = !results
        .iter()
        .any(|r| r.status == Status::Fail && r.severity == "hard");
    PolicyReport { results, ok }
}

/// Text block mirroring the reference's `formatPolicyResults`.
pub fn format_policy_results(r: &PolicyReport, verbose: bool) -> String {
    let mut lines = Vec::new();
    for x in &r.results {
        lines.push(format!(
            "  [{}] {:<8} {} ({}): {}",
            x.status.label(),
            x.severity,
            x.id,
            x.check,
            x.message
        ));
        let show = matches!(x.status, Status::Fail | Status::Warn) || verbose;
        if show {
            let limit = if verbose { 50 } else { 8 };
            for e in x.evidence.iter().take(limit) {
                lines.push(format!("         - {e}"));
            }
            if !verbose && x.evidence.len() > 8 {
                lines.push(format!(
                    "         … {} more (use --verbose)",
                    x.evidence.len() - 8
                ));
            }
        }
    }
    lines.join("\n")
}
