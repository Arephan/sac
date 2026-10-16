//! Turning free-text instructions into structured rules: scope, severity, date. Ported from
//! the reference `framework/lib/rules.mjs`. The keyword table is generic; an instance extends
//! it through `import.options.<importer>.scope_keywords: { scope: [words...] }`.
//!
//! Also home to the small JavaScript-compatibility helpers the Claude importers share, so that
//! ids, truthiness and string conversion match the Node reference exactly.

use crate::ctx::Ctx;
use crate::util::is_truthy;
use crate::Doc;
use serde_json::Value;

/// Compile a regex once, on first use, at the call site (so it is cheap inside loops).
macro_rules! static_re {
    ($pat:expr) => {{
        static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        RE.get_or_init(|| $crate::importers::rules::compile_static($pat))
    }};
}
pub(crate) use static_re;

/// Compile one of this module's fixed patterns; they are all tested, so failure is a bug.
pub(crate) fn compile_static(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("static regex compiles")
}

/// The generic scope keyword table, in the reference's order (earlier scopes win ties).
pub const DEFAULT_SCOPE_KEYWORDS: &[(&str, &[&str])] = &[
    (
        "money",
        &[
            "spend",
            "purchase",
            "pay",
            "payment",
            "subscription",
            "subscribe",
            "billing",
            "buy",
            "refund",
            "price",
            "card",
        ],
    ),
    ("email", &["email", "gmail", "inbox", "mail"]),
    (
        "chat",
        &[
            "slack", "discord", "teams", "whatsapp", "telegram", "reply", "thread",
        ],
    ),
    (
        "social",
        &[
            "reddit",
            "twitter",
            " x ",
            "tiktok",
            "instagram",
            "threads",
            "youtube",
            "shorts",
            "reels",
            "post",
            "tweet",
            "linkedin",
        ],
    ),
    (
        "press",
        &["press", "journalist", "reporter", "pitch", "media outlet"],
    ),
    (
        "apps",
        &[
            "app store",
            "testflight",
            "play store",
            "review",
            "build",
            "release",
            "listing",
            "aso",
            "paywall",
        ],
    ),
    (
        "jobs",
        &["job", "resume", "recruiter", "interview", "salary", "apply"],
    ),
    (
        "contracts",
        &["bid", "tender", "contract", "rfp", "procurement"],
    ),
    (
        "calendar",
        &[
            "calendar",
            "appointment",
            "booking",
            "schedule",
            "custody",
            "event",
        ],
    ),
    ("browser", &["chrome", "browser", "playwright", "cdp"]),
    (
        "privacy",
        &[
            "phone",
            "address",
            "pii",
            "customer name",
            "identity",
            "separation",
        ],
    ),
    (
        "code",
        &[
            "git",
            "repo",
            "commit",
            "deploy",
            "test",
            "simulator",
            "backend",
            "component",
        ],
    ),
    ("loops", &["loop", "launchd", "cron", "daemon", "every run"]),
    ("design", &["design", "ui", "icon", "mockup", "screenshot"]),
];

/// Best-matching scope for a rule text, by keyword hits; `global` when nothing matches.
/// Each keyword counts once when it occurs anywhere in ` text ` (lowercased, padded with a
/// space on each side); the scope with the most hits wins and ties go to the earlier scope.
/// `extra` (`{ scope: [words...] }`) appends words to a scope or adds a new scope at the end.
pub fn classify_scope(text: &str, extra: Option<&Doc>) -> String {
    let t = format!(" {} ", text.to_lowercase());
    let mut table: Vec<(String, Vec<String>)> = DEFAULT_SCOPE_KEYWORDS
        .iter()
        .map(|(scope, words)| {
            (
                (*scope).to_string(),
                words.iter().map(|w| (*w).to_string()).collect(),
            )
        })
        .collect();
    if let Some(Value::Object(extra)) = extra {
        for (scope, words) in extra {
            let added: Vec<String> = match words {
                Value::Array(a) => a.iter().map(|w| js_string(w).to_lowercase()).collect(),
                Value::Null => Vec::new(),
                other => vec![js_string(other).to_lowercase()],
            };
            match table.iter_mut().find(|(s, _)| s == scope) {
                Some((_, list)) => list.extend(added),
                None => table.push((scope.clone(), added)),
            }
        }
    }
    // Every word in the table is lowercase by now (the defaults are written that way).
    let mut best = "global";
    let mut best_n = 0;
    for (scope, words) in &table {
        let n = words.iter().filter(|w| t.contains(w.as_str())).count();
        if n > best_n {
            best = scope;
            best_n = n;
        }
    }
    best.to_string()
}

