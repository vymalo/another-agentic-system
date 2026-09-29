//! Conformance testkit: validates values against the vendored AG-UI 1.0 JSON Schema.
//!
//! Enable the `testkit` feature (as a dev-dependency feature) in any crate that produces
//! AG-UI events and call [`assert_conforms`] on every event it can emit. The schema is the
//! oracle for *structure* only; ordering and lifecycle rules are behavioural and live in the
//! prose specification.
//!
//! The schema is strict on purpose (objects are closed), which is right for tests and wrong
//! for a receive path: do not use this module to validate inbound requests.
//!
//! The assertion functions panic, by design; the `*_errors` functions return the messages.
#![allow(clippy::expect_used)]

use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::Value;

use crate::{Event, RunAgentInput, SCHEMA_1_0};

/// The vendored schema, parsed once.
pub fn schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| serde_json::from_str(SCHEMA_1_0).expect("vendored schema is JSON"))
}

/// A validator for one entry point of the schema. The file's own root `$ref` is `#/$defs/Event`;
/// for any other entry point the root is re-pointed and everything else is left as vendored.
fn validator_for(entry: &str) -> Validator {
    let mut root = schema().clone();
    root.as_object_mut()
        .expect("schema is an object")
        .insert("$ref".to_owned(), Value::String(format!("#/$defs/{entry}")));
    jsonschema::draft202012::options()
        .build(&root)
        .expect("vendored schema compiles")
}

fn event_validator() -> &'static Validator {
    static V: OnceLock<Validator> = OnceLock::new();
    V.get_or_init(|| validator_for("Event"))
}

fn input_validator() -> &'static Validator {
    static V: OnceLock<Validator> = OnceLock::new();
    V.get_or_init(|| validator_for("RunAgentInput"))
}

fn errors(validator: &Validator, instance: &Value) -> Vec<String> {
    validator
        .iter_errors(instance)
        .map(|e| {
            let at = e.instance_path().to_string();
            format!("{e} (at {})", if at.is_empty() { "/" } else { &at })
        })
        .collect()
}

/// Why `event` does not validate as a schema `Event`; empty when it does.
pub fn event_errors(event: &Value) -> Vec<String> {
    errors(event_validator(), event)
}

/// Why `input` does not validate as a schema `RunAgentInput`; empty when it does.
pub fn input_errors(input: &Value) -> Vec<String> {
    errors(input_validator(), input)
}

/// Panics unless the JSON `event` validates as a schema `Event`.
#[track_caller]
pub fn assert_json_conforms(event: &Value) {
    let problems = event_errors(event);
    assert!(
        problems.is_empty(),
        "not an AG-UI 1.0 event:\n  {}\nevent: {event}",
        problems.join("\n  ")
    );
}

/// Panics unless the JSON `input` validates as a schema `RunAgentInput`.
#[track_caller]
pub fn assert_input_json_conforms(input: &Value) {
    let problems = input_errors(input);
    assert!(
        problems.is_empty(),
        "not an AG-UI 1.0 RunAgentInput:\n  {}\ninput: {input}",
        problems.join("\n  ")
    );
}

/// Panics unless `event` serialises to a valid schema `Event`.
#[track_caller]
pub fn assert_conforms(event: &Event) {
    let json = serde_json::to_value(event).expect("event serialises");
    assert_json_conforms(&json);
}

/// Panics unless `input` serialises to a valid schema `RunAgentInput`.
#[track_caller]
pub fn assert_input_conforms(input: &RunAgentInput) {
    let json = serde_json::to_value(input).expect("input serialises");
    assert_input_json_conforms(&json);
}
