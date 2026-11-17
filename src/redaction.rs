use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

static PRIVATE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----")
        .unwrap()
});
static AUTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(Bearer|Basic)\s+[A-Za-z0-9._~+/=-]+").unwrap());
static KEY_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b((?:[A-Z0-9]+[_-])*(?:api[_-]?key|access[_-]?token|refresh[_-]?token|token|password|passwd|secret|credential|authorization|cookie))\s*[:=]\s*(?:"[^"]*"|'[^']*'|[^\s,;]+)"#).unwrap()
});
static FLAG_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(--(?:[a-z0-9]+[-_])*(?:api[-_]?key|token|password|passwd|secret|credential))\s+(?:"[^"]*"|'[^']*'|[^\s,;]+)"#).unwrap()
});
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{20,}|AIza[A-Za-z0-9_-]{25,}|AKIA[A-Z0-9]{16}|eyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})\b|aigh1_[A-Fa-f0-9]{64}").unwrap()
});
static URL_AUTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([a-zA-Z][a-zA-Z0-9+.-]*://)[^/@\s:]+:[^/@\s]+@").unwrap());

fn normalize_key(key: &str) -> String {
    let trimmed = key.trim_start_matches('-');
    let mut normalized = String::with_capacity(trimmed.len() + 4);
    let chars: Vec<char> = trimmed.chars().collect();
    for (idx, &ch) in chars.iter().enumerate() {
        if ch == '-' {
            normalized.push('_');
            continue;
        }
        if ch.is_ascii_uppercase() {
            let prev = idx.checked_sub(1).map(|i| chars[i]);
            let next = chars.get(idx + 1).copied();
            let boundary = prev.is_some_and(|p| {
                p != '-'
                    && (p.is_ascii_lowercase()
                        || p.is_ascii_digit()
                        || (p.is_ascii_uppercase() && next.is_some_and(|n| n.is_ascii_lowercase())))
            });
            if boundary {
                normalized.push('_');
            }
        }
        normalized.push(ch.to_ascii_lowercase());
    }
    normalized
}

fn sensitive_key(key: &str) -> bool {
    let key = normalize_key(key);
    [
        "authorization",
        "cookie",
        "set_cookie",
        "api_key",
        "apikey",
        "token",
        "password",
        "passwd",
        "secret",
        "credentials",
        "credential",
        "private_key",
    ]
    .contains(&key.as_str())
        || key.ends_with("_token")
        || key.ends_with("_password")
        || key.ends_with("_secret")
        || key.ends_with("_api_key")
}

pub fn clip(input: &str, max: usize) -> String {
    if input.len() <= max {
        return input.to_string();
    }
    let mut end = max;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated]", &input[..end])
}

