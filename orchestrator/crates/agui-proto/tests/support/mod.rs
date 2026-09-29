//! Wire fixtures: one literal JSON document per event type and per message role.
//!
//! `full_events` sets every optional member; `minimal_events` sets only the required ones.
//! They are written as wire JSON, not built with the typed API, so a serde rename that drifts
//! from the schema fails a test instead of round-tripping silently.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use serde_json::{Value, json};

/// Adds the envelope members every event may carry.
fn enveloped(mut event: Value) -> Value {
    let obj = event.as_object_mut().unwrap();
    obj.insert("timestamp".into(), json!(1_700_000_000_123_i64));
    obj.insert("rawEvent".into(), json!({"native": ["a", 1, true]}));
    obj.insert(
        "metadata".into(),
        json!({"vymalo.actor": {"type": "agent", "name": "plain"}, "nullable": null}),
    );
    event
}

pub fn full_input() -> Value {
    json!({
        "threadId": "t-1",
        "runId": "r-2",
        "protocolVersion": "1.0",
        "parentRunId": "r-1",
        "state": {"thread": {"state": "queued"}},
        "messages": full_messages(),
        "tools": [
            {"name": "pick", "description": "Pick one", "parameters": {"type": "object"}, "metadata": {"k": "v"}},
            {"name": "noop", "description": "Does nothing"}
        ],
        "context": [{"description": "locale", "value": "de-DE"}],
        "forwardedProps": {"https://agents.vymalo.com/a2a/extensions/release-channels/v1": {"release": "stable"}},
        "resume": [
            {"interruptId": "int-3", "status": "resolved", "payload": {"text": "main"}, "metadata": {"sig": "x"}},
            {"interruptId": "int-4", "status": "cancelled"}
        ]
    })
}

pub fn minimal_input() -> Value {
    json!({"threadId": "t-1", "runId": "r-1", "messages": []})
}

/// One message per role, every optional member set.
pub fn full_messages() -> Value {
    json!([
        {"id": "m-dev", "role": "developer", "content": "be brief", "name": "ops",
         "encryptedValue": "enc", "metadata": {"k": 1}, "subagentRunId": "sub-1"},
        {"id": "m-sys", "role": "system", "content": "you are helpful", "name": "root",
         "encryptedValue": "enc", "metadata": {}, "subagentRunId": "sub-1"},
        {"id": "m-asst", "role": "assistant", "content": "calling", "name": "plain",
         "toolCalls": [
             {"id": "tc-1", "type": "function", "function": {"name": "pick", "arguments": "{\"n\":1}"},
              "encryptedValue": "enc", "metadata": {"k": true}},
             {"id": "tc-2", "type": "function", "function": {"name": "noop", "arguments": "{}"}}
         ],
         "encryptedValue": "enc", "metadata": {"k": null}, "subagentRunId": "sub-2"},
        {"id": "m-user", "role": "user", "content": "hello", "name": "alice",
         "encryptedValue": "enc", "metadata": {"vymalo.actor": {"type": "user"}}, "subagentRunId": "sub-3"},
        {"id": "m-user-parts", "role": "user", "content": [
            {"type": "text", "id": "p1", "text": "look", "metadata": {"src": "web"}},
            {"type": "image", "id": "p2", "source": {"type": "url", "value": "https://e.example/a.png", "mimeType": "image/png"}, "metadata": 1},
            {"type": "audio", "source": {"type": "data", "value": "AAAA", "mimeType": "audio/wav"}},
            {"type": "video", "source": {"type": "file", "value": "file-1", "provider": "acme", "mimeType": "video/mp4"}},
            {"type": "document", "source": {"type": "url", "value": "https://e.example/a.pdf"}},
            {"type": "document", "source": {"type": "file", "value": "file-2"}}
        ]},
        {"id": "m-tool", "role": "tool", "content": "ok", "toolCallId": "tc-1", "error": "boom",
         "encryptedValue": "enc", "metadata": {"k": 2}, "subagentRunId": "sub-2"},
        {"id": "m-tool-parts", "role": "tool", "toolCallId": "tc-2",
         "content": [{"type": "text", "text": "done"}]},
        {"id": "m-act", "role": "activity", "activityType": "vymalo.status",
         "content": {"status": "working", "detail": null}, "metadata": {"k": 3}, "subagentRunId": "sub-2"},
        {"id": "m-rsn", "role": "reasoning", "content": "thinking", "encryptedValue": "enc",
         "metadata": {"k": 4}, "subagentRunId": "sub-2"}
    ])
}

