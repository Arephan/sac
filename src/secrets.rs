//! Secret detection: shapes of credential values, names of files that hold credentials, and
//! object keys whose values are credentials. Shared by the importer context, the
//! `secrets_by_reference` check and the lint.

use regex::Regex;
use std::sync::OnceLock;

/// A secret-shaped string found in text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretHit {
    pub id: &'static str,
    /// The first six characters followed by an ellipsis, never the whole value.
    pub sample: String,
}

fn patterns() -> &'static Vec<(&'static str, Regex)> {
    static P: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    P.get_or_init(|| {
        let defs: [(&str, &str); 13] = [
            ("slack-token", r"(?-u:\b)xox[abposr]-[A-Za-z0-9-]{10,}"),
            ("slack-app-token", r"(?-u:\b)xapp-\d-[A-Za-z0-9-]{10,}"),
            ("openai-style-key", r"(?-u:\b)sk-(?:proj-|ant-)?[A-Za-z0-9_-]{20,}"),
            ("github-token", r"(?-u:\b)(?:ghp|gho|ghu|ghs|ghr|github_pat)_[A-Za-z0-9_]{20,}"),
            ("aws-access-key", r"(?-u:\b)AKIA[0-9A-Z]{16}(?-u:\b)"),
            ("google-api-key", r"(?-u:\b)AIza[0-9A-Za-z_-]{35}(?-u:\b)"),
            ("google-oauth-token", r"(?-u:\b)ya29\.[0-9A-Za-z_-]{20,}"),
            ("stripe-key", r"(?-u:\b)(?:sk|rk|pk)_(?:live|test)_[0-9A-Za-z]{16,}"),
            ("private-key-block", r"-----BEGIN (?:RSA |EC |OPENSSH |PGP )?PRIVATE KEY-----"),
            ("jwt", r"(?-u:\b)eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}"),
            ("bearer-header", r"(?-u:\b)Bearer\s+[A-Za-z0-9._~+/-]{20,}=*"),
            ("url-credentials", r"(?i)(?-u:\b)[a-z][a-z0-9+.-]*://[^\s:/@]+:[^\s:/@]{6,}@"),
            (
                "generic-assignment",
                r#"(?i)(?-u:\b)(?:api[_-]?key|secret|token|password|passwd)\s*[:=]\s*["']?[A-Za-z0-9_\-/+=]{24,}"#,
            ),
        ];
        defs.iter()
            .map(|(id, re)| (*id, Regex::new(re).expect("secret pattern compiles")))
            .collect()
    })
}

/// Every secret pattern that matches `text`, once per pattern.
pub fn find_secrets(text: &str) -> Vec<SecretHit> {
    patterns()
        .iter()
        .filter_map(|(id, re)| {
            re.find(text).map(|m| SecretHit {
                id,
                sample: format!("{}…", m.as_str().chars().take(6).collect::<String>()),
            })
        })
        .collect()
}

fn secret_file_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(
            r"(?i)(^|/)(\.env(\..*)?|.*token.*|.*_key|.*\.key|.*\.pem|.*\.p12|.*\.p8|id_(rsa|ed25519|ecdsa)(\.pub)?|credentials(\.json)?|auth\.json|.*secret.*|.*\.keychain(-db)?|cookies(\.sqlite)?|login data)$",
        )
        .unwrap()
    })
}

/// True when a path's final component looks like it holds a credential.
pub fn is_secret_file(path: &str) -> bool {
    secret_file_re().is_match(path)
}

fn secret_key_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)^(env|headers|environment|environmentvariables|token|apikey|api_key|secret|password|authorization|auth|credentials|cookie)$").unwrap()
    })
}

/// True for object keys whose values are credentials and must be reduced to names.
pub fn is_secret_key(key: &str) -> bool {
    secret_key_re().is_match(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_tokens_and_keeps_only_a_sample() {
        let tok = ["xoxb-", "1234567890-abcdefghij"].concat();
        let hits = find_secrets(&format!("token {tok} here"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "slack-token");
        assert_eq!(hits[0].sample, "xoxb-1…");
        assert!(find_secrets("plain text ~/.bot_token").is_empty());
        let glued = format!("é{tok}日");
        assert_eq!(
            find_secrets(&glued).len(),
            1,
            "ASCII word boundaries, like JavaScript's"
        );
    }

    #[test]
    fn secret_files_and_keys() {
        assert!(is_secret_file("/Users/alex/.slack_bot_token"));
        assert!(is_secret_file("~/.env.local"));
        assert!(is_secret_file("/x/credentials.json"));
        assert!(!is_secret_file("/x/settings.json"));
        assert!(is_secret_key("env"));
        assert!(is_secret_key("Headers"));
        assert!(!is_secret_key("env_names"));
    }
}
