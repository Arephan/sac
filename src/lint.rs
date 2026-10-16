//! Secret and personal-data lint over everything meant to be shareable (sources, examples,
//! docs). Ported from the Node prototype (tag v0.2.0).
//!
//! Two kinds of detectors:
//!  1. Generic: secret-shaped strings, real-looking emails, phone numbers, user home paths,
//!     chat-platform member and channel ids.
//!  2. Instance-derived: every `instance/<name>/` contributes terms that identify its owner:
//!     `lint-denylist.txt` (one term per line) plus values pulled from its `setup.yaml` and
//!     `overrides.yaml` (account principals and workspaces, channel handles, senders and ids,
//!     home directory user names). An instance may list exact phrases in `lint-allow.txt` that
//!     are allowed despite containing a term; they are blanked before matching.
//!
//! The tool never contains those terms itself; it reads them from the instances at run time.

use crate::init::resolve;
use crate::plan::js_display;
use crate::secrets::find_secrets;
use crate::util::{arr, is_truthy};
use crate::yaml::read_yaml;
use crate::Doc;
use indexmap::IndexMap;
use regex::{NoExpand, Regex};
use serde_json::Value;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use walkdir::WalkDir;

#[derive(Debug, Clone, serde::Serialize)]
pub struct LintFinding {
    pub at: String,
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct LintReport {
    pub files: usize,
    pub terms: usize,
    pub allowed: Vec<String>,
    pub findings: Vec<LintFinding>,
}

/// Top-level entries `default_targets` leaves out: build output, dependencies, version
/// control, personal instances and agent working folders.
const DEFAULT_EXCLUDE: [&str; 7] = [
    "target",
    "node_modules",
    "out",
    ".git",
    "instance",
    ".superpowers",
    ".worktrees",
];

/// Directories never descended into, at any depth.
const SKIP_DIRS: [&str; 4] = ["node_modules", "out", ".git", "target"];

/// Extensions of scanned files (see `is_text_name` for names without one).
const TEXT_EXT: [&str; 12] = [
    "rs", "mjs", "js", "json", "yaml", "yml", "md", "txt", "sh", "toml", "ts", "py",
];

/// Files of this size or larger are skipped.
const MAX_BYTES: u64 = 5_000_000;

/// Placeholder user names allowed in home paths.
const SAFE_HOME_USERS: [&str; 11] = [
    "you", "me", "user", "username", "example", "alice", "bob", "shared", "<you>", "runner", "alex",
];

/// Findings listed by `format_lint`.
const LISTED: usize = 200;

struct Patterns {
    email: Regex,
    safe_email: Regex,
    file_like: Regex,
    home: Regex,
    chat: Regex,
    chat_whole: Regex,
    upper_token: Regex,
    piece_split: Regex,
    ignored_term: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let re = |s: &str| Regex::new(s).expect("lint pattern compiles");
        Patterns {
            email: re(r"(?-u:\b)[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}(?-u:\b)"),
            safe_email: re(
                r"(?i)@(?:example\.(?:com|org|net)|users\.noreply\.github\.com|anthropic\.com)$",
            ),
            file_like: re(r"^[A-Za-z0-9_.-]+@[A-Za-z0-9_.-]+\.(?:mjs|js|json|md)$"),
            home: re(r"/(?:Users|home)/([A-Za-z0-9._-]+)"),
            chat: re(r"(?-u:\b)[CDGUW][A-Z0-9]{8,11}(?-u:\b)"),
            chat_whole: re(r"^[CDGUW][A-Z0-9]{8,11}$"),
            upper_token: re(r"^[A-Z0-9]{9,}$"),
            piece_split: re(r"[,+]| and "),
            ignored_term: re(r"(?i)^(?:true|false|none|null|unknown|n/a)$"),
        }
    })
}

