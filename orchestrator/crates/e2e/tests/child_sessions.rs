//! `child-sessions/v1` as far as S1 builds it (ADR 0072, `docs/api/child-sessions-v1.md`), on both
//! stores: the live card is read into the capabilities document, well formed or not at all, and
//! never cached.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use common::*;
use orch_core::CHILD_SESSIONS_EXTENSION;
use orch_testsupport::FakeAgentOptions;
use serde_json::{Value, json};

fn params() -> Value {
    json!({
        "agents": [
            {"name": "planner", "description": "Breaks a request into steps."},
            {"name": "researcher", "description": "Looks things up."}
        ],
        "maxOpen": 16
    })
}

/// A world whose `plain` agent lists `child-sessions/v1` and whose `coder` does not.
async fn world(backend: Backend) -> World {
    World::with(
        backend,
        Setup {
            plain: FakeAgentOptions {
                child_sessions: Some(params()),
                ..FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await
}

async fn the_capabilities_document_carries_the_helpers_of_the_live_card(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let (status, doc) = chat.agui_capabilities("plain").await;
    assert_eq!(status, 200);
    assert_eq!(doc["custom"][CHILD_SESSIONS_EXTENSION], params());
    // an agent that does not list it has no key
    let (status, doc) = chat.agui_capabilities("coder").await;
    assert_eq!(status, 200);
    assert!(
        doc.get("custom")
            .is_none_or(|c| c.get(CHILD_SESSIONS_EXTENSION).is_none())
    );
    // a card that lists it badly offers nothing (fail closed)
    world
        .plain
        .set_child_sessions(Some(json!({"agents": [], "maxOpen": 16})));
    let (_, doc) = chat.agui_capabilities("plain").await;
    assert!(
        doc.get("custom")
            .is_none_or(|c| c.get(CHILD_SESSIONS_EXTENSION).is_none())
    );
    // the card is read live: a change shows on the next read
    world.plain.set_child_sessions(Some(json!({
        "agents": [{"name": "writer", "description": ""}],
        "maxOpen": 2
    })));
    let (_, doc) = chat.agui_capabilities("plain").await;
    assert_eq!(
        doc["custom"][CHILD_SESSIONS_EXTENSION],
        json!({"agents": [{"name": "writer", "description": ""}], "maxOpen": 2})
    );
    world.plain.set_child_sessions(None);
    let (_, doc) = chat.agui_capabilities("plain").await;
    assert!(
        doc.get("custom")
            .is_none_or(|c| c.get(CHILD_SESSIONS_EXTENSION).is_none())
    );
}

backends!(the_capabilities_document_carries_the_helpers_of_the_live_card);