/// `hard` when the text contains the uppercase word HARD, else `standing`.
pub fn severity_of(text: &str) -> &'static str {
    // `(?-u:\b)` is an ASCII word boundary, which is what JavaScript's `\b` is.
    if static_re!(r"(?-u:\b)HARD(?-u:\b)").is_match(text) {
        "hard"
    } else {
        "standing"
    }
}

/// The first `20YY-MM-DD` in any of the texts.
pub fn first_date(texts: &[&str]) -> Option<String> {
    let re = static_re!(r"(?-u:\b)(20[0-9][0-9])-([0-9][0-9])-([0-9][0-9])(?-u:\b)");
    texts
        .iter()
        .find_map(|t| re.captures(t))
        .map(|c| format!("{}-{}-{}", &c[1], &c[2], &c[3]))
}

/// JavaScript's `String(v)` for a value read from JSON or YAML.
pub(crate) fn js_string(v: &Doc) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                n.to_string()
            } else {
                let f = n.as_f64().unwrap_or(0.0);
                if f == 0.0 {
                    "0".to_string()
                } else if f.fract() == 0.0 && f.abs() < 1e21 {
                    format!("{f:.0}")
                } else {
                    f.to_string()
                }
            }
        }
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
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// JavaScript's `s.slice(0, max)`: the first `max` UTF-16 code units, without splitting a
/// character in two.
pub(crate) fn js_slice(s: &str, max: usize) -> &str {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > max {
            return &s[..i];
        }
    }
    s
}

/// Node's `path.normalize` for an absolute POSIX path: empty and `.` segments dropped, `..`
/// removes the previous segment, a trailing slash kept.
fn normalize_abs(p: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    let mut out = format!("/{}", segments.join("/"));
    if p.ends_with('/') && out != "/" {
        out.push('/');
    }
    out
}

/// The reference's `ctx.p`: `~/x` is expanded with Node's `path.join(home, x)`, which also
/// normalizes `.` and `..` segments; everything else expands as `Ctx::p` does.
pub(crate) fn js_p(ctx: &Ctx, rel: &str) -> String {
    let home = ctx.home().to_string_lossy();
    match rel.strip_prefix("~/") {
        // `path.join` skips an empty argument, so `~/` is the home itself.
        Some("") => normalize_abs(&home),
        Some(rest) => normalize_abs(&format!("{home}/{rest}")),
        None => ctx.p(rel).to_string_lossy().to_string(),
    }
}

/// `js_p` shown with the home as `~`, as manifests store paths.
pub(crate) fn js_shown(ctx: &Ctx, rel: &str) -> String {
    ctx.tilde(&js_p(ctx, rel))
}

/// `Some(v)` only when `v` is present and truthy (JavaScript's `x || fallback` test).
pub(crate) fn truthy(v: Option<&Doc>) -> Option<&Doc> {
    v.filter(|x| is_truthy(Some(x)))
}

/// `Some(v)` when `v` is present and not null (JavaScript's `x ?? fallback` test).
pub(crate) fn present(v: Option<&Doc>) -> Option<&Doc> {
    v.filter(|x| !x.is_null())
}

/// `Object.entries(v || {})`: the entries of an object in document order, or the
/// index/item pairs of an array; anything else has none.
pub(crate) fn entries(v: Option<&Doc>) -> Vec<(String, &Doc)> {
    match v {
        Some(Value::Object(o)) => o.iter().map(|(k, x)| (k.clone(), x)).collect(),
        Some(Value::Array(a)) => a
            .iter()
            .enumerate()
            .map(|(i, x)| (i.to_string(), x))
            .collect(),
        _ => Vec::new(),
    }
}

/// `Object.keys(v || {})`: object keys in document order, array indices, or the UTF-16
/// indices of a string; anything else has none.
pub(crate) fn key_list(v: Option<&Doc>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => (0..s.encode_utf16().count())
            .map(|i| i.to_string())
            .collect(),
        _ => entries(v).into_iter().map(|(k, _)| k).collect(),
    }
}

/// A JavaScript array index ("0", "17", never "07"): such object keys enumerate first.
fn array_index(k: &str) -> Option<u64> {
    let b = k.as_bytes();
    if b.is_empty() || b.len() > 10 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    if b.len() > 1 && b[0] == b'0' {
        return None;
    }
    k.parse::<u64>().ok().filter(|n| *n < u64::from(u32::MAX))
}