/// JavaScript `String.prototype.trim` (it trims exactly the `\s` set).
fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// JavaScript `\s`.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// JavaScript `\w`.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `line.replace(/#.*/, '')`: drop a comment up to the end of the line.
fn strip_comment(line: &str) -> String {
    match line.find('#') {
        None => line.to_string(),
        Some(i) => {
            let end = line[i..]
                .find(['\r', '\u{2028}', '\u{2029}'])
                .map_or(line.len(), |j| i + j);
            format!("{}{}", &line[..i], &line[end..])
        }
    }
}

fn read_lossy(p: &Path) -> Option<String> {
    std::fs::read(p)
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Entry names of a directory, sorted (empty when it cannot be read).
fn sorted_entries(dir: &Path) -> Vec<OsString> {
    let mut v: Vec<OsString> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn add_term(terms: &mut IndexMap<String, String>, t: &str, origin: &str) {
    let t = js_trim(t);
    if t.encode_utf16().count() < 3 || patterns().ignored_term.is_match(t) {
        return;
    }
    terms
        .entry(t.to_lowercase())
        .or_insert_with(|| origin.to_string());
}

/// `String(v || '')`.
fn text_of(v: Option<&Doc>) -> String {
    if is_truthy(v) {
        js_display(v)
    } else {
        String::new()
    }
}

/// Terms one parsed `setup.yaml` or `overrides.yaml` gives away.
fn terms_from_doc(s: &Doc, origin: &str, terms: &mut IndexMap<String, String>) {
    let p = patterns();
    for a in arr(s.get("identity").and_then(|i| i.get("accounts"))) {
        let workspace = a.get("workspace");
        for v in [a.get("principal"), workspace] {
            for piece in p.piece_split.split(&text_of(v)) {
                let piece = js_trim(piece);
                // `piece.trim().split(/\s+/)`: the piece is trimmed, so only runs inside it
                // leave empty pieces behind.
                for tok in piece.split(is_js_space).filter(|t| !t.is_empty()) {
                    if tok.contains(['@', '.'])
                        || p.upper_token.is_match(tok)
                        || tok.starts_with("u/")
                    {
                        add_term(terms, &tok.replace(['(', ')', ','], ""), origin);
                    }
                }
                if is_truthy(workspace) && workspace.and_then(Value::as_str) == Some(piece) {
                    add_term(terms, piece, origin);
                }
            }
        }
    }
    let lists: Vec<&Doc> = match s.get("channels") {
        Some(Value::Object(o)) => o.values().collect(),
        Some(Value::Array(a)) => a.iter().collect(),
        _ => Vec::new(),
    };
    for list in lists {
        let items: Vec<&Doc> = match list {
            Value::Array(a) => a.iter().collect(),
            other if is_truthy(Some(other)) => vec![other],
            _ => Vec::new(),
        };
        for c in items {
            for v in [c.get("handle"), c.get("sender")] {
                if is_truthy(v) {
                    add_term(terms, &js_display(v), origin);
                }
            }
            let id = js_display(c.get("id"));
            if p.chat_whole.is_match(&id) {
                add_term(terms, &id, origin);
            }
        }
    }
    let text = serde_json::to_string(s).unwrap_or_default();
    for c in p.home.captures_iter(&text) {
        add_term(terms, &c[1], origin);
    }
}

/// Terms derived from every `instance/<name>/` (denylist, account principals, handles, home users) -> origin.
pub fn instance_terms(instance_root: &Path) -> IndexMap<String, String> {
    let mut terms = IndexMap::new();
    if !instance_root.exists() {
        return terms;
    }
    for entry in sorted_entries(instance_root) {
        let dir = instance_root.join(&entry);
        if !dir.is_dir() {
            continue;
        }
        let name = entry.to_string_lossy();
        if let Some(text) = read_lossy(&dir.join("lint-denylist.txt")) {
            let origin = format!("{name}/lint-denylist.txt");
            for line in text.split('\n') {
                add_term(&mut terms, &strip_comment(line), &origin);
            }
        }
        for file in ["setup.yaml", "overrides.yaml"] {
            let f = dir.join(file);
            if !f.exists() {
                continue;
            }
            let Ok(s) = read_yaml(&f) else { continue };
            terms_from_doc(&s, &format!("{name}/{file}"), &mut terms);
        }
    }
    terms
}

/// Phrases allowed through, from every `instance/<name>/lint-allow.txt`.
pub fn instance_allow(instance_root: &Path) -> Vec<String> {
    let mut allow = Vec::new();
    if !instance_root.exists() {
        return allow;
    }
    for entry in sorted_entries(instance_root) {
        let Some(text) = read_lossy(&instance_root.join(&entry).join("lint-allow.txt")) else {
            continue;
        };
        for line in text.split('\n') {
            let t = js_trim(&strip_comment(line)).to_string();
            if !t.is_empty() {
                allow.push(t);
            }
        }
    }
    allow
}

/// Everything under `root` except target/, node_modules/, out/, .git/, instance/, .superpowers/, .worktrees/.
pub fn default_targets(root: &Path) -> Vec<PathBuf> {
    sorted_entries(root)
        .into_iter()
        .filter(|n| !n.to_str().is_some_and(|n| DEFAULT_EXCLUDE.contains(&n)))
        .map(|n| root.join(n))
        .collect()
}

/// A scanned file name: a listed extension, no dot at all (`LICENSE`, extensionless scripts),
/// or `.gitignore`. Other dotfiles (`.env`, `.DS_Store`) are local, not shared, and skipped.
fn is_text_name(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    if name == ".gitignore" || !name.contains('.') {
        return true;
    }
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| TEXT_EXT.contains(&ext))
}

/// Text files under `p` (or `p` itself), following symlinks, skipping `SKIP_DIRS`.
fn walk(p: &Path, out: &mut Vec<PathBuf>) {
    if !p.exists() {
        return;
    }
    let entries = WalkDir::new(p)
        .follow_links(true)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            !(e.file_type().is_dir()
                && e.file_name()
                    .to_str()
                    .is_some_and(|n| SKIP_DIRS.contains(&n)))
        });
    for e in entries.flatten() {
        if e.file_type().is_file()
            && is_text_name(e.file_name())
            && e.metadata().map(|m| m.len() < MAX_BYTES).unwrap_or(false)
        {
            out.push(e.into_path());
        }
    }
}