#[derive(Clone)]
pub struct Redactor {
    secrets: Vec<String>,
}
impl Redactor {
    pub fn new(secrets: Vec<String>) -> Self {
        let mut secrets: Vec<String> = secrets.into_iter().filter(|s| s.len() >= 4).collect();
        secrets.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        secrets.dedup();
        Self { secrets }
    }
    /// Extend a redaction dictionary without exposing or mutating its values.
    /// Captured diagnostics must merge private runtime and caller secrets
    /// before masking the original bytes, including overlapping values.
    pub fn with_secrets(&self, values: impl IntoIterator<Item = String>) -> Self {
        Self::new(self.secrets.iter().cloned().chain(values).collect())
    }
    /// Combine literal dictionaries without changing either source. Longer
    /// values precede their prefixes so masking cannot expose a secret suffix.
    pub fn union(&self, other: &Self) -> Self {
        self.with_secrets(other.secrets.iter().cloned())
    }
    /// Require exact membership for every known literal secret in `other`.
    /// Conservatively require descending lengths to mask longer secrets first;
    /// overlapping values do not imply coverage of another dictionary.
    pub fn covers(&self, other: &Self) -> bool {
        self.secrets
            .windows(2)
            .all(|pair| pair[0].len() >= pair[1].len())
            && other
                .secrets
                .iter()
                .all(|secret| self.secrets.contains(secret))
    }
    pub fn text(&self, input: &str) -> String {
        let mut s = input.to_string();
        for secret in &self.secrets {
            s = s.replace(secret, "[REDACTED]");
        }
        s = PRIVATE_KEY
            .replace_all(&s, "[REDACTED PRIVATE KEY]")
            .into_owned();
        s = AUTH.replace_all(&s, "$1 [REDACTED]").into_owned();
        s = TOKEN.replace_all(&s, "[REDACTED]").into_owned();
        s = URL_AUTH.replace_all(&s, "$1[REDACTED]@").into_owned();
        s = KEY_VALUE.replace_all(&s, "$1=[REDACTED]").into_owned();
        FLAG_SECRET.replace_all(&s, "$1 [REDACTED]").into_owned()
    }
    /// Mask a bounded plain-text capture before lossy UTF-8 decoding.
    ///
    /// A truncated capture may end within a known literal secret. Mark full
    /// occurrences and secret-prefix suffixes on the original bytes, then mask
    /// their union. Neither periodic nor overlapping secrets can move the cut
    /// into another occurrence or conceal an unfinished longer secret.
    /// This protects literal known bytes, not arbitrary secret encodings. JSONL
    /// must instead retain complete records and mask their decoded values.
    pub fn captured_text(&self, input: &[u8], truncated: bool) -> String {
        if !truncated || self.secrets.is_empty() {
            return self.text(&String::from_utf8_lossy(input));
        }
        let patterns: Vec<_> = self
            .secrets
            .iter()
            .map(|secret| (secret.as_bytes(), prefix_table(secret.as_bytes())))
            .collect();
        let mut hidden = vec![false; input.len()];
        for (secret, prefix) in &patterns {
            mark_literal_bytes(input, secret, prefix, &mut hidden);
        }
        let tail = patterns
            .iter()
            .map(|(secret, prefix)| {
                // Only a proper prefix can be an unfinished secret. Restrict
                // the KMP scan to the suffix that could contain such a prefix.
                let start = input.len().saturating_sub(secret.len() - 1);
                let mut matched = 0;
                for &byte in &input[start..] {
                    while matched > 0 && secret[matched] != byte {
                        matched = prefix[matched - 1];
                    }
                    if secret[matched] == byte {
                        matched += 1;
                    }
                }
                matched
            })
            .max()
            .unwrap_or(0);
        if tail > 0 {
            hidden[input.len() - tail..].fill(true);
        }
        let mut bytes = Vec::with_capacity(input.len());
        let mut index = 0;
        while index < input.len() {
            if hidden[index] {
                bytes.extend_from_slice(b"[REDACTED]");
                while index < input.len() && hidden[index] {
                    index += 1;
                }
            } else {
                bytes.push(input[index]);
                index += 1;
            }
        }
        self.text(&String::from_utf8_lossy(&bytes))
    }
    pub fn value(&self, input: &Value) -> Value {
        self.value_with_string_limit(input, Some(32_768))
    }
    /// Mask structured values without truncating strings. Callers must enforce
    /// their own serialized size limit before persistence or transport.
    pub fn value_unclipped(&self, input: &Value) -> Value {
        self.value_with_string_limit(input, None)
    }
    fn value_with_string_limit(&self, input: &Value, max_string_bytes: Option<usize>) -> Value {
        match input {
            Value::String(s) => {
                let masked = self.text(s);
                Value::String(match max_string_bytes {
                    Some(max) => clip(&masked, max),
                    None => masked,
                })
            }
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| {
                        (
                            self.text(k),
                            if sensitive_key(k) {
                                Value::String("[REDACTED]".into())
                            } else {
                                self.value_with_string_limit(v, max_string_bytes)
                            },
                        )
                    })
                    .collect(),
            ),
            Value::Array(items) => {
                let mut hide_next = false;
                Value::Array(
                    items
                        .iter()
                        .map(|v| {
                            let hide = hide_next;
                            hide_next = v
                                .as_str()
                                .is_some_and(|s| s.starts_with('-') && sensitive_key(s));
                            if hide {
                                Value::String("[REDACTED]".into())
                            } else {
                                self.value_with_string_limit(v, max_string_bytes)
                            }
                        })
                        .collect(),
                )
            }
            _ => input.clone(),
        }
    }
}

fn prefix_table(pattern: &[u8]) -> Vec<usize> {
    let mut prefix = vec![0; pattern.len()];
    let mut matched = 0;
    for index in 1..pattern.len() {
        while matched > 0 && pattern[matched] != pattern[index] {
            matched = prefix[matched - 1];
        }
        if pattern[matched] == pattern[index] {
            matched += 1;
        }
        prefix[index] = matched;
    }
    prefix
}

