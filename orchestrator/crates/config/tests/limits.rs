//! `auth.roles.<role>.limits` (ADR 0081): the shapes the schema accepts and refuses, the defaults,
//! and that a file with no `limits` loads as it did. Nothing reads the limits yet.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::AuthLimits;
use support::{Fake, lines, load};

const MINIMAL: &str = "\
version: 1
database:
  url: { env: DATABASE_URL }
agents:
  file: agents.yaml
";

fn env() -> Fake {
    Fake::default().env("DATABASE_URL", "postgres://u:pw@db/orch")
}

fn with_role(role_body: &str) -> String {
    format!("{MINIMAL}auth:\n  roles:\n    member:\n      permissions: [agent.invoke]\n{role_body}")
}

fn limits_of(text: &str) -> AuthLimits {
    let valid = load(text, &env()).unwrap();
    valid.config.auth.roles.unwrap()["member"].effective_limits()
}

#[test]
fn a_role_with_no_limits_gets_four_runs_at_once_and_no_other_limit() {
    let limits = limits_of(&with_role(""));
    assert_eq!(limits, AuthLimits::default());
    assert_eq!(limits.runs_at_once, 4);
    assert_eq!(limits.model_calls_in_flight, 4);
    assert_eq!(limits.messages_per_minute, None);
    assert_eq!(limits.webhook_jobs_per_minute, None);
    assert_eq!(limits.tokens_per_period, None);
    assert_eq!(limits.admission_wait_secs, None);
}

#[test]
fn a_file_with_no_limits_loads_and_renders_as_before() {
    let valid = load(&with_role(""), &env()).unwrap();
    let json = serde_json::to_value(valid.config.effective()).unwrap();
    assert!(json["auth"]["roles"]["member"].get("limits").is_none());
}

#[test]
fn an_empty_limits_block_is_the_defaults() {
    assert_eq!(
        limits_of(&with_role("      limits: {}\n")),
        AuthLimits::default()
    );
}

#[test]
fn every_key_is_read() {
    let limits = limits_of(&with_role(
        "      limits:\n        runsAtOnce: 2\n        messagesPerMinute: 30\n        \
         webhookJobsPerMinute: 10\n        tokensPerPeriod: 5000000\n        \
         modelCallsInFlight: 1\n        admissionWaitSecs: 20\n",
    ));
    assert_eq!(limits.runs_at_once, 2);
    assert_eq!(limits.messages_per_minute, Some(30));
    assert_eq!(limits.webhook_jobs_per_minute, Some(10));
    assert_eq!(limits.tokens_per_period, Some(5_000_000));
    assert_eq!(limits.model_calls_in_flight, 1);
    assert_eq!(limits.admission_wait_secs, Some(20));
}

#[test]
fn a_key_left_out_keeps_its_default_beside_one_that_is_set() {
    let limits = limits_of(&with_role("      limits: { messagesPerMinute: 9 }\n"));
    assert_eq!(limits.runs_at_once, 4);
    assert_eq!(limits.messages_per_minute, Some(9));
}

#[test]
fn a_zero_or_a_negative_limit_is_refused_naming_its_key() {
    for key in [
        "runsAtOnce",
        "messagesPerMinute",
        "webhookJobsPerMinute",
        "tokensPerPeriod",
        "modelCallsInFlight",
        "admissionWaitSecs",
    ] {
        for bad in ["0", "-1"] {
            let text = with_role(&format!("      limits: {{ {key}: {bad} }}\n"));
            let errors = lines(load(&text, &env()));
            assert!(
                errors.iter().any(|l| l.contains(key)),
                "{key}: {bad} should be refused: {errors:?}"
            );
        }
    }
}

#[test]
fn a_text_a_fraction_and_an_unknown_key_are_refused() {
    for body in [
        "      limits: { runsAtOnce: many }\n",
        "      limits: { runsAtOnce: 1.5 }\n",
        "      limits: { runs_at_once: 2 }\n",
        "      limits: { perHour: 2 }\n",
        "      limits: 4\n",
    ] {
        let errors = lines(load(&with_role(body), &env()));
        assert!(!errors.is_empty(), "{body} should be refused");
    }
}