/// Reorder every object the way a JavaScript engine enumerates it: array-index keys first in
/// ascending numeric order, then the other keys in document order. Applied to parsed files so
/// iteration, duplicate-name resolution and output order match the reference.
pub(crate) fn js_order(mut v: Doc) -> Doc {
    fn walk(v: &mut Doc) {
        match v {
            Value::Array(a) => a.iter_mut().for_each(walk),
            Value::Object(o) => {
                o.values_mut().for_each(walk);
                if o.keys().any(|k| array_index(k).is_some()) {
                    let mut items: Vec<(String, Doc)> = std::mem::take(o).into_iter().collect();
                    // stable, so the non-index keys keep their document order
                    items.sort_by_key(|(k, _)| array_index(k).map_or((1, 0), |n| (0, n)));
                    *o = items.into_iter().collect();
                }
            }
            _ => {}
        }
    }
    walk(&mut v);
    v
}

/// A throwaway home directory for importer tests. Nothing here touches the real home.
#[cfg(test)]
pub(crate) mod testing {
    use crate::ctx::Ctx;
    use serde_json::json;
    use std::path::PathBuf;

    pub struct Home {
        _dir: tempfile::TempDir,
        root: PathBuf,
    }

    /// A fresh, empty, canonical temporary home.
    pub fn home() -> Home {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().canonicalize().expect("canonical tempdir");
        Home { _dir: dir, root }
    }

    impl Home {
        /// The home directory as a string.
        pub fn s(&self) -> String {
            self.root.to_string_lossy().to_string()
        }
        /// Write `text` to `rel` (relative to the home), creating parent directories.
        /// Every `@HOME@` in `text` becomes the home path.
        pub fn write(&self, rel: &str, text: &str) {
            let p = self.root.join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            std::fs::write(p, text.replace("@HOME@", &self.s())).expect("write");
        }
        /// Create an empty directory `rel` under the home.
        pub fn mkdir(&self, rel: &str) {
            std::fs::create_dir_all(self.root.join(rel)).expect("mkdir");
        }
        /// A context that reads this home and nothing else, with `options`.
        pub fn ctx_with(&self, options: crate::Doc) -> Ctx {
            Ctx::new(Some(&self.root), options)
        }
        pub fn ctx(&self) -> Ctx {
            self.ctx_with(json!({}))
        }
    }