/// One message per role, required members only.
pub fn minimal_messages() -> Value {
    json!([
        {"id": "a", "role": "developer", "content": ""},
        {"id": "b", "role": "system", "content": ""},
        {"id": "c", "role": "assistant"},
        {"id": "d", "role": "user", "content": ""},
        {"id": "e", "role": "tool", "content": "", "toolCallId": "tc"},
        {"id": "f", "role": "activity", "activityType": "x", "content": {}},
        {"id": "g", "role": "reasoning", "content": ""}
    ])
}

fn usage_full() -> Value {
    json!([{
        "provider": "acme", "model": "m-1", "inputTokens": 10, "outputTokens": 5, "totalTokens": 15,
        "reasoningTokens": 2, "cachedInputTokens": 3, "cacheWriteInputTokens": 1
    }, {}])
}

fn patch_full() -> Value {
    json!([
        {"op": "add", "path": "/a", "value": null},
        {"op": "remove", "path": "/b"},
        {"op": "replace", "path": "/c/0", "value": {"x": [1, 2]}},
        {"op": "move", "from": "/d", "path": "/e"},
        {"op": "copy", "from": "/f", "path": ""},
        {"op": "test", "path": "/g~0h~1i", "value": "s"}
    ])
}

/// One event per type, every optional member set (and the envelope on all of them).
pub fn full_events() -> Vec<Value> {
    let raw = vec![
        json!({"type": "TEXT_MESSAGE_START", "messageId": "m1", "role": "user", "name": "alice", "subagentRunId": "sub-1"}),
        json!({"type": "TEXT_MESSAGE_CONTENT", "messageId": "m1", "delta": "hi", "subagentRunId": "sub-1"}),
        json!({"type": "TEXT_MESSAGE_END", "messageId": "m1", "subagentRunId": "sub-1"}),
        json!({"type": "TEXT_MESSAGE_CHUNK", "messageId": "m2", "role": "assistant", "delta": "yo", "name": "plain", "subagentRunId": "sub-1"}),
        json!({"type": "TOOL_CALL_START", "toolCallId": "tc-1", "toolCallName": "pick", "parentMessageId": "m2", "subagentRunId": "sub-1"}),
        json!({"type": "TOOL_CALL_ARGS", "toolCallId": "tc-1", "delta": "{\"n\":", "subagentRunId": "sub-1"}),
        json!({"type": "TOOL_CALL_END", "toolCallId": "tc-1", "subagentRunId": "sub-1"}),
        json!({"type": "TOOL_CALL_CHUNK", "toolCallId": "tc-2", "toolCallName": "noop", "parentMessageId": "m2", "delta": "{}", "subagentRunId": "sub-1"}),
        json!({"type": "TOOL_CALL_RESULT", "messageId": "m3", "toolCallId": "tc-1", "content": "ok", "role": "tool", "subagentRunId": "sub-1"}),
        json!({"type": "TOOL_CALL_RESULT", "messageId": "m4", "toolCallId": "tc-2", "content": [{"type": "text", "text": "ok"}], "role": "tool"}),
        json!({"type": "STATE_SNAPSHOT", "snapshot": {"thread": {"state": "working", "target": {"agentId": "plain"}}}, "subagentRunId": "sub-1"}),
        json!({"type": "STATE_DELTA", "delta": patch_full(), "subagentRunId": "sub-1"}),
        json!({"type": "MESSAGES_SNAPSHOT", "messages": full_messages()}),
        json!({"type": "ACTIVITY_SNAPSHOT", "messageId": "evt-2", "activityType": "vymalo.status", "content": {"status": "working"}, "replace": false, "subagentRunId": "sub-1"}),
        json!({"type": "ACTIVITY_DELTA", "messageId": "evt-2", "activityType": "vymalo.status", "patch": patch_full(), "subagentRunId": "sub-1"}),
        json!({"type": "RAW", "event": {"provider": "x"}, "source": "acme", "subagentRunId": "sub-1"}),
        json!({"type": "CUSTOM", "name": "vymalo.ping", "value": [1, "two", null], "subagentRunId": "sub-1"}),
        json!({"type": "RUN_STARTED", "threadId": "t-1", "runId": "r-2", "protocolVersion": "1.0", "parentRunId": "r-1", "input": full_input()}),
        json!({"type": "RUN_FINISHED", "threadId": "t-1", "runId": "r-2", "result": {"pr": 1}, "outcome": {"type": "success", "pendingToolCallIds": ["tc-1"]}, "usage": usage_full()}),
        json!({"type": "RUN_FINISHED", "threadId": "t-1", "runId": "r-3", "outcome": {"type": "interrupt", "interrupts": [{
            "id": "int-3", "reason": "input_required", "message": "Which branch?", "toolCallId": "tc-1",
            "responseSchema": {"type": "object", "required": ["text"]}, "expiresAt": "2026-10-01T00:00:00Z",
            "metadata": {"k": 1}, "subagentRunId": "sub-1"}]}}),
        json!({"type": "RUN_FINISHED", "threadId": "t-1", "runId": "r-4", "outcome": {"type": "cancelled"}}),
        json!({"type": "RUN_ERROR", "message": "the agent failed", "code": "agent_failed", "usage": usage_full()}),
        json!({"type": "STEP_STARTED", "stepName": "plan", "subagentRunId": "sub-1"}),
        json!({"type": "STEP_FINISHED", "stepName": "plan", "subagentRunId": "sub-1"}),
        json!({"type": "REASONING_START", "messageId": "rs-1", "subagentRunId": "sub-1"}),
        json!({"type": "REASONING_MESSAGE_START", "messageId": "rs-1", "role": "reasoning", "subagentRunId": "sub-1"}),
        json!({"type": "REASONING_MESSAGE_CONTENT", "messageId": "rs-1", "delta": "hmm", "subagentRunId": "sub-1"}),
        json!({"type": "REASONING_MESSAGE_END", "messageId": "rs-1", "subagentRunId": "sub-1"}),
        json!({"type": "REASONING_MESSAGE_CHUNK", "messageId": "rs-2", "delta": "ok", "subagentRunId": "sub-1"}),
        json!({"type": "REASONING_END", "messageId": "rs-1", "subagentRunId": "sub-1"}),
        json!({"type": "REASONING_ENCRYPTED_VALUE", "subtype": "tool-call", "entityId": "tc-1", "encryptedValue": "enc", "subagentRunId": "sub-1"}),
        json!({"type": "REASONING_ENCRYPTED_VALUE", "subtype": "message", "entityId": "rs-1", "encryptedValue": "enc"}),
        json!({"type": "SUBAGENT_STARTED", "subagentRunId": "sub-2", "name": "plain", "description": "answers", "parentSubagentRunId": "sub-1", "parentToolCallId": "tc-1", "parentMessageId": "m2"}),
        json!({"type": "SUBAGENT_FINISHED", "subagentRunId": "sub-2", "result": {"status": "canceled"}, "outcome": {"type": "success"}}),
        json!({"type": "SUBAGENT_FINISHED", "subagentRunId": "sub-3", "outcome": {"type": "suspended", "interruptIds": ["int-3"]}}),
        json!({"type": "SUBAGENT_ERROR", "subagentRunId": "sub-2", "message": "delivery failed", "code": "delivery_failed"}),
    ];
    raw.into_iter().map(enveloped).collect()
}