/// Mark all literal matches in one KMP scan. A frontier avoids marking a byte
/// twice per secret, even when complete matches overlap periodically.
fn mark_literal_bytes(input: &[u8], pattern: &[u8], prefix: &[usize], hidden: &mut [bool]) {
    let mut matched = 0;
    let mut marked_until = 0;
    for (index, &byte) in input.iter().enumerate() {
        while matched > 0 && pattern[matched] != byte {
            matched = prefix[matched - 1];
        }
        if pattern[matched] == byte {
            matched += 1;
        }
        if matched == pattern.len() {
            let start = (index + 1 - pattern.len()).max(marked_until);
            hidden[start..=index].fill(true);
            marked_until = index + 1;
            matched = prefix[matched - 1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn masks_known_secret_and_bearer_before_storage() {
        let r = Redactor::new(vec!["actual-secret-value".into()]);
        let s = r.text("x=actual-secret-value Authorization: Bearer abcdefghijklmnopqrstuvwxyz");
        assert!(!s.contains("actual-secret-value"));
        assert!(!s.contains("abcdefghijklmnopqrstuvwxyz"));
    }
    #[test]
    fn masks_nested_sensitive_keys_and_argv_flags() {
        let r = Redactor::new(vec![]);
        let v=r.value(&json!({"nested":{"api_key":"never-log-this"},"argv":["tool","--token","very-private-value"]}));
        assert!(!v.to_string().contains("never-log-this"));
        assert!(!v.to_string().contains("very-private-value"));
    }
    #[test]
    fn unclipped_values_preserve_long_strings_and_share_structured_masking() {
        let r = Redactor::new(vec!["fixture-known-secret".into()]);
        let long = "x".repeat(40_000);
        let input = json!({"entries":[{
            "body":long,
            "literal":"fixture-known-secret",
            "api_key":"fixture-key-value",
            "argv":["tool","--token","fixture-argument-value"]
        }]});
        let mut expected = json!({"entries":[{
            "body":long,
            "literal":"[REDACTED]",
            "api_key":"[REDACTED]",
            "argv":["tool","--token","[REDACTED]"]
        }]});
        assert_eq!(r.value_unclipped(&input), expected);
        expected["entries"][0]["body"] =
            Value::String(format!("{}…[truncated]", "x".repeat(32_768)));
        assert_eq!(r.value(&input), expected);
    }
    #[test]
    fn union_covers_exact_filtered_dictionaries_without_mutating_sources() {
        let left = Redactor::new(vec!["fixture-shared-long".into(), "fixture-left".into()]);
        let right = Redactor::new(vec![
            "fixture-shared-long".into(),
            "fixture-right".into(),
            "abc".into(),
        ]);
        let original_left = left.secrets.clone();
        let original_right = right.secrets.clone();
        let combined = left.union(&right);
        assert!(combined.covers(&left) && combined.covers(&right));
        assert!(!left.covers(&right) && !right.covers(&left));
        assert!(
            !Redactor::new(vec!["fixture-shared".into()])
                .covers(&Redactor::new(vec!["fixture-shared-long".into()]))
        );
        assert!(Redactor::new(vec![]).covers(&Redactor::new(vec!["abc".into()])));
        assert_eq!(
            combined.text("fixture-left fixture-right fixture-shared-long"),
            "[REDACTED] [REDACTED] [REDACTED]"
        );
        assert_eq!(left.secrets, original_left);
        assert_eq!(right.secrets, original_right);
    }
    #[test]
    fn union_masks_longer_secrets_before_known_prefixes_and_deduplicates() {
        let prefix = Redactor::new(vec!["fixture-prefix".into()]);
        let longer = Redactor::new(vec!["fixture-prefix-canary-tail".into()]);
        let normalized_order = Redactor::new(vec![
            "fixture-prefix".into(),
            "fixture-prefix-canary-tail".into(),
        ]);
        assert!(normalized_order.covers(&longer));
        assert_eq!(
            normalized_order.text("fixture-prefix-canary-tail fixture-prefix"),
            "[REDACTED] [REDACTED]"
        );
        let combined = prefix.union(&longer);
        assert_eq!(
            combined.text("fixture-prefix-canary-tail fixture-prefix"),
            "[REDACTED] [REDACTED]"
        );
        assert_eq!(combined.union(&longer).secrets, combined.secrets);
        assert!(combined.covers(&prefix) && combined.covers(&longer));
        assert!(combined.covers(&normalized_order));
    }
    #[test]
    fn leaves_readable_safe_text_and_utf8() {
        let r = Redactor::new(vec![]);
        assert_eq!(r.text("Événement : cargo test"), "Événement : cargo test");
    }
    #[test]
    fn new_and_with_secrets_sort_descending_satisfy_covers_and_mask_long_before_short() {
        let longer_only = Redactor::new(vec!["fixture-prefix-canary-tail".into()]);
        let ordered = Redactor::new(vec![
            "fixture-prefix".into(),
            "fixture-prefix-canary-tail".into(),
            "abc".into(),
            "fixture-prefix".into(),
        ]);
        assert!(ordered.covers(&longer_only));
        assert_eq!(
            ordered.text("fixture-prefix-canary-tail fixture-prefix abc"),
            "[REDACTED] [REDACTED] abc"
        );
        let extended = Redactor::new(vec!["fixture-prefix".into()])
            .with_secrets(["fixture-prefix-canary-tail".into(), "fixture-prefix".into()]);
        assert!(extended.covers(&longer_only));
        assert_eq!(
            extended.text("fixture-prefix-canary-tail fixture-prefix"),
            "[REDACTED] [REDACTED]"
        );
        let tied = Redactor::new(vec!["bbbb".into(), "aaaa".into(), "bbbb".into()]);
        assert_eq!(tied.secrets, vec!["aaaa".to_string(), "bbbb".to_string()]);
    }
    #[test]
    fn masks_camelcase_sensitive_keys_while_preserving_snake_and_kebab_keys() {
        let r = Redactor::new(vec![]);
        let input = json!({
            "accessToken": "canary-access-token",
            "apiKey": "canary-api-key",
            "clientSecret": "canary-client-secret",
            "privateKey": "canary-private-key",
            "refreshToken": "canary-refresh-token",
            "idToken": "canary-id-token",
            "sessionToken": "canary-session-token",
            "authToken": "canary-auth-token",
            "customApiKey": "canary-custom-api-key",
            "APIToken": "canary-acronym-api-token",
            "api_key": "canary-snake-api-key",
            "api-key": "canary-kebab-api-key",
            "Set-Cookie": "canary-set-cookie",
            "X-API-Key": "canary-header-api-key",
            "argv": ["tool", "--accessToken", "canary-argv-camel", "--api-key", "canary-argv-kebab"],
            "sessionId": "safe-session-id",
            "publicKey": "safe-public-key",
            "tokenCount": 42,
            "clientId": "safe-client-id"
        });
        let redacted = r.value(&input);
        for secret in [
            "canary-access-token",
            "canary-api-key",
            "canary-client-secret",
            "canary-private-key",
            "canary-refresh-token",
            "canary-id-token",
            "canary-session-token",
            "canary-auth-token",
            "canary-custom-api-key",
            "canary-acronym-api-token",
            "canary-snake-api-key",
            "canary-kebab-api-key",
            "canary-set-cookie",
            "canary-header-api-key",
            "canary-argv-camel",
            "canary-argv-kebab",
        ] {
            assert!(
                !redacted.to_string().contains(secret),
                "sensitive field value leaked: {secret}"
            );
        }
        assert_eq!(redacted["sessionId"], "safe-session-id");
        assert_eq!(redacted["publicKey"], "safe-public-key");
        assert_eq!(redacted["tokenCount"], 42);
        assert_eq!(redacted["clientId"], "safe-client-id");
    }
    #[test]
    fn masks_bare_three_segment_base64url_jwt_tokens() {
        let r = Redactor::new(vec![]);
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyLTEifQ.c2lnbmF0dXJlX3ZhbHVl";
        let jwt_urlsafe =
            "eyJ0eXAiOiJKV1QtXzEifQ.eyJzY29wZV9hIjoiYWJjLV8ifQ.c2lnX2FiYy0xMjM0NTY3ODkw";
        let masked = r.text(&format!("token={jwt} next {jwt_urlsafe}"));
        assert!(!masked.contains("eyJhbGci"));
        assert!(!masked.contains("eyJzdWI"));
        assert!(!masked.contains("c2lnbmF0dXJl"));
        assert!(!masked.contains("eyJ0eXAi"));
        assert!(!masked.contains("c2lnX2FiYy"));
        assert_eq!(masked, "token=[REDACTED] next [REDACTED]");
        let structured = r.value(&json!({"note": format!("raw {jwt}")}));
        assert_eq!(structured["note"], "raw [REDACTED]");
        assert_eq!(r.text("eyJshort.eyJshort"), "eyJshort.eyJshort");
    }
    #[test]
    fn captured_text_truncated_in_middle_of_long_overlapping_secret_masks_cleanly() {
        let r = Redactor::new(vec![
            "fixture-prefix".into(),
            "fixture-prefix-canary-tail".into(),
        ]);
        let truncated_mid_long = r.captured_text(b"output fixture-prefix-can", true);
        assert_eq!(truncated_mid_long, "output [REDACTED]");
        assert!(!truncated_mid_long.contains("-can"));
        assert_eq!(
            r.captured_text(b"output fixture-prefix-canary-tail", false),
            "output [REDACTED]"
        );
        let untruncated_full = r.captured_text(b"fixture-prefix-canary-tail", false);
        assert_eq!(untruncated_full, "[REDACTED]");
    }
}
