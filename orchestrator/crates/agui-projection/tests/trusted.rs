//! A trusted component an agent asked the client to draw (ADR 0083, `docs/api/agui.md`): one
//! `vymalo.trustedRequest` card per request, with the component and the agent's bounded hints.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector, ThreadView};
use orch_agui_proto::testkit::assert_conforms;
use orch_core::{
    Actor, AgentId, Event, EventBody, GatePolicy, Timestamp, TrustedComponent, TrustedHints,
    TrustedRequestData, UserId,
};
use serde_json::{Value, json};
use support::log::{meta_under, thread_id};
use support::verify;

fn event(seq: i64, body: EventBody) -> Event {
    Event {
        seq,
        thread_id: thread_id(),
        at: "2026-01-01T00:00:00Z".parse::<Timestamp>().unwrap(),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body,
    }
}

fn cards(frames: &[Frame]) -> Vec<(String, Option<bool>, Value)> {
    frames
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(e)
                if e.activity_type == "vymalo.trustedRequest" =>
            {
                Some((
                    e.message_id.as_str().to_owned(),
                    e.replace,
                    Value::Object(e.content.clone()),
                ))
            }
            _ => None,
        })
        .collect()
}

fn request(hints: TrustedHints) -> EventBody {
    EventBody::TrustedRequest(TrustedRequestData {
        component: TrustedComponent::McpCredential,
        hints,
    })
}

fn project(events: &[Event]) -> Vec<Frame> {
    let mut projector = Projector::new(meta_under(GatePolicy::default()));
    let frames: Vec<Frame> = events
        .iter()
        .flat_map(|e| projector.apply(e, Audience::Viewer))
        .collect();
    for frame in &frames {
        assert_conforms(&frame.event);
    }
    verify::check(&frames).unwrap();
    frames
}

#[test]
fn a_request_is_a_card_with_the_component_and_the_hints_and_does_not_replace() {
    let ev = event(
        1,
        request(TrustedHints {
            name: Some("My search".into()),
            scopes: vec!["search.read".into()],
            expires_in_days: Some(30),
        }),
    );
    let frames = project(std::slice::from_ref(&ev));
    let cards = cards(&frames);
    assert_eq!(cards.len(), 1);
    let (id, replace, content) = &cards[0];
    assert_eq!(id, "trusted-1");
    assert_eq!(*replace, Some(false), "a request never replaces another");
    assert_eq!(
        content,
        &json!({
            "component": "McpCredential",
            "hints": {"name": "My search", "scopes": ["search.read"], "expiresInDays": 30},
            "at": ev.at.to_string(),
        })
    );
}

#[test]
fn empty_hints_are_an_empty_object_on_the_card() {
    let frames = project(&[event(2, request(TrustedHints::default()))]);
    let (_, _, content) = &cards(&frames)[0];
    assert_eq!(content["hints"], json!({}));
}

#[test]
fn two_requests_are_two_cards() {
    let frames = project(&[
        event(3, request(TrustedHints::default())),
        event(4, request(TrustedHints::default())),
    ]);
    let ids: Vec<String> = cards(&frames).into_iter().map(|(id, _, _)| id).collect();
    assert_eq!(ids, ["trusted-3", "trusted-4"]);
}

#[test]
fn the_view_holds_the_trusted_cards_the_log_has() {
    // The id the client's receipt names as `surfaceId` is the activity's own `messageId`, so the
    // view must hold exactly those (ADR 0083).
    let mut projector = Projector::new(meta_under(GatePolicy::default()));
    for ev in [
        event(6, request(TrustedHints::default())),
        event(7, request(TrustedHints::default())),
    ] {
        projector.apply(&ev, Audience::Viewer);
    }
    let ThreadView::Known(known) = projector.view(&UserId::new("alice@example.com")) else {
        panic!("a known thread");
    };
    assert_eq!(
        known.trusted_cards,
        ["trusted-6", "trusted-7"].map(str::to_owned).into()
    );
}

#[test]
fn the_card_is_attributed_to_the_agent_that_asked() {
    let frames = project(&[event(5, request(TrustedHints::default()))]);
    let snapshot = frames
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(e)
                if e.activity_type == "vymalo.trustedRequest" =>
            {
                Some(e)
            }
            _ => None,
        })
        .unwrap();
    let metadata = snapshot.base.metadata.as_ref().unwrap();
    assert_eq!(
        metadata.get("vymalo.actor"),
        Some(&json!({"type": "agent", "name": "coder"}))
    );
}
