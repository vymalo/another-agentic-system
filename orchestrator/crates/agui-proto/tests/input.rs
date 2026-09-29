//! Inbound `RunAgentInput`: unknown members are dropped and reported, malformed input is
//! rejected.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_proto::testkit::assert_input_conforms;
use orch_agui_proto::{InputError, Message, MessageContent, RunAgentInput};
use serde_json::{Value, json};

fn parse(value: &Value) -> orch_agui_proto::ParsedInput {
    RunAgentInput::parse(value.to_string().as_bytes()).unwrap()
}

#[test]
fn a_clean_input_drops_nothing() {
    for wire in [support::full_input(), support::minimal_input()] {
        let parsed = parse(&wire);
        assert!(parsed.dropped.is_empty(), "{:?}", parsed.dropped);
        assert_eq!(serde_json::to_value(&parsed.input).unwrap(), wire);
    }
}

#[test]
fn unknown_members_are_stripped_from_the_typed_view_and_reported() {
    let mut wire = support::full_input();
    wire["extraTop"] = json!({"a": 1});
    wire["messages"][3]["extraOnUser"] = json!(true);
    wire["messages"][4]["content"][1]["extraOnPart"] = json!("x");
    wire["messages"][4]["content"][1]["source"]["extraOnSource"] = json!(1);
    wire["messages"][2]["toolCalls"][0]["function"]["extraOnFunction"] = json!(null);
    wire["tools"][0]["extraOnTool"] = json!([]);
    wire["context"][0]["extraOnContext"] = json!(0);
    wire["resume"][1]["extraOnResume"] = json!({});

    let parsed = parse(&wire);
    let mut dropped = parsed.dropped.clone();
    dropped.sort();
    assert_eq!(
        dropped,
        [
            "context[0].extraOnContext",
            "extraTop",
            "messages[2].toolCalls[0].function.extraOnFunction",
            "messages[3].extraOnUser",
            "messages[4].content[1].extraOnPart",
            "messages[4].content[1].source.extraOnSource",
            "resume[1].extraOnResume",
            "tools[0].extraOnTool",
        ]
    );

    // The typed view is exactly the clean input: nothing unknown survives a re-serialise, and
    // what remains is schema-valid.
    let kept = serde_json::to_value(&parsed.input).unwrap();
    assert_eq!(kept, support::full_input());
    assert_input_conforms(&parsed.input);
}

#[test]
fn open_objects_are_kept_whole() {
    // `state`, `forwardedProps` and `metadata` are open by meaning: their keys are data.
    let wire = json!({
        "threadId": "t", "runId": "r",
        "state": {"anything": {"goes": [null, 1.5, "x"]}},
        "forwardedProps": {"vendor": {"nested": null}},
        "messages": [{"id": "m", "role": "user", "content": "hi", "metadata": {"whatever": null}}]
    });
    let parsed = parse(&wire);
    assert!(parsed.dropped.is_empty(), "{:?}", parsed.dropped);
    assert_eq!(serde_json::to_value(&parsed.input).unwrap(), wire);
}

#[test]
fn null_for_an_optional_member_is_dropped_not_kept() {
    let wire =
        json!({"threadId": "t", "runId": "r", "messages": [], "state": null, "resume": null});
    let parsed = parse(&wire);
    let mut dropped = parsed.dropped.clone();
    dropped.sort();
    assert_eq!(dropped, ["resume", "state"]);
    assert!(parsed.input.state.is_none() && parsed.input.resume.is_none());
    assert_eq!(
        serde_json::to_value(&parsed.input).unwrap(),
        support::minimal_input()
            .as_object()
            .map(|_| json!({"threadId": "t", "runId": "r", "messages": []}))
            .unwrap()
    );
}

#[test]
fn malformed_input_is_rejected() {
    let cases = [
        ("no threadId", json!({"runId": "r", "messages": []})),
        ("no runId", json!({"threadId": "t", "messages": []})),
        ("no messages", json!({"threadId": "t", "runId": "r"})),
        (
            "messages not a list",
            json!({"threadId": "t", "runId": "r", "messages": {}}),
        ),
        (
            "threadId not a string",
            json!({"threadId": 7, "runId": "r", "messages": []}),
        ),
        (
            "unknown role",
            json!({"threadId": "t", "runId": "r", "messages": [{"id": "m", "role": "narrator", "content": ""}]}),
        ),
        (
            "user message without id",
            json!({"threadId": "t", "runId": "r", "messages": [{"role": "user", "content": ""}]}),
        ),
        (
            "bad resume status",
            json!({"threadId": "t", "runId": "r", "messages": [], "resume": [{"interruptId": "i", "status": "maybe"}]}),
        ),
        (
            "tool call of another kind",
            json!({"threadId": "t", "runId": "r", "messages": [{"id": "m", "role": "assistant", "toolCalls": [{"id": "c", "type": "mcp", "function": {"name": "n", "arguments": ""}}]}]}),
        ),
        ("not an object", json!([1, 2])),
    ];
    for (why, wire) in cases {
        let err = RunAgentInput::parse(wire.to_string().as_bytes()).unwrap_err();
        assert!(matches!(err, InputError::Malformed(_)), "{why}: {err:?}");
    }
}

#[test]
fn bytes_that_are_not_json_are_a_different_error() {
    let err = RunAgentInput::parse(b"{not json").unwrap_err();
    assert!(matches!(err, InputError::NotJson(_)), "{err:?}");
}

#[test]
fn multimodal_and_text_user_content_both_parse() {
    let parsed = parse(&json!({
        "threadId": "t", "runId": "r",
        "messages": [
            {"id": "a", "role": "user", "content": "plain"},
            {"id": "b", "role": "user", "content": [{"type": "text", "text": "parts"}]}
        ]
    }));
    let contents: Vec<&MessageContent> = parsed
        .input
        .messages
        .iter()
        .map(|m| match m {
            Message::User(u) => &u.content,
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert!(matches!(contents[0], MessageContent::Text(t) if t == "plain"));
    assert!(matches!(contents[1], MessageContent::Parts(p) if p.len() == 1));
}
