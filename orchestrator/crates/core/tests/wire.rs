//! Wire-format tests: JSON must match the contract schemas exactly.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;

use orch_core::*;
use serde_json::json;
use uuid::Uuid;

fn tid() -> ThreadId {
    ThreadId(Uuid::parse_str("0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000").unwrap())
}

fn event(body: EventBody, actor: Actor) -> Event {
    Event {
        seq: 3,
        thread_id: tid(),
        at: "2026-09-29T10:00:00.123456Z".parse().unwrap(),
        actor,
        body,
    }
}

#[test]
fn event_json_is_exactly_the_contract_shape() {
    let e = event(
        EventBody::AgentStatus(AgentStatusData {
            status: AgentStatus::InputRequired,
            detail: None,
        }),
        Actor::agent(&AgentId::new("coder"), Some("rev-2".into())),
    );
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(
        v,
        json!({
            "seq": 3,
            "threadId": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "at": "2026-09-29T10:00:00.123456Z",
            "kind": "agent_status",
            "actor": {"type": "agent", "name": "coder", "revision": "rev-2"},
            "data": {"status": "input_required"}
        })
    );
    let back: Event = serde_json::from_value(v).unwrap();
    assert_eq!(back, e);
}

#[test]
fn every_kind_roundtrips_and_never_emits_null() {
    let bodies = vec![
        EventBody::UserMessage(UserMessageData::new("hi")),
        EventBody::AgentMessage(AgentMessageData {
            text: "t".into(),
            message_id: "m".into(),
            is_final: false,
        }),
        EventBody::AgentStatus(AgentStatusData {
            status: AgentStatus::Canceled,
            detail: Some("d".into()),
        }),
        EventBody::Artifact(ArtifactData {
            name: "pr".into(),
            mime_type: None,
            uri: Some("https://x".into()),
            text: None,
        }),
        EventBody::ThreadState(ThreadStateData {
            state: ThreadState::Cancelled,
        }),
        EventBody::Error(ErrorData {
            message: "m".into(),
            retryable: true,
        }),
    ];
    for body in bodies {
        let e = event(body, Actor::system());
        let text = serde_json::to_string(&e).unwrap();
        assert!(!text.contains("null"), "{text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["kind"], e.kind().as_str());
        assert_eq!(
            v["actor"],
            json!({"type": "system", "name": "orchestrator"})
        );
        let back: Event = serde_json::from_str(&text).unwrap();
        assert_eq!(back, e);
    }
}

#[test]
fn agent_message_uses_final_and_message_id() {
    let e = event(
        EventBody::AgentMessage(AgentMessageData {
            text: "t".into(),
            message_id: "m1".into(),
            is_final: true,
        }),
        Actor::system(),
    );
    assert_eq!(
        serde_json::to_value(&e).unwrap()["data"],
        json!({"text": "t", "messageId": "m1", "final": true})
    );
}

#[test]
fn artifact_uses_camel_case_mime_type() {
    let e = event(
        EventBody::Artifact(ArtifactData {
            name: "patch".into(),
            mime_type: Some("text/x-diff".into()),
            uri: None,
            text: Some("diff".into()),
        }),
        Actor::system(),
    );
    assert_eq!(
        serde_json::to_value(&e).unwrap()["data"],
        json!({"name": "patch", "mimeType": "text/x-diff", "text": "diff"})
    );
}

#[test]
fn thread_wire_hides_owner_and_version() {
    let t = ThreadRecord {
        id: tid(),
        owner: UserId::new("a@b.c"),
        title: "T".into(),
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        state: ThreadState::Working,
        version: 7,
        last_seq: 2,
        created_at: "2026-09-29T10:00:00Z".parse().unwrap(),
        updated_at: "2026-09-29T10:00:01Z".parse().unwrap(),
    };
    assert_eq!(
        serde_json::to_value(&t).unwrap(),
        json!({
            "id": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "title": "T",
            "target": {"agentId": "coder"},
            "state": "working",
            "lastSeq": 2,
            "createdAt": "2026-09-29T10:00:00Z",
            "updatedAt": "2026-09-29T10:00:01Z"
        })
    );
}

#[test]
fn releases_and_target_wire() {
    let r = Releases {
        default_channel: "stable".into(),
        channels: BTreeMap::from([("stable".to_owned(), "rev-1".to_owned())]),
        revisions: None,
    };
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({"defaultChannel": "stable", "channels": {"stable": "rev-1"}})
    );
    assert!(r.accepts("stable"));
    assert!(!r.accepts("rev-1"));
    assert_eq!(r.resolve("stable").as_deref(), Some("rev-1"));
    let t: AgentTarget = serde_json::from_value(json!({"agentId": "a", "release": "x"})).unwrap();
    assert_eq!(t.release.as_deref(), Some("x"));
}

