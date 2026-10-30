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
    Regex::new(r"\b(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9]{16,}|github_pat_[A-Za-z0-9_]{20,}|AIza[A-Za-z0-9_-]{25,}|AKIA[A-Z0-9]{16})\b").unwrap()
});
static URL_AUTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([a-zA-Z][a-zA-Z0-9+.-]*://)[^/@\s:]+:[^/@\s]+@").unwrap());

fn sensitive_key(key: &str) -> bool {
    let key = key
        .trim_start_matches('-')
        .to_ascii_lowercase()
        .replace('-', "_");
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
        Self {
            secrets: secrets.into_iter().filter(|s| s.len() >= 4).collect(),
        }
    }
    /// Extend a redaction dictionary without exposing or mutating its values.
    /// Captured diagnostics must merge private runtime and caller secrets
    /// before masking the original bytes, including overlapping values.
    pub fn with_secrets(&self, values: impl IntoIterator<Item = String>) -> Self {
        Self::new(self.secrets.iter().cloned().chain(values).collect())
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
        match input {
            Value::String(s) => Value::String(clip(&self.text(s), 32_768)),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| {
                        (
                            self.text(k),
                            if sensitive_key(k) {
                                Value::String("[REDACTED]".into())
                            } else {
                                self.value(v)
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
                                self.value(v)
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
    fn leaves_readable_safe_text_and_utf8() {
        let r = Redactor::new(vec![]);
        assert_eq!(r.text("Événement : cargo test"), "Événement : cargo test");
    }
}