/// `path.relative(from, to)` for two absolute, normalized paths, with `/` separators.
fn relative(from: &Path, to: &Path) -> String {
    let f: Vec<_> = from.components().collect();
    let t: Vec<_> = to.components().collect();
    let common = f.iter().zip(&t).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_string(); f.len() - common];
    parts.extend(
        t[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

/// `\d{n}` at `at`.
fn digits_at(cs: &[char], at: usize, n: usize) -> bool {
    (at..at + n).all(|k| cs.get(k).is_some_and(char::is_ascii_digit))
}

/// `[\s.-]` at `at`.
fn sep_at(cs: &[char], at: usize) -> bool {
    cs.get(at)
        .is_some_and(|&c| is_js_space(c) || c == '.' || c == '-')
}

/// `\(?\d{3}\)?[\s.-]\d{3}[\s.-]\d{4}` anchored at `i`; returns where it ends. The optional
/// parentheses never need backtracking: without them the next token cannot match.
fn phone_body(cs: &[char], i: usize) -> Option<usize> {
    let mut j = i;
    if cs.get(j) == Some(&'(') {
        j += 1;
    }
    if !digits_at(cs, j, 3) {
        return None;
    }
    j += 3;
    if cs.get(j) == Some(&')') {
        j += 1;
    }
    let ok =
        sep_at(cs, j) && digits_at(cs, j + 1, 3) && sep_at(cs, j + 4) && digits_at(cs, j + 5, 4);
    ok.then_some(j + 9)
}

/// The reference PHONE_RE anchored at `i`, alternatives in backtracking order, with the
/// `(?![\w-])` lookahead checked at the end.
fn phone_at(cs: &[char], i: usize) -> Option<usize> {
    let end_ok = |e: usize| cs.get(e).is_none_or(|&c| !(is_word(c) || c == '-'));
    // Optional `\+?\d{1,2}[\s.-]` prefix: two digits, then one.
    let j = if cs.get(i) == Some(&'+') { i + 1 } else { i };
    for n in [2, 1] {
        if digits_at(cs, j, n) && sep_at(cs, j + n) {
            if let Some(e) = phone_body(cs, j + n + 1).filter(|&e| end_ok(e)) {
                return Some(e);
            }
        }
    }
    phone_body(cs, i).filter(|&e| end_ok(e))
}

/// Matches of the reference
/// `(?<![\w.-])(?:\+?\d{1,2}[\s.-])?\(?\d{3}\)?[\s.-]\d{3}[\s.-]\d{4}(?![\w-])`, with the
/// lookbehind and lookahead checked by hand (the `regex` crate has no look-around).
fn find_phones(line: &str) -> Vec<String> {
    let cs: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        let before_ok = i == 0 || !(is_word(cs[i - 1]) || cs[i - 1] == '.' || cs[i - 1] == '-');
        if before_ok {
            if let Some(e) = phone_at(&cs, i) {
                out.push(cs[i..e].iter().collect());
                i = e;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// True when the term occurs with no ASCII letter or digit on either side
/// (`(?<![A-Za-z0-9])term(?![A-Za-z0-9])`, case-insensitive), trying every start position.
fn term_hit(re: &Regex, line: &str) -> bool {
    let mut start = 0;
    while start <= line.len() {
        let Some(m) = re.find_at(line, start) else {
            return false;
        };
        let before_ok = line[..m.start()]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        let after_ok = line[m.end()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        if before_ok && after_ok {
            return true;
        }
        start = m.start() + line[m.start()..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

fn literal_ci(s: &str) -> Option<Regex> {
    Regex::new(&format!("(?i){}", regex::escape(s))).ok()
}

fn scan_line(
    line: &str,
    at: &str,
    terms: &[(&String, &String, Regex)],
    out: &mut Vec<LintFinding>,
) {
    let p = patterns();
    let mut push = |kind: &str, detail: String| {
        out.push(LintFinding {
            at: at.to_string(),
            kind: kind.to_string(),
            detail,
        })
    };
    for s in find_secrets(line) {
        push("secret", s.id.to_string());
    }
    for m in p.email.find_iter(line) {
        let e = m.as_str();
        if !p.safe_email.is_match(e) && !p.file_like.is_match(e) {
            push("email", e.to_string());
        }
    }
    for ph in find_phones(line) {
        push("phone", ph);
    }
    for c in p.home.captures_iter(line) {
        if !SAFE_HOME_USERS.contains(&c[1].to_lowercase().as_str()) {
            push("home-path", c[0].to_string());
        }
    }
    for m in p.chat.find_iter(line) {
        if m.as_str().bytes().filter(u8::is_ascii_digit).count() >= 2 {
            push("chat-id", m.as_str().to_string());
        }
    }
    for (t, origin, re) in terms {
        if term_hit(re, line) {
            push("instance-term", format!("\"{t}\" (from {origin})"));
        }
    }
}

/// Scan the targets.
pub fn lint(
    root: &Path,
    targets: &[PathBuf],
    terms: &IndexMap<String, String>,
    allow: &[String],
) -> LintReport {
    let allow_res: Vec<Regex> = allow.iter().filter_map(|a| literal_ci(a)).collect();
    let term_res: Vec<(&String, &String, Regex)> = terms
        .iter()
        .filter_map(|(t, o)| literal_ci(t).map(|re| (t, o, re)))
        .collect();
    let mut files = Vec::new();
    for t in targets {
        walk(&root.join(t), &mut files);
    }
    let root_abs = resolve(root);
    let mut findings = Vec::new();
    for f in &files {
        let rel = relative(&root_abs, &resolve(f));
        let Ok(bytes) = std::fs::read(f) else {
            continue;
        };
        // Lossy like the reference's `readFileSync(f, 'utf8')`: NUL bytes and invalid UTF-8
        // never stop a file from being scanned.
        let text = String::from_utf8_lossy(&bytes);
        for (i, raw) in text.split('\n').enumerate() {
            let at = format!("{rel}:{}", i + 1);
            let mut line = raw.to_string();
            for re in &allow_res {
                line = re.replace_all(&line, NoExpand(" ")).into_owned();
            }
            scan_line(&line, &at, &term_res, &mut findings);
        }
    }
    LintReport {
        files: files.len(),
        terms: terms.len(),
        allowed: allow.to_vec(),
        findings,
    }
}

pub fn format_lint(r: &LintReport) -> String {
    let mut lines = vec![format!(
        "  scanned {} file(s) against {} instance-derived term(s) + generic secret/PII detectors",
        r.files, r.terms
    )];
    if !r.allowed.is_empty() {
        lines.push(format!(
            "  allowlisted phrase(s) from instance lint-allow.txt: {}",
            r.allowed.join(", ")
        ));
    }
    for f in r.findings.iter().take(LISTED) {
        lines.push(format!("  {:<14} {}  {}", f.kind, f.at, f.detail));
    }
    if r.findings.len() > LISTED {
        lines.push(format!("  … {} more", r.findings.len() - LISTED));
    }
    lines.push(if r.findings.is_empty() {
        "  LINT PASSED: 0 findings".to_string()
    } else {
        format!("  LINT FAILED: {} finding(s)", r.findings.len())
    });
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // Canary values are assembled at run time so this file itself passes the lint.
    fn email() -> String {
        format!("{}@{}", "someone", "mailbox-provider.net")
    }
    fn phone() -> String {
        ["555", "123", "4567"].join("-")
    }
    fn home_path() -> String {
        format!("/{}/someone/projects", "Users")
    }
    fn chat_id() -> String {
        format!("C{}", "0123456789")
    }
    fn token() -> String {
        format!("{}{}", "xox", "b-1234567890-abcdefghijklmnop")
    }

    fn write(p: &Path, text: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    fn kinds(r: &LintReport) -> Vec<(String, String, String)> {
        r.findings
            .iter()
            .map(|f| (f.kind.clone(), f.at.clone(), f.detail.clone()))
            .collect()
    }

    #[test]
    fn lint_catches_planted_canaries() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        write(
            &root.join("instance/x/lint-denylist.txt"),
            "# terms\nzorblax   # the owner's codename\n\n",
        );
        let body = format!(
            "clean line\ncontact {}\ncall {} today\nsee {}\nchannel {} here\nbot {}\nThe Zorblax plan\n",
            email(),
            phone(),
            home_path(),
            chat_id(),
            token()
        );
        write(&root.join("docs/notes.md"), &body);
        let iroot = root.join("instance");
        let terms = instance_terms(&iroot);
        assert_eq!(
            terms.get("zorblax").map(|s| s.as_str()),
            Some("x/lint-denylist.txt")
        );
        let r = lint(
            root,
            &default_targets(root),
            &terms,
            &instance_allow(&iroot),
        );
        assert_eq!(r.files, 1, "the instance folder is not a default target");
        assert_eq!(r.terms, 1);
        let got = kinds(&r);
        let want = vec![
            ("email".to_string(), "docs/notes.md:2".to_string(), email()),
            ("phone".to_string(), "docs/notes.md:3".to_string(), phone()),
            (
                "home-path".to_string(),
                "docs/notes.md:4".to_string(),
                format!("/{}/someone", "Users"),
            ),
            (
                "chat-id".to_string(),
                "docs/notes.md:5".to_string(),
                chat_id(),
            ),
            (
                "secret".to_string(),
                "docs/notes.md:6".to_string(),
                "slack-token".to_string(),
            ),
            (
                "instance-term".to_string(),
                "docs/notes.md:7".to_string(),
                "\"zorblax\" (from x/lint-denylist.txt)".to_string(),
            ),
        ];
        assert_eq!(got, want);
        let text = format_lint(&r);
        assert!(text.starts_with("  scanned 1 file(s) against 1 instance-derived term(s) + generic secret/PII detectors\n"));
        assert!(text.contains(&format!(
            "\n  {:<14} {}  {}\n",
            "email",
            "docs/notes.md:2",
            email()
        )));
        assert!(text.ends_with("\n  LINT FAILED: 6 finding(s)"));
    }

    #[test]
    fn lint_ignores_safe_values_and_allowlisted_phrases() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        write(&root.join("instance/x/lint-denylist.txt"), "zorblax\n");
        write(
            &root.join("instance/x/lint-allow.txt"),
            "zorblax-labs  # public folder name\n",
        );
        write(
            &root.join("src/a.rs"),
            "// me@example.com and alex@users.noreply.github.com and noreply@anthropic.com\n\
             // see helper@util.mjs and /Users/alex/code and /home/runner/work\n\
             // ZORBLAXIAN and prezorblax are other words; ZORBLAX-LABS is allowed\n\
             // dates 2026-10-15, versions v1.555-123-4567, ids CABCDEFGH1 and C12\n",
        );
        let iroot = root.join("instance");
        let allow = instance_allow(&iroot);
        assert_eq!(allow, vec!["zorblax-labs".to_string()]);
        let r = lint(
            root,
            &default_targets(root),
            &instance_terms(&iroot),
            &allow,
        );
        assert_eq!(r.files, 1);
        assert!(r.findings.is_empty(), "{:?}", kinds(&r));
        let text = format_lint(&r);
        assert_eq!(
            text,
            "  scanned 1 file(s) against 1 instance-derived term(s) + generic secret/PII detectors\n  allowlisted phrase(s) from instance lint-allow.txt: zorblax-labs\n  LINT PASSED: 0 findings"
        );
    }

    #[test]
    fn lint_terms_match_whole_words_case_insensitively() {
        let terms: IndexMap<String, String> = [("zorblax", "t"), ("ab-ab", "t")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        write(
            &root.join("a.txt"),
            "ZORBLAX.\nx_zorblax_y\nzorblax2\nxab-ab-ab\nab-abc\n",
        );
        let r = lint(root, &[root.join("a.txt")], &terms, &[]);
        let at: Vec<String> = r
            .findings
            .iter()
            .map(|f| format!("{} {}", f.at, f.detail))
            .collect();
        assert_eq!(
            at,
            vec![
                "a.txt:1 \"zorblax\" (from t)",
                "a.txt:2 \"zorblax\" (from t)",
                "a.txt:4 \"ab-ab\" (from t)",
            ]
        );
    }

    #[test]
    fn lint_phone_shapes_follow_the_reference() {
        let hits = |s: &str| find_phones(s);
        let p = |a: &str, b: &str, c: &str, sep: &str| [a, b, c].join(sep);
        assert_eq!(
            hits(&format!("call {} now", p("555", "123", "4567", "-"))),
            vec![p("555", "123", "4567", "-")]
        );
        let paren = format!("({}) {}-{}", "555", "123", "4567");
        assert_eq!(hits(&format!("tel:{paren};")), vec![paren]);
        assert_eq!(
            hits(&format!("+1 {}", p("555", "123", "4567", " "))),
            vec![format!("+1 {}", p("555", "123", "4567", " "))]
        );
        assert_eq!(
            hits(&format!(
                "{},{}",
                p("555", "123", "4567", "."),
                p("555", "123", "4568", ".")
            )),
            vec![p("555", "123", "4567", "."), p("555", "123", "4568", ".")]
        );
        // Preceded by a word character, a dot or a dash, or followed by one: not a phone.
        assert!(hits(&format!("v1.{}", p("555", "123", "4567", "-"))).is_empty());
        assert!(hits(&format!("x{}", p("555", "123", "4567", "-"))).is_empty());
        assert!(hits(&format!("{}8", p("555", "123", "4567", "-"))).is_empty());
        assert!(hits(&format!("{}-x", p("555", "123", "4567", "-"))).is_empty());
        assert!(hits("5551234567 and 2026-10-15").is_empty());
        // A short country/area prefix belongs to the match; a trailing period is fine.
        assert_eq!(
            hits(&format!("12 {}.", p("555", "123", "4567", " "))),
            vec![format!("12 {}", p("555", "123", "4567", " "))]
        );
        let doubled = format!(
            "{} {}",
            p("555", "123", "4567", "-"),
            p("555", "123", "4568", "-")
        );
        assert_eq!(
            hits(&doubled),
            vec![p("555", "123", "4567", "-"), p("555", "123", "4568", "-")]
        );
    }

    #[test]
    fn lint_default_targets_skip_build_and_private_dirs() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        for d in [
            "target",
            "node_modules",
            "out",
            ".git",
            "instance",
            ".superpowers",
            ".worktrees",
            "src",
            "docs",
        ] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        write(&root.join("README.md"), "x");
        let got: Vec<String> = default_targets(root)
            .iter()
            .map(|p| p.strip_prefix(root).unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(got, vec!["README.md", "docs", "src"]);
    }

    #[test]
    fn lint_walks_text_files_only() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let canary = format!("mail {}\n", email());
        for f in [
            "src/a.rs",
            "LICENSE",
            ".gitignore",
            "x/b.yml",
            "x/c.py",
            "x/d.toml",
            "x/e.sh",
            "x/f.json",
        ] {
            write(&root.join(f), &canary);
        }
        for f in [
            "x/g.bin",
            "x/h.lock",
            "x/i.MD",
            "x/j.cjs",
            "x/.env",
            "x/node_modules/j.md",
            "x/out/k.md",
            "x/target/l.md",
            "x/.git/m.md",
        ] {
            write(&root.join(f), &canary);
        }
        write(&root.join("x/big.md"), &"a".repeat(5_000_000));
        let r = lint(root, &[root.to_path_buf()], &IndexMap::new(), &[]);
        let mut at: Vec<String> = r.findings.iter().map(|f| f.at.clone()).collect();
        at.sort();
        assert_eq!(
            at,
            vec![
                ".gitignore:1",
                "LICENSE:1",
                "src/a.rs:1",
                "x/b.yml:1",
                "x/c.py:1",
                "x/d.toml:1",
                "x/e.sh:1",
                "x/f.json:1"
            ]
        );
        assert_eq!(r.files, 8);
        // A relative target resolves against the root; a missing one is skipped.
        let r2 = lint(
            root,
            &[PathBuf::from("src"), PathBuf::from("nope")],
            &IndexMap::new(),
            &[],
        );
        assert_eq!(r2.files, 1);
        assert_eq!(r2.findings[0].at, "src/a.rs:1");
    }

    #[test]
    fn lint_scans_nul_bytes_and_invalid_utf8_lossily() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        let mut bytes = b"\0\0binary\0".to_vec();
        bytes.extend_from_slice(format!(" mail {}\n", email()).as_bytes());
        bytes.extend_from_slice(b"caf\xe9 \xff\xfe ");
        bytes.extend_from_slice(format!("{}\n", phone()).as_bytes());
        fs::write(root.join("blob"), &bytes).unwrap();
        let r = lint(root, &[root.join("blob")], &IndexMap::new(), &[]);
        assert_eq!(r.files, 1);
        assert_eq!(
            kinds(&r),
            vec![
                ("email".to_string(), "blob:1".to_string(), email()),
                ("phone".to_string(), "blob:2".to_string(), phone()),
            ]
        );
    }

    #[test]
    fn lint_principal_tokens_split_on_javascript_whitespace() {
        // U+FEFF separates tokens in JavaScript `\s`; U+0085 does not.
        let doc = serde_json::json!({ "identity": { "accounts": [
            { "id": "a", "principal": "alex@example.com\u{feff}bob.k x.y\u{85}z.w" }
        ] } });
        let mut terms = IndexMap::new();
        terms_from_doc(&doc, "o", &mut terms);
        let got: Vec<&str> = terms.keys().map(|k| k.as_str()).collect();
        assert_eq!(got, vec!["alex@example.com", "bob.k", "x.y\u{85}z.w"]);
    }

    #[cfg(unix)]
    #[test]
    fn lint_follows_symlinks_without_looping() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path();
        write(&root.join("real/a.md"), &format!("mail {}\n", email()));
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("real/loop")).unwrap();
        let r = lint(root, &default_targets(root), &IndexMap::new(), &[]);
        let at: Vec<&str> = r.findings.iter().map(|f| f.at.as_str()).collect();
        assert_eq!(at, vec!["link/a.md:1", "real/a.md:1"]);
    }

    #[test]
    fn lint_instance_terms_follow_the_extraction_rules() {
        let t = tempfile::tempdir().unwrap();
        let iroot = t.path().join("instance");
        let setup = format!(
            "version: 1\nmeta: {{ instance: x }}\nidentity:\n  accounts:\n\
             \x20   - {{ id: a, kind: k, access: unknown, principal: \"alex@example.com, alex.k + U0ABCDEFGH and u/alexk\", workspace: Acme Workspace }}\n\
             \x20   - {{ id: b, kind: k, access: unknown, principal: \"(alexbot.dev)\", workspace: \"n/a\" }}\n\
             \x20   - {{ id: c, kind: k, access: unknown, principal: plain words only }}\n\
             channels:\n  slack:\n    - {{ id: {}, handle: \"@alexk\", sender: ab }}\n  email:\n    - {{ id: CSHORT1, sender: \"Alex K\" }}\n\
             loops:\n  - {{ id: l, runner: manual, enabled: false, working_dir: \"{}\" }}\n",
            chat_id(),
            home_path()
        );
        write(&iroot.join("x/setup.yaml"), &setup);
        write(
            &iroot.join("x/overrides.yaml"),
            &format!(
                "data: {{state: [{{id: s, path: \"/{}/alexk/notes\", kind: f}}]}}\n",
                "home"
            ),
        );
        write(
            &iroot.join("x/lint-denylist.txt"),
            "Zorblax\nnone\nab\n  # only a comment\nzorblax\n",
        );
        write(&iroot.join("y/setup.yaml"), "not: [valid\n");
        write(&iroot.join("stray.txt"), "ignored");
        let terms = instance_terms(&iroot);
        let got: Vec<(String, String)> =
            terms.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            got,
            vec![
                s("zorblax", "x/lint-denylist.txt"),
                s("alex@example.com", "x/setup.yaml"),
                s("alex.k", "x/setup.yaml"),
                s("u0abcdefgh", "x/setup.yaml"),
                s("u/alexk", "x/setup.yaml"),
                s("acme workspace", "x/setup.yaml"),
                s("alexbot.dev", "x/setup.yaml"),
                s("@alexk", "x/setup.yaml"),
                s(&chat_id().to_lowercase(), "x/setup.yaml"),
                s("alex k", "x/setup.yaml"),
                s("someone", "x/setup.yaml"),
                s("alexk", "x/overrides.yaml"),
            ]
        );
        assert!(instance_terms(&t.path().join("missing")).is_empty());
        assert!(instance_allow(&t.path().join("missing")).is_empty());
    }

    #[test]
    fn lint_format_caps_the_listing_at_200() {
        let r = LintReport {
            files: 3,
            terms: 0,
            allowed: vec![],
            findings: (0..205)
                .map(|i| LintFinding {
                    at: format!("a.md:{i}"),
                    kind: "secret".into(),
                    detail: "jwt".into(),
                })
                .collect(),
        };
        let text = format_lint(&r);
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 1 + 200 + 2);
        assert_eq!(
            lines[1],
            format!("  {:<14} {}  {}", "secret", "a.md:0", "jwt")
        );
        assert_eq!(lines[201], "  … 5 more");
        assert_eq!(lines[202], "  LINT FAILED: 205 finding(s)");
    }
}
