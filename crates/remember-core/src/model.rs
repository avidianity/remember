//! Domain vocabulary shared by every transport. Terms follow CONTEXT.md.

use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    Project,
    Task,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project => "project",
            Scope::Task => "task",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "user" => Ok(Scope::User),
            "project" => Ok(Scope::Project),
            "task" => Ok(Scope::Task),
            other => Err(Error::rejected(format!(
                "unknown scope '{other}'; use user, project or task"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Fact,
    Decision,
    Todo,
    Note,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Category::Fact => "fact",
            Category::Decision => "decision",
            Category::Todo => "todo",
            Category::Note => "note",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "fact" => Ok(Category::Fact),
            "decision" => Ok(Category::Decision),
            "todo" => Ok(Category::Todo),
            "note" => Ok(Category::Note),
            other => Err(Error::rejected(format!(
                "unknown category '{other}'; use fact, decision, todo or note"
            ))),
        }
    }
}

/// Caps and budgets; every value can be overridden by the host.
#[derive(Debug, Clone)]
pub struct Limits {
    pub memory_chars: usize,
    pub brief_chars: usize,
    pub message_chars: usize,
    pub revisions_kept: usize,
    pub context_tokens: usize,
    pub recall_tokens: usize,
    pub join_tokens: usize,
    pub recall_default: usize,
    pub recall_max: usize,
    pub stale_after_secs: i64,
    pub online_window_secs: i64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            memory_chars: 1000,
            brief_chars: 4000,
            message_chars: 1000,
            revisions_kept: 10,
            context_tokens: 1500,
            recall_tokens: 800,
            join_tokens: 1500,
            recall_default: 10,
            recall_max: 50,
            stale_after_secs: 14 * DAY,
            online_window_secs: 10 * 60,
        }
    }
}

pub const DAY: i64 = 24 * 60 * 60;

pub fn memory_ref(id: i64) -> String {
    format!("m{id}")
}

pub fn task_ref(id: i64) -> String {
    format!("t{id}")
}

/// Accepts `m42` or `42`.
pub fn parse_ref(value: &str, prefix: char, noun: &str) -> Result<i64> {
    let digits = value.trim().strip_prefix(prefix).unwrap_or(value.trim());
    digits
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| Error::rejected(format!("'{value}' is not a {noun} id like {prefix}42")))
}

pub fn is_task_ref(value: &str) -> bool {
    value
        .strip_prefix('t')
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// Compact relative time: `now`, `5m`, `3h`, `12d`, `2y`.
pub fn age(seconds: i64) -> String {
    let s = seconds.max(0);
    match s {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m", s / 60),
        3600..DAY => format!("{}h", s / 3600),
        _ if s < 365 * DAY => format!("{}d", s / DAY),
        _ => format!("{}y", s / (365 * DAY)),
    }
}

/// UTC timestamp like `2026-09-30T14:05:00Z`, for human-facing output.
pub fn iso8601(unix: i64) -> String {
    let days = unix.div_euclid(DAY);
    let secs = unix.rem_euclid(DAY);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// Conservative token estimate. Each run of letters or digits counts one token
/// per 4 bytes and every other non-space character counts one token, which
/// tracks BPE tokenizers closely on identifier- and punctuation-heavy TOON.
/// Budgets use this at runtime; tests check it against a real tokenizer.
pub fn estimate_tokens(text: &str) -> usize {
    let mut tokens = 0;
    let mut run = 0;
    for c in text.chars() {
        if c.is_alphanumeric() {
            run += c.len_utf8();
            continue;
        }
        tokens += run.div_ceil(4);
        run = 0;
        if !c.is_whitespace() {
            tokens += 1;
        }
    }
    tokens + run.div_ceil(4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_age() {
        assert_eq!(age(5), "now");
        assert_eq!(age(300), "5m");
        assert_eq!(age(3 * 3600), "3h");
        assert_eq!(age(12 * DAY), "12d");
        assert_eq!(age(800 * DAY), "2y");
    }

    #[test]
    fn formats_iso8601() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_790_777_100), "2026-09-30T14:05:00Z");
        assert_eq!(iso8601(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn parses_refs() {
        assert_eq!(parse_ref("m42", 'm', "memory").unwrap(), 42);
        assert_eq!(parse_ref("42", 'm', "memory").unwrap(), 42);
        assert!(parse_ref("t42", 'm', "memory").is_err());
        assert!(is_task_ref("t7"));
        assert!(!is_task_ref("claude-code#3f2a"));
        assert!(!is_task_ref("t"));
    }
}
