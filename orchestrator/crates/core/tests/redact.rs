//! The redaction corpus (ADR 0030): what must never reach the log, and what must pass through.
//! A filter, not a guarantee: the negatives keep the filter from eating the step's usefulness,
//! and a secret in a shape the rules do not know is not in the positives.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::{
    REDACTED, StepOutput, StepReport, StepSource, is_secret_key, redact_text, redact_value,
};
use serde_json::{Value, json};

/// Fake credentials, built so that no scanner takes this file for one.
fn jwt() -> String {
    [
        "eyJhbGciOiJSUzI1NiJ9",
        "eyJzdWIiOiIxMjM0NTY3ODkwIn0",
        "c2lnbmF0dXJlLWJ5dGVz",
    ]
    .join(".")
}
fn gh() -> String {
    format!("{}_{}", "ghp", "a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8")
}
fn sk() -> String {
    format!("{}-{}", "sk", "proj-abcdefghijklmnopqrstuvwxyz0123456789")
}
fn aws() -> String {
    format!("{}{}", "AKIA", "IOSFODNN7EXAMPLE")
}

#[test]
fn credentials_in_text_are_replaced() {
    let cases: Vec<(String, &str)> = vec![
        (format!("curl -H 'Authorization: Bearer {}' https://api", jwt()), "eyJ"),
        ("Authorization: Bearer abcdef1234567890xyz".to_owned(), "abcdef1234567890xyz"),
        ("bearer abcdef1234567890xyz".to_owned(), "abcdef1234567890xyz"),
        ("Authorization: Basic dXNlcjpwYXNzd29yZA==".to_owned(), "dXNlcjpwYXNzd29yZA"),
        (format!("token {}", jwt()), "eyJhbGci"),
        (format!("remote: https://x-access-token:{}@github.com/o/r.git", gh()), "a1B2c3D4"),
        (format!("export GITHUB_TOKEN={}", gh()), "a1B2c3D4"),
        (format!("key {} was rejected", gh()), "a1B2c3D4"),
        (format!("github_pat_{}", "11ABCDEFG0abcdefghijkl_mnopqrstuvwxyz0123456789"), "11ABCDEFG0"),
        (format!("OPENAI_API_KEY={}", sk()), "abcdefghijklmnopqrstuvwxyz"),
        (format!("{{\"key\": \"{}\"}}", sk()), "abcdefghijklmnopqrstuvwxyz"),
        (format!("aws_access_key_id = {}", aws()), "IOSFODNN7EXAMPLE"),
        ("https://user:hunter2@example.com/path".to_owned(), "hunter2"),
        ("postgres://app:s3cr3t-pass@db:5432/orch".to_owned(), "s3cr3t-pass"),
        ("git clone https://ghs0123456789abcdef0123456789@host/x".to_owned(), "ghs0123456789abcdef0123456789"),
        ("GET /cb?code=1&password=hunter2&x=1".to_owned(), "hunter2"),
        ("curl 'https://h/p?access_token=abc123def&x=1'".to_owned(), "abc123def"),
        ("db_password=hunter2 next".to_owned(), "hunter2"),
        ("API_KEY=abcdef123456".to_owned(), "abcdef123456"),
        ("{\"password\": \"hun\\\"ter2\", \"user\": \"me\"}".to_owned(), "ter2"),
        ("{\"client_secret\":\"shh-its-a-secret\"}".to_owned(), "shh-its-a-secret"),
        ("{\"Authorization\" : \"Token abc\"}".to_owned(), "Token abc"),
        (
            "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA0123\nabcd\n-----END RSA PRIVATE KEY-----\nafter".to_owned(),
            "MIIEowIBAAKCAQEA0123",
        ),
        (
            "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkq\n(the output was cut here".to_owned(),
            "MIIEvQIBADANBgkq",
        ),
        (
            "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk\n-----END OPENSSH PRIVATE KEY-----".to_owned(),
            "b3BlbnNzaC1rZXk",
        ),
        (format!("{}-{}", "xoxb", "123456789012-abcdefghijklmnop"), "abcdefghijklmnop"),
    ];
    for (text, secret) in cases {
        let out = redact_text(&text);
        assert!(!out.contains(secret), "not redacted: {text:?} -> {out:?}");
        assert!(out.contains("[redacted"), "no marker: {text:?} -> {out:?}");
    }
}

