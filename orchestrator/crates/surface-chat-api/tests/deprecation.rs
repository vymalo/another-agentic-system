//! The deprecation contract (RFC 9745): the four legacy operations, and only they, answer with
//! `Deprecation: @<unix seconds>`. `conformance.rs` also checks the header on every response it
//! drives; this file pins the set and the edges that file cannot reach.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use serde_json::json;
use support::contract::Contract;
use support::*;

const RANDOM: &str = "0190aaaa-0000-7000-8000-000000000123";
const LEGACY: [&str; 4] = ["createThread", "postMessage", "listEvents", "streamEvents"];

#[test]
fn the_contract_deprecates_exactly_the_four_legacy_operations() {
    let contract = Contract::load();
    let deprecated: BTreeSet<String> = contract
        .operations()
        .into_iter()
        .map(|(id, _, _)| id)
        .filter(|id| contract.is_deprecated(id))
        .collect();
    let want: BTreeSet<String> = LEGACY.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(deprecated, want);
}

#[test]
fn the_header_is_an_sf_date_of_the_deprecation_day() {
    assert_eq!(
        format!("@{}", orch_surface_chat_api::DEPRECATED_AT),
        orch_surface_chat_api::DEPRECATION
    );
    let day = jiff::civil::date(2026, 9, 29)
        .in_tz("UTC")
        .unwrap()
        .timestamp()
        .as_second();
    assert_eq!(orch_surface_chat_api::DEPRECATED_AT, day);
    // RFC 9651 sf-date: `@` and an integer.
    let digits = orch_surface_chat_api::DEPRECATION
        .strip_prefix('@')
        .unwrap();
    assert!(digits.chars().all(|c| c.is_ascii_digit()) && !digits.is_empty());
}

#[tokio::test]
async fn only_the_legacy_operations_send_deprecation() {
    let h = Harness::start().await;
    let thread = h.create(ALICE, "plain", "echo hello").await;
    h.wait_state(ALICE, &thread, "done").await;

    // The four operations, on success and on their own errors.
    let legacy = [
        (
            "createThread 201",
            h.post(
                "/api/threads",
                Some(ALICE),
                json!({"target": {"agentId": "plain"}, "text": "echo hi"}),
            )
            .await,
        ),
        (
            "createThread 400",
            h.post("/api/threads", Some(ALICE), json!({"text": "no target"}))
                .await,
        ),
        (
            "listEvents 200",
            h.get(&format!("/api/threads/{thread}/events"), Some(ALICE))
                .await,
        ),
        (
            "listEvents 404",
            h.get(&format!("/api/threads/{RANDOM}/events"), Some(ALICE))
                .await,
        ),
        (
            "postMessage 409",
            h.post(
                &format!("/api/threads/{thread}/messages"),
                Some(ALICE),
                json!({"text": "late"}),
            )
            .await,
        ),
        (
            "postMessage 404",
            h.post(
                &format!("/api/threads/{RANDOM}/messages"),
                Some(ALICE),
                json!({"text": "x"}),
            )
            .await,
        ),
        (
            "streamEvents 404",
            h.get(&format!("/api/threads/{RANDOM}/stream"), Some(ALICE))
                .await,
        ),
    ];
    for (what, r) in &legacy {
        assert_eq!(
            r.deprecation.as_deref(),
            Some(orch_surface_chat_api::DEPRECATION),
            "{what}"
        );
    }
    let sse = h.stream(ALICE, &thread, None).await;
    assert_eq!(sse.status.as_u16(), 200);
    assert_eq!(
        sse.headers.get("deprecation").unwrap().to_str().unwrap(),
        orch_surface_chat_api::DEPRECATION
    );

    // Everything else: the resource API (including the method that shares a path with
    // createThread), health, an identity refusal, a route nobody serves.
    let others = [
        ("getHealth", h.get("/healthz", None).await),
        ("getReady", h.get("/readyz", None).await),
        ("listAgents", h.get("/api/agents", Some(ALICE)).await),
        ("listThreads", h.get("/api/threads", Some(ALICE)).await),
        (
            "getThread",
            h.get(&format!("/api/threads/{thread}"), Some(ALICE)).await,
        ),
        (
            "getThread 404",
            h.get(&format!("/api/threads/{RANDOM}"), Some(ALICE)).await,
        ),
        (
            "cancelThread",
            h.post_empty(&format!("/api/threads/{thread}/cancel"), Some(ALICE))
                .await,
        ),
        (
            "createThread without identity (401 is the shared identity layer's)",
            h.post("/api/threads", None, json!({})).await,
        ),
        (
            "listEvents without identity",
            h.get(&format!("/api/threads/{thread}/events"), None).await,
        ),
        ("an unknown route", h.get("/api/nothing", Some(ALICE)).await),
    ];
    for (what, r) in &others {
        assert_eq!(r.deprecation, None, "{what}");
    }
    assert_eq!(
        others[3].1.status, 200,
        "GET /api/threads is not the legacy POST"
    );
}
