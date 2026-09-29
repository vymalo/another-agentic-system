//! Every value the crate can emit validates against the vendored official 1.0 schema.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use orch_agui_proto::testkit::{
    assert_conforms, assert_input_conforms, assert_json_conforms, event_errors, input_errors,
    schema,
};
use orch_agui_proto::{
    ActivitySnapshotEvent, Event, Interrupt, RunAgentInput, RunErrorEvent, RunFinishedEvent,
    RunFinishedOutcome, RunStartedEvent, StateSnapshotEvent, SubagentErrorEvent,
    SubagentFinishedEvent, SubagentFinishedOutcome, SubagentStartedEvent, TextMessageContentEvent,
    TextMessageEndEvent, TextMessageRole, TextMessageStartEvent,
};
use serde_json::{Value, json};

fn typed(fixture: Value) -> Event {
    serde_json::from_value(fixture).unwrap()
}

#[test]
fn every_event_type_conforms_to_the_schema() {
    for fixture in support::full_events()
        .into_iter()
        .chain(support::minimal_events())
    {
        assert_conforms(&typed(fixture));
    }
}

#[test]
fn run_agent_input_conforms_to_the_schema() {
    for wire in [support::full_input(), support::minimal_input()] {
        let input: RunAgentInput = serde_json::from_value(wire).unwrap();
        assert_input_conforms(&input);
    }
}

#[test]
fn the_constructors_conform() {
    let meta = |k: &str| {
        let mut m = serde_json::Map::new();
        m.insert(k.to_owned(), json!({"type": "agent", "name": "plain"}));
        m
    };
    let mut status = serde_json::Map::new();
    status.insert("status".to_owned(), json!("working"));
    let events: Vec<Event> = vec![
        RunStartedEvent::new("t", "r").into(),
        StateSnapshotEvent::new(json!({"thread": {"state": "queued"}})).into(),
        TextMessageStartEvent::new("m", TextMessageRole::User).into(),
        TextMessageContentEvent::new("m", "hi").into(),
        TextMessageEndEvent::new("m").into(),
        SubagentStartedEvent::new("sub-1", "plain").into(),
        ActivitySnapshotEvent::new("evt-2", "vymalo.status", status).into(),
        SubagentFinishedEvent::new(
            "sub-1",
            Some(SubagentFinishedOutcome::Suspended {
                interrupt_ids: Some(vec!["int-3".into()]),
            }),
        )
        .into(),
        SubagentFinishedEvent::new("sub-1", None).into(),
        SubagentErrorEvent::new("sub-1", "boom", Some("delivery_failed".into())).into(),
        RunFinishedEvent::new("t", "r", RunFinishedOutcome::success()).into(),
        RunFinishedEvent::new("t", "r", RunFinishedOutcome::Cancelled).into(),
        RunFinishedEvent::new(
            "t",
            "r",
            RunFinishedOutcome::Interrupt {
                interrupts: vec![Interrupt::new("int-3", "input_required")],
            },
        )
        .into(),
        RunErrorEvent::new("failed", None).into(),
        Event::from(RunErrorEvent::new("failed", Some("agent_failed".into()))).with_timestamp_ms(1),
        Event::from(RunStartedEvent::new("t", "r")).with_metadata(meta("vymalo.actor")),
    ];
    for event in &events {
        assert_conforms(event);
    }
}

