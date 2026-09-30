//! Rejects text that looks like a credential before it is stored.
//!
//! A false positive costs the Agent one rephrase; a stored live key leaks into
//! every Project and Agent Kind that can see the Scope.

use std::sync::OnceLock;

use regex::Regex;

struct Pattern {
    name: &'static str,
    regex: Regex,
}

fn patterns() -> &'static [Pattern] {
    static PATTERNS: OnceLock<Vec<Pattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            ("private key", r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
            ("aws access key", r"\b(AKIA|ASIA)[0-9A-Z]{16}\b"),
            (
                "github token",
                r"\b(gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{22,})",
            ),
            ("stripe key", r"\b[sr]k_live_[0-9A-Za-z]{16,}"),
            ("anthropic key", r"\bsk-ant-[A-Za-z0-9_-]{20,}"),
            ("openai key", r"\bsk-(proj-|svcacct-)?[A-Za-z0-9_-]{32,}"),
            ("slack token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}"),
            ("google api key", r"\bAIza[0-9A-Za-z_-]{35}\b"),
            (
                "jwt",
                r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
            ),
        ]
        .into_iter()
        .map(|(name, pattern)| Pattern {
            name,
            regex: Regex::new(pattern).expect("secret pattern compiles"),
        })
        .collect()
    })
}

fn assignment() -> &'static Regex {
    static ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
    ASSIGNMENT.get_or_init(|| {
        Regex::new(
            r#"(?i)\b[a-z0-9_]*(secret|token|passw(or)?d|api_?key|private_?key|access_?key)[a-z0-9_]*\s*[=:]\s*["']?([^\s"']{12,})"#,
        )
        .expect("assignment pattern compiles")
    })
}

/// Shannon entropy in bits per character.
fn entropy(value: &str) -> f64 {
    let mut counts = [0u32; 256];
    for byte in value.bytes() {
        counts[byte as usize] += 1;
    }
    let len = value.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

/// Returns the kind of secret found, if any.
pub fn detect(text: &str) -> Option<&'static str> {
    if let Some(pattern) = patterns().iter().find(|p| p.regex.is_match(text)) {
        return Some(pattern.name);
    }
    assignment()
        .captures_iter(text)
        .filter_map(|c| c.get(3))
        .any(|value| entropy(value.as_str()) >= 3.5)
        .then_some("credential assignment")
}

#[cfg(test)]
mod tests {
    use super::detect;

    // Fixtures are split with concat! so the source never holds a complete
    // token shape; hosting push protection would otherwise block the repo.
    #[test]
    fn flags_known_token_shapes() {
        let cases = [
            (
                concat!("stripe key is sk_", "live_51H8xY2eZvKYlo2C0aBcDeFgHiJ"),
                "stripe key",
            ),
            (concat!("AKIA", "IOSFODNN7EXAMPLE"), "aws access key"),
            (
                concat!("token gh", "p_0123456789abcdefghijABCDEFGHIJ012345"),
                "github token",
            ),
            (
                concat!("sk-", "ant-api03-abcdefghijklmnopqrstuvwxyz"),
                "anthropic key",
            ),
            (
                concat!("-----BEGIN OPENSSH ", "PRIVATE KEY-----"),
                "private key",
            ),
            ("DB_PASSWORD=Xk9#mQ2$vLp8zR4w", "credential assignment"),
        ];
        for (text, kind) in cases {
            assert_eq!(detect(text), Some(kind), "{text}");
        }
    }

    #[test]
    fn allows_ordinary_prose() {
        let cases = [
            "auth uses JWT in an httpOnly cookie",
            "store the token in REMEMBER_TOKEN env var",
            "password reset flow lives in src/auth/reset.rs",
            "api_key=changeme_later",
            "use sk- prefix check in the validator",
        ];
        for text in cases {
            assert_eq!(detect(text), None, "{text}");
        }
    }
}