#[test]
fn what_surrounds_a_credential_stays() {
    let out = redact_text("remote: https://user:hunter2@example.com/o/r.git failed with 403");
    assert_eq!(
        out,
        "remote: https://[redacted]@example.com/o/r.git failed with 403"
    );
    let out = redact_text("a=1&password=hunter2&b=2");
    assert_eq!(out, "a=1&password=[redacted]&b=2");
    let out = redact_text("{\"password\": \"hunter2\", \"user\": \"me\"}");
    assert_eq!(out, "{\"password\": \"[redacted]\", \"user\": \"me\"}");
    let out =
        redact_text("-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\ntrailing text");
    assert_eq!(out, "[redacted private key]\ntrailing text");
    assert_eq!(
        redact_text("Authorization: Bearer abcdefgh12345678"),
        "Authorization: Bearer [redacted]"
    );
}

#[test]
fn ordinary_text_passes_through_untouched() {
    for text in [
        "npm test",
        "cargo build --release",
        "git clone git@github.com:vymalo/another-agentic-system.git",
        "ssh://git@github.com/vymalo/x.git",
        "https://example.com/a@b",
        "https://example.com/docs/token-bucket",
        "the password field is required",
        "set a password in the settings",
        "Token count: 5 (max_tokens=2048)",
        "tokens=5",
        "author=alice",
        "sk-learn and task-list and disk-usage",
        "risk-assessment-for-the-quarter-planning-document",
        "AKIA is a prefix",
        "ghp_ is a prefix, and so is gho_",
        "a bearer of bad news",
        "bearer",
        "Basic usage: run it",
        "eyJ is how base64 json starts",
        "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----",
        "-----BEGIN PUBLIC KEY-----\nMIIB\n-----END PUBLIC KEY-----",
        "commit 5a269b0d5b1f9a4c1c1d2b6f4b8c3f1e2d3c4b5a",
        "sha256:3f2c1b7e9d4a5c6b8e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f",
        "{\"user\": \"me\", \"query\": \"how does token auth work\"}",
        "error: could not resolve host example.org",
        "日本語のテキストと emoji 🎉",
        "",
    ] {
        assert_eq!(redact_text(text), text, "over-redacted: {text:?}");
    }
}

#[test]
fn keys_that_name_a_credential_hide_what_they_hold() {
    let mut v = json!({
        "Authorization": "Bearer x",
        "api_key": "k",
        "apiKey": "k",
        "x-api-key": "k",
        "access_token": "t",
        "token": {"nested": "object"},
        "GITHUB_TOKEN": "t",
        "client-secret": "s",
        "db_password": "p",
        "Password": 12345,
        "private_key": "-",
        "Cookie": "a=b",
        "set-cookie": "a=b",
        "credentials": ["x"],
        "env": {"NPM_TOKEN": "t", "CI": "1"},
        "list": [{"secret": "s", "name": "n"}],
    });
    redact_value(&mut v);
    for key in [
        "Authorization",
        "api_key",
        "apiKey",
        "x-api-key",
        "access_token",
        "token",
        "GITHUB_TOKEN",
        "client-secret",
        "db_password",
        "Password",
        "private_key",
        "Cookie",
        "set-cookie",
        "credentials",
    ] {
        assert_eq!(v[key], REDACTED, "{key}");
    }
    assert_eq!(v["env"], json!({"NPM_TOKEN": REDACTED, "CI": "1"}));
    assert_eq!(v["list"], json!([{"secret": REDACTED, "name": "n"}]));
}

#[test]
fn keys_that_do_not_name_a_credential_keep_their_values() {
    for key in [
        "query",
        "command",
        "path",
        "max_tokens",
        "tokens",
        "input_tokens",
        "author",
        "authority",
        "secretary",
        "passwords_policy",
        "cookies_enabled",
        "limit",
        "url",
        "name",
    ] {
        assert!(!is_secret_key(key), "{key}");
    }
    for key in [
        "token",
        "Token",
        "TOKEN",
        "my_secret",
        "dbPassword",
        "authorization",
        "API-KEY",
    ] {
        assert!(is_secret_key(key), "{key}");
    }
    let mut v = json!({"max_tokens": 5, "query": "node 24", "null_token": null});
    redact_value(&mut v);
    assert_eq!(
        v,
        json!({"max_tokens": 5, "query": "node 24", "null_token": null})
    );
}

