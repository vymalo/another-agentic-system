//! Redaction of what a step carries (ADR 0030): a **filter, not a guarantee**.
//!
//! A step's input and output come from an agent and reach the log, so the core takes out what
//! looks like a credential before anything is kept: values under a key that names one, and text
//! that has the shape of a token, a key, a private key block or a password in a URL. The agent
//! redacts what it knows exactly (its own secrets); this is the net for what it does not know.
//! A secret in a shape no rule here knows passes through, which is why the record can be turned
//! off (`ORCH_STEPS_RECORD_IO`) and why the log is treated as sensitive as the chat is.
//!
//! Pure functions over `str` and [`serde_json::Value`], no I/O. The patterns run on the `regex`
//! crate, whose matching time is linear in the text: a step's text is untrusted and long, and a
//! pattern must not be a way to stall the orchestrator.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::{Regex, RegexSet};
use serde_json::Value;

/// What replaces a redacted value.
pub const REDACTED: &str = "[redacted]";

/// Names that hold a credential, matched at the **end** of a JSON key, ignoring case, so
/// `token`, `access_token`, `GITHUB_TOKEN`, `db-password` and `x-api-key` are all caught.
static SECRET_KEY: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(authorization|api[-_]?key|apikey|token|secret|password|passwd|private[-_]?key|cookie|credentials?)$",
    )
    .ok()
});

/// The text rules: each pattern with its replacement, and the set of all the patterns, which one
/// pass over a text uses to find the rules that apply (most text has none).
struct TextRules {
    set: Option<RegexSet>,
    rules: Vec<(Regex, &'static str)>,
}

/// Text patterns, each with its replacement. Order matters only for the PEM block, which comes
/// first so that what is inside it is not half-matched by a later rule.
static TEXT_RULES: LazyLock<TextRules> = LazyLock::new(|| {
    let rules: [(&str, &str); 11] = [
        // a private key block; one that is cut before its END line is taken to the end of the text
        (
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|\z)",
            "[redacted private key]",
        ),
        // `"password": "..."` inside text that is JSON itself (a tool that returns JSON as text)
        (
            r#"(?i)("[A-Za-z0-9_.-]*(?:authorization|api[-_]?key|apikey|token|secret|password|passwd|private[-_]?key|cookie)"\s*:\s*)"(?:[^"\\]|\\.)*""#,
            r#"$1"[redacted]""#,
        ),
        // a password or a token in a URL: `scheme://user:password@host`, or one long token alone
        (
            r"([A-Za-z][A-Za-z0-9+.-]*://)[^/\s:@]+:[^/\s@]+@",
            "$1[redacted]@",
        ),
        (
            r"([A-Za-z][A-Za-z0-9+.-]*://)[A-Za-z0-9_.~%-]{20,}@",
            "$1[redacted]@",
        ),
        // `Authorization: Bearer ...`, `Bearer ...`, `Basic ...`
        (
            r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}",
            "$1 [redacted]",
        ),
        // a JWT: three base64url parts, the first two starting `eyJ` (`{"`)
        (
            r"\beyJ[A-Za-z0-9_-]{4,}\.eyJ[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]*",
            "[redacted jwt]",
        ),
        // GitHub tokens
        (
            r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})",
            "[redacted token]",
        ),
        // `sk-...` keys (OpenAI, Anthropic and look-alikes)
        (r"\bsk-[A-Za-z0-9_-]{20,}", "[redacted key]"),
        // AWS access key ids
        (r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b", "[redacted key]"),
        // `password=...`, `token=...` pairs in a query string or a config line
        (
            r#"(?i)\b((?:[a-z0-9]+[-_])*(?:password|passwd|pwd|token|secret|api[-_]?key|apikey|access[-_]?key|auth)=)[^\s&"'`]+"#,
            "$1[redacted]",
        ),
        // a Slack token
        (r"\bxox[abprs]-[A-Za-z0-9-]{10,}", "[redacted token]"),
    ];
    TextRules {
        set: RegexSet::new(rules.iter().map(|(pattern, _)| *pattern)).ok(),
        rules: rules
            .iter()
            .filter_map(|(pattern, with)| Regex::new(pattern).ok().map(|re| (re, *with)))
            .collect(),
    }
});

/// Whether the JSON key `key` names a credential, so that whatever it holds is redacted.
pub fn is_secret_key(key: &str) -> bool {
    SECRET_KEY.as_ref().is_some_and(|re| re.is_match(key))
}

/// `text` with every credential-shaped part replaced. Borrowed when there was none.
pub fn redact_text(text: &str) -> Cow<'_, str> {
    let rules = &*TEXT_RULES;
    // one pass finds the rules that match at all; the others are not run. (A rule's replacement
    // cannot make another rule match what it did not match before, except the PEM block, which
    // is first, so the set's answer for the original text stays right.)
    let hits = match &rules.set {
        Some(set) => {
            let matched = set.matches(text);
            if !matched.matched_any() {
                return Cow::Borrowed(text);
            }
            Some(matched)
        }
        None => None,
    };
    let mut out = Cow::Borrowed(text);
    for (n, (re, with)) in rules.rules.iter().enumerate() {
        if hits.as_ref().is_some_and(|h| !h.matched(n)) {
            continue;
        }
        if re.is_match(&out) {
            out = Cow::Owned(re.replace_all(&out, *with).into_owned());
        }
    }
    out
}

/// `value` redacted in place: what an object holds under a secret key becomes
/// [`REDACTED`], and every string is passed through [`redact_text`].
pub fn redact_value(value: &mut Value) {
    match value {
        Value::String(s) => {
            if let Cow::Owned(clean) = redact_text(s) {
                *s = clean;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if is_secret_key(key) && !v.is_null() {
                    *v = Value::String(REDACTED.to_owned());
                } else {
                    redact_value(v);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}
