//! Round trips: literal wire JSON -> typed value -> the same wire JSON.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use orch_agui_proto::{
    Event, EventType, JsonPatchOperation, JsonPointer, Message, RunAgentInput, testkit,
};
use serde_json::{Value, json};

fn type_of(fixture: &Value) -> &str {
    fixture["type"].as_str().unwrap()
}

#[test]
fn fixtures_cover_every_event_type_exactly() {
    let all: BTreeSet<&str> = EventType::ALL.iter().map(|t| t.as_str()).collect();
    assert_eq!(all.len(), 31, "1.0 has 31 event types");
    for (name, fixtures) in [
        ("full", support::full_events()),
        ("minimal", support::minimal_events()),
    ] {
        let seen: BTreeSet<&str> = fixtures.iter().map(type_of).collect();
        assert_eq!(seen, all, "{name} fixtures must cover every event type");
    }
}

#[test]
fn every_event_type_round_trips_exactly() {
    let fixtures = support::full_events()
        .into_iter()
        .chain(support::minimal_events());
    for fixture in fixtures {
        let event: Event = serde_json::from_value(fixture.clone())
            .unwrap_or_else(|e| panic!("{} does not parse: {e}\n{fixture}", type_of(&fixture)));
        assert_eq!(event.event_type().as_str(), type_of(&fixture));
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            fixture,
            "{} does not serialise back to its wire form",
            type_of(&fixture)
        );
        // Through text as well, the way the SSE `data:` line carries it.
        let text = serde_json::to_string(&event).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&text).unwrap(), event);
    }
}

#[test]
fn event_type_spelling_matches_the_serde_tag() {
    for t in EventType::ALL {
        assert_eq!(serde_json::to_value(t).unwrap(), json!(t.as_str()));
        assert_eq!(
            serde_json::from_value::<EventType>(json!(t.as_str())).unwrap(),
            *t
        );
    }
}

#[test]
fn event_types_match_the_official_schema_in_order() {
    let schema = testkit::schema();
    let in_schema: Vec<&str> = schema["$defs"]["EventType"]["enum"]
        .as_array()
        .expect("EventType enum")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let ours: Vec<&str> = EventType::ALL.iter().map(|t| t.as_str()).collect();
    assert_eq!(ours, in_schema);

    let members = schema["$defs"]["Event"]["oneOf"].as_array().unwrap();
    assert_eq!(
        members.len(),
        EventType::ALL.len(),
        "Event is a oneOf over every type"
    );
}

#[test]
fn an_unknown_event_type_is_not_silently_accepted() {
    let err = serde_json::from_value::<Event>(json!({"type": "FROM_THE_FUTURE", "x": 1}))
        .unwrap_err()
        .to_string();
    assert!(err.contains("FROM_THE_FUTURE"), "{err}");
}