#[test]
fn strings_inside_values_are_scanned() {
    let mut v = json!({"command": "curl -H 'Authorization: Bearer abcdefgh12345678' x", "args": ["--password=hunter2", "ok"]});
    redact_value(&mut v);
    let text = v.to_string();
    assert!(
        !text.contains("abcdefgh12345678") && !text.contains("hunter2"),
        "{text}"
    );
    assert_eq!(v["args"][1], "ok");
}

#[test]
fn a_step_is_redacted_at_the_door_in_both_members() {
    let r = StepReport {
        id: "t/a".into(),
        parent: None,
        kind: orch_core::StepKind::Tool,
        label: "fetch".into(),
        state: orch_core::StepState::Completed,
        icon: None,
        detail: None,
        input: json!({"url": "https://u:pw123@h/x", "headers": {"Authorization": "Bearer abcdefgh12345678"}})
            .as_object()
            .cloned(),
        output: Some(StepOutput {
            text: format!("token {}", jwt()),
            ..StepOutput::default()
        }),
    };
    let c = r.sanitize(StepSource::Agent).unwrap();
    let input = Value::Object(c.input.unwrap());
    assert_eq!(input["url"], "https://[redacted]@h/x");
    assert_eq!(input["headers"]["Authorization"], REDACTED);
    assert!(!c.output.unwrap().text.contains("eyJ"));
}

#[test]
fn a_secret_cut_by_the_input_bound_cannot_leak_through_its_first_half() {
    // redaction runs before the cut: a long string that holds a token deep inside it
    let text = format!("{} {} {}", "x".repeat(480), gh(), "y".repeat(100));
    let mut r = StepReport {
        id: "t/a".into(),
        parent: None,
        kind: orch_core::StepKind::Tool,
        label: "x".into(),
        state: orch_core::StepState::Running,
        icon: None,
        detail: None,
        input: json!({ "text": text }).as_object().cloned(),
        output: None,
    };
    let c = r.sanitize(StepSource::Agent).unwrap();
    let out = serde_json::to_string(&c.input).unwrap();
    assert!(!out.contains("a1B2c3D4"), "{out}");
    r.input = None;
}

#[test]
fn matching_time_does_not_depend_on_the_shape_of_the_text() {
    // patterns that backtrack badly in other engines: none of these may stall
    let started = std::time::Instant::now();
    for text in [
        format!("{}!", "a".repeat(200_000)),
        "-----BEGIN PRIVATE KEY-----".repeat(5_000),
        "https://".repeat(20_000),
        "password=".repeat(20_000),
        format!("bearer {}", "a-".repeat(100_000)),
        "\"password\":\"".repeat(10_000),
        format!("eyJ{}.eyJ{}", "a".repeat(50_000), "b".repeat(50_000)),
    ] {
        let _ = redact_text(&text);
    }
    assert!(started.elapsed().as_secs() < 20, "{:?}", started.elapsed());
}

#[test]
fn every_rule_compiles() {
    // the patterns are built once and a bad one would be skipped silently: this is the check
    assert!(is_secret_key("password"));
    assert_ne!(
        redact_text("-----BEGIN PRIVATE KEY-----\nx"),
        "-----BEGIN PRIVATE KEY-----\nx"
    );
    assert_ne!(redact_text("https://u:p@h/"), "https://u:p@h/");
    assert_ne!(redact_text("bearer abcdefgh1234"), "bearer abcdefgh1234");
    assert_ne!(redact_text(&jwt()), jwt());
    assert_ne!(redact_text(&gh()), gh());
    assert_ne!(redact_text(&sk()), sk());
    assert_ne!(redact_text(&aws()), aws());
    assert_ne!(redact_text("password=x1"), "password=x1");
    assert_ne!(redact_text("{\"token\": \"x\"}"), "{\"token\": \"x\"}");
    assert_ne!(
        redact_text("xoxb-1234567890-abcdef"),
        "xoxb-1234567890-abcdef"
    );
}