#[test]
fn agent_info_card_url_is_optional_and_absent_when_none() {
    let mut info = AgentInfo {
        id: AgentId::new("coder"),
        name: "Coder".into(),
        description: None,
        card_url: Some("https://coder.example.com/.well-known/agent-card.json".into()),
        releases: None,
    };
    assert_eq!(
        serde_json::to_value(&info).unwrap(),
        json!({
            "id": "coder",
            "name": "Coder",
            "cardUrl": "https://coder.example.com/.well-known/agent-card.json"
        })
    );
    info.card_url = None;
    let wire = serde_json::to_value(&info).unwrap();
    assert_eq!(wire, json!({"id": "coder", "name": "Coder"}));
    // And a client that never learned the key reads it back.
    let back: AgentInfo = serde_json::from_value(wire).unwrap();
    assert_eq!(back, info);
}

#[test]
fn thread_state_spelling() {
    for (s, w) in [
        (ThreadState::Queued, "queued"),
        (ThreadState::Working, "working"),
        (ThreadState::Blocked, "blocked"),
        (ThreadState::Done, "done"),
        (ThreadState::Failed, "failed"),
        (ThreadState::Cancelled, "cancelled"),
    ] {
        assert_eq!(serde_json::to_value(s).unwrap(), json!(w));
        assert_eq!(s.as_str(), w);
    }
}

#[test]
fn user_id_is_normalised() {
    assert_eq!(UserId::new("  A@B.Com ").as_str(), "a@b.com");
}

#[test]
fn user_message_ids_are_optional_camel_case_and_absent_when_none() {
    let plain = event(
        EventBody::UserMessage(UserMessageData::new("hi")),
        Actor::system(),
    );
    assert_eq!(
        serde_json::to_value(&plain).unwrap()["data"],
        json!({"text": "hi"}),
        "no null, no empty member"
    );

    let named = event(
        EventBody::UserMessage(UserMessageData {
            text: "hi".into(),
            message_id: Some("msg-1".into()),
            run_id: Some("run-1".into()),
        }),
        Actor::system(),
    );
    let v = serde_json::to_value(&named).unwrap();
    assert_eq!(
        v["data"],
        json!({"text": "hi", "messageId": "msg-1", "runId": "run-1"})
    );
    assert_eq!(serde_json::from_value::<Event>(v).unwrap(), named);

    // Only one of the two, and a log written before these fields existed, both read back.
    let only_run = json!({"text": "hi", "runId": "run-1"});
    let d: UserMessageData = serde_json::from_value(only_run.clone()).unwrap();
    assert_eq!(
        (d.message_id.as_deref(), d.run_id.as_deref()),
        (None, Some("run-1"))
    );
    assert_eq!(serde_json::to_value(&d).unwrap(), only_run);
    let old: UserMessageData = serde_json::from_value(json!({"text": "hi"})).unwrap();
    assert_eq!(old, UserMessageData::new("hi"));
}

#[test]
fn auth_required_is_its_own_status_spelling() {
    let e = event(
        EventBody::AgentStatus(AgentStatusData {
            status: AgentStatus::AuthRequired,
            detail: Some("github".into()),
        }),
        Actor::agent(&AgentId::new("coder"), None),
    );
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(
        v["data"],
        json!({"status": "auth_required", "detail": "github"})
    );
    assert_eq!(serde_json::from_value::<Event>(v).unwrap(), e);
}