    /// The keys of an object in document order (`==` on documents ignores key order).
    pub fn keys(v: &crate::Doc) -> Vec<&str> {
        v.as_object()
            .map(|o| o.keys().map(|k| k.as_str()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classify_scope_picks_the_scope_with_most_keyword_hits() {
        assert_eq!(
            classify_scope("Never spend money on a purchase", None),
            "money"
        );
        assert_eq!(classify_scope("nothing to see here", None), "global");
        // a tie keeps the scope listed first in the table (email before chat)
        assert_eq!(classify_scope("inbox or slack", None), "email");
        // " x " only matches as a whole word thanks to the padding
        assert_eq!(classify_scope("x", None), "social");
        assert_eq!(classify_scope("xenon", None), "global");
        // keywords match inside words too
        assert_eq!(classify_scope("xylophone", None), "privacy");
        // case-insensitive
        assert_eq!(classify_scope("GIT COMMIT", None), "code");
    }

    #[test]
    fn classify_scope_takes_extra_keywords_from_options() {
        let extra = json!({ "pets": ["Cat", "dog"], "money": ["coin"] });
        assert_eq!(
            classify_scope("feed the cat and the dog", Some(&extra)),
            "pets"
        );
        assert_eq!(classify_scope("a coin", Some(&extra)), "money");
        assert_eq!(classify_scope("a coin", None), "global");
        assert_eq!(classify_scope("a coin", Some(&json!(null))), "global");
    }

    #[test]
    fn severity_is_hard_only_for_the_uppercase_word() {
        assert_eq!(severity_of("MONEY HARD RULE"), "hard");
        assert_eq!(severity_of("(HARD)"), "hard");
        assert_eq!(severity_of("HARD"), "hard");
        assert_eq!(severity_of("hard rule"), "standing");
        assert_eq!(severity_of("HARDWARE"), "standing");
        assert_eq!(severity_of("NOHARD"), "standing");
        // the word boundary is ASCII, as in the reference
        assert_eq!(severity_of("éHARD"), "hard");
    }

    #[test]
    fn first_date_finds_the_first_dated_text() {
        assert_eq!(
            first_date(&["no date", "since 2026-09-02, then 2026-10-01"]),
            Some("2026-09-02".to_string())
        );
        assert_eq!(
            first_date(&["(2025-01-31)", "2026-01-01"]),
            Some("2025-01-31".to_string())
        );
        assert_eq!(first_date(&["x2026-01-01", "2026-10-15T08:00"]), None);
        assert_eq!(first_date(&["1999-01-01", ""]), None);
        assert_eq!(first_date(&[]), None);
    }

    #[test]
    fn default_keywords_are_lowercase_and_in_reference_order() {
        for (_, words) in DEFAULT_SCOPE_KEYWORDS {
            for w in *words {
                assert_eq!(*w, w.to_lowercase());
            }
        }
        let scopes: Vec<&str> = DEFAULT_SCOPE_KEYWORDS.iter().map(|(s, _)| *s).collect();
        assert_eq!(
            scopes,
            [
                "money",
                "email",
                "chat",
                "social",
                "press",
                "apps",
                "jobs",
                "contracts",
                "calendar",
                "browser",
                "privacy",
                "code",
                "loops",
                "design"
            ]
        );
    }

    #[test]
    fn paths_expand_like_node_path_join() {
        assert_eq!(normalize_abs("/a/./b//c/../d/"), "/a/b/d/");
        assert_eq!(normalize_abs("/../x"), "/x");
        assert_eq!(normalize_abs("/a/.."), "/");
        assert_eq!(normalize_abs("/a/../"), "/");
        let h = testing::home();
        let ctx = h.ctx();
        assert_eq!(js_shown(&ctx, "~/.claude/../x.md"), "~/x.md");
        assert_eq!(js_shown(&ctx, "~/"), "~");
        assert_eq!(js_shown(&ctx, "~"), "~");
        assert_eq!(js_p(&ctx, "~/a"), format!("{}/a", h.s()));
        // absolute paths are not normalized, as in the reference
        assert_eq!(js_shown(&ctx, "/opt/./x"), "/opt/./x");
    }

    #[test]
    fn js_helpers_behave_like_javascript() {
        assert_eq!(js_string(&json!("a")), "a");
        assert_eq!(js_string(&json!(null)), "null");
        assert_eq!(js_string(&json!(true)), "true");
        assert_eq!(js_string(&json!(3)), "3");
        assert_eq!(js_string(&json!(1.0)), "1");
        assert_eq!(js_string(&json!(1.5)), "1.5");
        assert_eq!(js_string(&json!(-0.0)), "0");
        assert_eq!(js_string(&json!(["a", null, 2])), "a,,2");
        assert_eq!(js_string(&json!({ "a": 1 })), "[object Object]");

        assert_eq!(js_slice("abcdef", 3), "abc");
        assert_eq!(js_slice("ab", 5), "ab");
        assert_eq!(js_slice("éé", 1), "é");
        // an astral character is two UTF-16 units and is never split in two
        assert_eq!(js_slice("a\u{1F600}b", 2), "a");
        assert_eq!(js_slice("a\u{1F600}b", 3), "a\u{1F600}");

        assert!(truthy(Some(&json!(""))).is_none());
        assert!(truthy(Some(&json!(0))).is_none());
        assert!(truthy(None).is_none());
        assert!(truthy(Some(&json!([]))).is_some());
        assert!(present(Some(&json!(""))).is_some());
        assert!(present(Some(&json!(null))).is_none());
        assert_eq!(key_list(Some(&json!({ "b": 1, "a": 2 }))), ["b", "a"]);
        assert_eq!(key_list(Some(&json!(["x"]))), ["0"]);
        assert_eq!(entries(Some(&json!({ "k": 1 }))).len(), 1);
        assert_eq!(entries(Some(&json!(["a"])))[0].0, "0");
        assert_eq!(key_list(Some(&json!("ab"))), ["0", "1"]);
        assert_eq!(key_list(Some(&json!(7))), Vec::<String>::new());
        let ordered =
            js_order(json!({ "b": 1, "10": 2, "a": { "2": 0, "z": 0, "1": 0 }, "01": 3, "3": 4 }));
        assert_eq!(
            ordered.to_string(),
            r#"{"3":4,"10":2,"b":1,"a":{"1":0,"2":0,"z":0},"01":3}"#
        );
    }
}
