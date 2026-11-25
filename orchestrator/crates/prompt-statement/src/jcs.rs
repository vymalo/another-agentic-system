//! RFC 8785 (the JSON Canonicalization Scheme) over `serde_json` values, and the hashes the contract takes of it.
//!
//! Written out here, not left to `serde_json`'s printer: object members are sorted by the UTF-16 code units of their
//! names (§3.2.3, not by code point and not in `serde_json`'s map order, which a feature of another crate can change for
//! the whole build), strings escape only what §3.2.2.2 says, and numbers print as ECMAScript's `Number::toString` does
//! (§3.2.2.3, through `ryu-js`). A `serde_json::Value` cannot hold what RFC 8785 refuses (`NaN`, an infinity, a lone
//! surrogate: `serde_json` refuses them when it parses), so canonicalization does not fail.

use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Number, Value};
use sha2::{Digest, Sha256};

/// The RFC 8785 bytes of `value`.
pub fn canonical(value: &Value) -> Vec<u8> {
    let mut out = String::new();
    write_value(value, &mut out);
    out.into_bytes()
}

/// `base64url(SHA-256(bytes))`, without padding: the form of every `*_sha256` member of the contract.
pub fn sha256_b64u(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(bytes))
}

/// `base64url(SHA-256(JCS(value)))`: the hash of a message's `content`, of `forwardedProps` or of the A2A parts.
pub fn hash_of(value: &Value) -> String {
    sha256_b64u(&canonical(value))
}

/// The `props_sha256` of a run: the hash of its `forwardedProps`, or of `{}` when it has none (absent or JSON `null`).
pub fn props_hash(forwarded_props: Option<&Value>) -> String {
    match forwarded_props {
        None | Some(Value::Null) => sha256_b64u(b"{}"),
        Some(props) => hash_of(props),
    }
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => write_number(n, out),
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut members: Vec<(&String, &Value)> = map.iter().collect();
            members.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, (name, inner)) in members.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(name, out);
                out.push(':');
                write_value(inner, out);
            }
            out.push('}');
        }
    }
}

/// §3.2.2.2: `"` and `\` escaped, the control characters as the short forms JSON has and `\u00xx` (lower case)
/// otherwise; everything else, U+007F and every non-ASCII character included, as it is.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if u32::from(c) < 0x20 => {
                // writing to a String does not fail
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// §3.2.2.3: as ECMAScript prints the IEEE 754 double the number is. An integer up to 2^53 in magnitude is that double
/// exactly and prints as its decimal; anything else goes through the double.
fn write_number(n: &Number, out: &mut String) {
    const EXACT: u64 = 1 << 53;
    if let Some(i) = n.as_i64().filter(|i| i.unsigned_abs() <= EXACT) {
        let _ = write!(out, "{i}");
    } else if let Some(u) = n.as_u64().filter(|u| *u <= EXACT) {
        let _ = write!(out, "{u}");
    } else if let Some(f) = n.as_f64() {
        if f == 0.0 {
            // -0 prints as 0
            out.push('0');
        } else {
            out.push_str(ryu_js::Buffer::new().format_finite(f));
        }
    } else {
        // unreachable without serde_json's `arbitrary_precision`, which no crate of the workspace turns on
        out.push_str(&n.to_string());
    }
}