/// The `required` members of the schema definition of one event type.
fn required_members(event_type: &str) -> BTreeSet<String> {
    let defs = schema()["$defs"].as_object().unwrap();
    let def = defs
        .iter()
        .filter(|(name, _)| name.ends_with("Event"))
        .map(|(_, def)| def)
        .find(|def| def["properties"]["type"]["const"] == json!(event_type))
        .unwrap_or_else(|| panic!("no schema definition for {event_type}"));
    def["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn absent_means_absent_not_null() {
    // Required members only: exactly the schema's `required` set is written, so no optional
    // member leaks out as `null` (or at all).
    for fixture in support::minimal_events() {
        let ty = fixture["type"].as_str().unwrap().to_owned();
        let out = serde_json::to_value(typed(fixture)).unwrap();
        let keys: BTreeSet<String> = out.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            keys,
            required_members(&ty),
            "{ty}: members written vs required by the schema"
        );
    }
    // Everything set: still no structural null anywhere.
    for fixture in support::full_events()
        .into_iter()
        .chain(support::minimal_events())
    {
        let out = serde_json::to_value(typed(fixture)).unwrap();
        assert!(!support::has_structural_null(&out), "{out}");
    }
    let input: RunAgentInput = serde_json::from_value(support::minimal_input()).unwrap();
    let out = serde_json::to_value(&input).unwrap();
    assert_eq!(
        out.as_object().unwrap().len(),
        3,
        "threadId, runId, messages only: {out}"
    );

    // The text form has no `null` either, for a plain constructor-built event.
    let text = serde_json::to_string(&Event::from(RunStartedEvent::new("t", "r"))).unwrap();
    assert!(!text.contains("null"), "{text}");
}

#[test]
fn the_schema_rejects_what_the_types_must_never_emit() {
    // The oracle must be able to say no, or a green run means nothing.
    let cases = [
        (
            "undeclared member",
            json!({"type": "TEXT_MESSAGE_END", "messageId": "m", "extra": 1}),
        ),
        (
            "null for an absent member",
            json!({"type": "RUN_ERROR", "message": "m", "code": null}),
        ),
        (
            "float timestamp",
            json!({"type": "TEXT_MESSAGE_END", "messageId": "m", "timestamp": 1.5}),
        ),
        (
            "timestamp beyond 2^53",
            json!({"type": "TEXT_MESSAGE_END", "messageId": "m", "timestamp": 9_007_199_254_740_993_i64}),
        ),
        ("unknown type", json!({"type": "RUN_CANCELLED"})),
        (
            "empty interrupts",
            json!({"type": "RUN_FINISHED", "threadId": "t", "runId": "r", "outcome": {"type": "interrupt", "interrupts": []}}),
        ),
        (
            "bad role",
            json!({"type": "TEXT_MESSAGE_START", "messageId": "m", "role": "tool"}),
        ),
    ];
    for (why, wire) in cases {
        assert!(
            !event_errors(&wire).is_empty(),
            "schema accepted {why}: {wire}"
        );
    }
    assert!(!input_errors(&json!({"threadId": "t", "runId": "r"})).is_empty());
    assert!(
        !input_errors(&json!({"threadId": "t", "runId": "r", "messages": [], "extra": 1}))
            .is_empty()
    );
}

#[test]
#[should_panic(expected = "not an AG-UI 1.0 event")]
fn assert_conforms_panics_on_a_schema_violation() {
    // Type-correct, schema-invalid: the schema requires at least one interrupt.
    assert_conforms(
        &RunFinishedEvent::new(
            "t",
            "r",
            RunFinishedOutcome::Interrupt { interrupts: vec![] },
        )
        .into(),
    );
}

#[test]
#[should_panic(expected = "not an AG-UI 1.0 RunAgentInput")]
fn assert_input_conforms_panics_on_a_schema_violation() {
    let mut input: RunAgentInput = serde_json::from_value(support::minimal_input()).unwrap();
    input.tools = Some(vec![orch_agui_proto::Tool {
        name: "n".into(),
        description: "d".into(),
        parameters: Some(Value::Null),
        metadata: None,
    }]);
    assert_input_conforms(&input);
}

#[test]
fn assert_json_conforms_accepts_a_hand_written_golden() {
    assert_json_conforms(
        &json!({"type": "RUN_FINISHED", "threadId": "t", "runId": "r", "outcome": {"type": "cancelled"}}),
    );
}