/// One event per type, required members only, no envelope.
pub fn minimal_events() -> Vec<Value> {
    vec![
        json!({"type": "TEXT_MESSAGE_START", "messageId": "m1"}),
        json!({"type": "TEXT_MESSAGE_CONTENT", "messageId": "m1", "delta": ""}),
        json!({"type": "TEXT_MESSAGE_END", "messageId": "m1"}),
        json!({"type": "TEXT_MESSAGE_CHUNK"}),
        json!({"type": "TOOL_CALL_START", "toolCallId": "tc", "toolCallName": "n"}),
        json!({"type": "TOOL_CALL_ARGS", "toolCallId": "tc", "delta": ""}),
        json!({"type": "TOOL_CALL_END", "toolCallId": "tc"}),
        json!({"type": "TOOL_CALL_CHUNK"}),
        json!({"type": "TOOL_CALL_RESULT", "messageId": "m", "toolCallId": "tc", "content": ""}),
        json!({"type": "STATE_SNAPSHOT", "snapshot": {}}),
        json!({"type": "STATE_DELTA", "delta": []}),
        json!({"type": "MESSAGES_SNAPSHOT", "messages": minimal_messages()}),
        json!({"type": "ACTIVITY_SNAPSHOT", "messageId": "m", "activityType": "x", "content": {}}),
        json!({"type": "ACTIVITY_DELTA", "messageId": "m", "activityType": "x", "patch": []}),
        json!({"type": "RAW", "event": 1}),
        json!({"type": "CUSTOM", "name": "n", "value": 1}),
        json!({"type": "RUN_STARTED", "threadId": "t", "runId": "r"}),
        json!({"type": "RUN_FINISHED", "threadId": "t", "runId": "r"}),
        json!({"type": "RUN_ERROR", "message": "m"}),
        json!({"type": "STEP_STARTED", "stepName": "s"}),
        json!({"type": "STEP_FINISHED", "stepName": "s"}),
        json!({"type": "REASONING_START", "messageId": "m"}),
        json!({"type": "REASONING_MESSAGE_START", "messageId": "m", "role": "reasoning"}),
        json!({"type": "REASONING_MESSAGE_CONTENT", "messageId": "m", "delta": ""}),
        json!({"type": "REASONING_MESSAGE_END", "messageId": "m"}),
        json!({"type": "REASONING_MESSAGE_CHUNK"}),
        json!({"type": "REASONING_END", "messageId": "m"}),
        json!({"type": "REASONING_ENCRYPTED_VALUE", "subtype": "message", "entityId": "e", "encryptedValue": "v"}),
        json!({"type": "SUBAGENT_STARTED", "subagentRunId": "s", "name": "n"}),
        json!({"type": "SUBAGENT_FINISHED", "subagentRunId": "s"}),
        json!({"type": "SUBAGENT_ERROR", "subagentRunId": "s", "message": "m"}),
    ]
}

/// True when `value` contains a JSON `null` anywhere except under a `metadata`, `content` (of
/// an activity), `value`, `snapshot`, `event` or `payload` key, where arbitrary data may
/// legitimately carry one.
pub fn has_structural_null(value: &Value) -> bool {
    const OPEN: [&str; 9] = [
        "metadata",
        "content",
        "value",
        "snapshot",
        "event",
        "payload",
        "state",
        "forwardedProps",
        "rawEvent",
    ];
    match value {
        Value::Null => true,
        Value::Array(items) => items.iter().any(has_structural_null),
        Value::Object(map) => map
            .iter()
            .any(|(k, v)| !OPEN.contains(&k.as_str()) && has_structural_null(v)),
        _ => false,
    }
}