#[test]
fn a_missing_required_member_is_an_error() {
    for bad in [
        json!({"type": "RUN_STARTED", "threadId": "t"}),
        json!({"type": "TEXT_MESSAGE_CONTENT", "messageId": "m"}),
        json!({"type": "RAW"}),
        json!({"type": "STATE_DELTA"}),
    ] {
        assert!(
            serde_json::from_value::<Event>(bad.clone()).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn envelope_members_are_reachable_on_every_variant() {
    for fixture in support::full_events() {
        let event: Event = serde_json::from_value(fixture).unwrap();
        let base = event.base();
        assert_eq!(base.timestamp, Some(1_700_000_000_123));
        assert!(base.raw_event.is_some());
        assert!(base.metadata.is_some());
    }
    let mut event: Event =
        serde_json::from_value(json!({"type": "TEXT_MESSAGE_END", "messageId": "m"})).unwrap();
    event.base_mut().timestamp = Some(-5);
    assert_eq!(
        serde_json::to_value(&event).unwrap()["timestamp"],
        json!(-5)
    );
}

#[test]
fn attribution_is_read_from_the_right_member() {
    let cases = [
        (
            json!({"type": "TEXT_MESSAGE_END", "messageId": "m", "subagentRunId": "sub-1"}),
            Some("sub-1"),
        ),
        (json!({"type": "TEXT_MESSAGE_END", "messageId": "m"}), None),
        // Names an invocation rather than belonging to one.
        (
            json!({"type": "SUBAGENT_STARTED", "subagentRunId": "sub-1", "name": "n"}),
            None,
        ),
        (
            json!({"type": "RUN_STARTED", "threadId": "t", "runId": "r"}),
            None,
        ),
    ];
    for (json, want) in cases {
        let event: Event = serde_json::from_value(json).unwrap();
        assert_eq!(event.attributed_to().map(|s| s.as_str()), want);
    }
}

#[test]
fn messages_round_trip_for_every_role() {
    for (name, wire) in [
        ("full", support::full_messages()),
        ("minimal", support::minimal_messages()),
    ] {
        let messages: Vec<Message> = serde_json::from_value(wire.clone()).unwrap();
        let roles: BTreeSet<&str> = messages.iter().map(Message::role).collect();
        assert_eq!(
            roles,
            BTreeSet::from([
                "developer",
                "system",
                "assistant",
                "user",
                "tool",
                "activity",
                "reasoning"
            ]),
            "{name} covers every role"
        );
        assert_eq!(serde_json::to_value(&messages).unwrap(), wire, "{name}");
        for (m, w) in messages.iter().zip(wire.as_array().unwrap()) {
            assert_eq!(m.id().as_str(), w["id"].as_str().unwrap());
            assert_eq!(m.role(), w["role"].as_str().unwrap());
        }
    }
}

#[test]
fn an_unknown_message_role_is_malformed() {
    let err =
        serde_json::from_value::<Message>(json!({"id": "x", "role": "narrator", "content": ""}));
    assert!(err.is_err());
}

#[test]
fn run_agent_input_round_trips_exactly() {
    for wire in [support::full_input(), support::minimal_input()] {
        let input: RunAgentInput = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&input).unwrap(), wire);
    }
}

#[test]
fn optional_lists_read_as_empty_when_absent() {
    let input: RunAgentInput = serde_json::from_value(support::minimal_input()).unwrap();
    assert!(input.tools().is_empty() && input.context().is_empty() && input.resume().is_empty());
    let full: RunAgentInput = serde_json::from_value(support::full_input()).unwrap();
    assert_eq!(
        (
            full.tools().len(),
            full.context().len(),
            full.resume().len()
        ),
        (2, 1, 2)
    );
}

#[test]
fn json_patch_null_values_survive_and_pointers_are_checked() {
    let op: JsonPatchOperation =
        serde_json::from_value(json!({"op": "add", "path": "/a", "value": null})).unwrap();
    assert_eq!(
        op,
        JsonPatchOperation::Add {
            path: JsonPointer::new("/a").unwrap(),
            value: Value::Null
        }
    );
    assert_eq!(
        serde_json::to_value(&op).unwrap(),
        json!({"op": "add", "path": "/a", "value": null})
    );

    // `add` without `value` is malformed, not an add of null.
    assert!(
        serde_json::from_value::<JsonPatchOperation>(json!({"op": "add", "path": "/a"})).is_err()
    );

    for ok in ["", "/", "/a/b", "/a~0b", "/a~1b", "//"] {
        assert!(JsonPointer::new(ok).is_ok(), "{ok:?}");
    }
    for bad in ["a", "a/b", "/a~", "/a~2", "/~"] {
        assert!(JsonPointer::new(bad).is_err(), "{bad:?}");
        assert!(
            serde_json::from_value::<JsonPointer>(json!(bad)).is_err(),
            "{bad:?}"
        );
    }
}

#[test]
fn ids_are_plain_strings_on_the_wire() {
    let event: Event =
        serde_json::from_value(json!({"type": "RUN_STARTED", "threadId": "t", "runId": "r"}))
            .unwrap();
    let Event::RunStarted(started) = event else {
        panic!("wrong variant")
    };
    assert_eq!(started.thread_id.as_str(), "t");
    assert_eq!(started.run_id.to_string(), "r");
}
