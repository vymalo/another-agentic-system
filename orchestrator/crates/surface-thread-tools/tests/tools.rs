//! The endpoint's tools: what `tools/list` says, `get_ui_catalog` for a thread with and without a
//! catalog, and the provider seam later slices add tools through.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::{Caller, Input, Origin, ThreadId};
use serde_json::{Value, json};
use support::*;

async fn client_for(h: &Harness, thread: ThreadId, agent: &str) -> Client {
    connect(&h.url(thread), &grant_token(thread, agent)).await
}

#[tokio::test]
async fn tools_list_is_get_ui_catalog_with_its_schemas() {
    let h = Harness::start().await;
    let thread = h.thread("plain").await;
    let client = client_for(&h, thread, "plain").await;
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(
        tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(),
        ["get_ui_catalog", "turn_output"]
    );
    let tool = &tools[0];
    assert_eq!(tool.input_schema.get("type"), Some(&json!("object")));
    assert_eq!(
        tool.input_schema["properties"]["knownDigest"]["pattern"],
        "^sha256:[0-9a-f]{64}$"
    );
    assert_eq!(
        tool.input_schema.get("additionalProperties"),
        Some(&json!(false))
    );
    let output = tool.output_schema.as_ref().expect("an output schema");
    assert_eq!(
        output["required"],
        json!(["catalogId", "version", "digest", "unchanged"])
    );
    assert_eq!(
        tool.annotations.as_ref().and_then(|a| a.read_only_hint),
        Some(true)
    );
}

#[tokio::test]
async fn a_thread_with_no_catalog_answers_with_an_error_to_read() {
    let h = Harness::start().await;
    let thread = h.thread("plain").await;
    let client = client_for(&h, thread, "plain").await;
    let out = call(&client, "get_ui_catalog", json!({})).await;
    assert!(out.is_error, "{out:?}");
    assert_eq!(out.text, "this thread has no UI catalog; answer in text");
}

/// ADR 0075, sections 4 and 9: the **write is strict** — a `ui-catalog/v1` document is refused by
/// the door a run goes through — and a thread from before the cutover, whose log holds one all the
/// same, is **never served** it: a v1.0 reader refuses a v1 document, so the agent is told to
/// answer in words, exactly as for a thread that has no catalog.
#[tokio::test]
async fn a_catalog_from_before_the_cutover_is_never_served() {
    let h = Harness::start().await;
    let thread = h.thread("plain").await;
    let client = client_for(&h, thread, "plain").await;
    let old = catalog_v1(8, "before");
    // the write is strict: the run route refuses the document before anything is written
    let refused = h
        .app
        .submit(
            &orch_core::UserId::new(ALICE),
            thread,
            Input::UserMessage {
                tier_cap: None,
                locale: None,
                pick: None,
                user: orch_core::UserId::new(ALICE),
                text: "echo again".to_owned(),
                message_id: None,
                run_id: None,
                origin: Origin::Agui,
                catalog: Some(old.clone()),
                mentions: Vec::new(),
                tier: None,
            },
            None,
        )
        .await;
    assert!(
        matches!(&refused, Err(orch_app::AppError::Invalid(why)) if why.contains("protocolVersion")),
        "{refused:?}"
    );
    // and a log written before the cutover — through the dispatcher's door, which does not re-read
    // what a surface checked — keeps its v1 document, which is not served
    h.show_as_written_before_the_cutover(thread, &old).await;
    let out = call(&client, "get_ui_catalog", json!({})).await;
    assert!(out.is_error, "{out:?}");
    assert!(out.text.contains("before A2UI v1.0"), "{}", out.text);
    // even the agent that holds its digest is not served it
    let out = call(
        &client,
        "get_ui_catalog",
        json!({"knownDigest": old.digest}),
    )
    .await;
    assert!(out.is_error, "{out:?}");
    // a v2 catalog from the same screen is served as ever
    let new = catalog(9, "after");
    h.show(thread, &new).await;
    let out = call(&client, "get_ui_catalog", json!({})).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.value["version"], 9);
    assert_eq!(out.value["catalog"]["protocolVersion"], "1.0");
}

#[tokio::test]
async fn the_newest_catalog_is_given_and_a_known_digest_leaves_it_out() {
    let h = Harness::start().await;
    let thread = h.thread("plain").await;
    let client = client_for(&h, thread, "plain").await;
    let (v1, v2) = (catalog(1, "a"), catalog(2, "a"));

    h.show(thread, &v1).await;
    let out = call(&client, "get_ui_catalog", json!({})).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(
        out.value,
        json!({
            "catalogId": v1.catalog_id, "version": 1, "digest": v1.digest,
            "unchanged": false, "catalog": v1.catalog,
        })
    );
    // The same JSON is the text content, as MCP asks.
    assert_eq!(serde_json::from_str::<Value>(&out.text).unwrap(), out.value);

    // A newer version is recorded: the next call gives it (no state is kept in the process).
    h.show(thread, &v2).await;
    let out = call(&client, "get_ui_catalog", json!({})).await;
    assert_eq!(out.value["version"], 2);
    assert_eq!(out.value["digest"], json!(v2.digest));
    assert_eq!(out.value["catalog"], v2.catalog);
    assert_eq!(out.value["unchanged"], false);

    // The agent holds version 1: it is told it is not the newest, and gets version 2.
    let out = call(&client, "get_ui_catalog", json!({"knownDigest": v1.digest})).await;
    assert_eq!(out.value["unchanged"], false);
    assert_eq!(out.value["digest"], json!(v2.digest));
    assert!(out.value.get("catalog").is_some());

    // The agent holds version 2: unchanged, and the catalog is left out.
    let out = call(&client, "get_ui_catalog", json!({"knownDigest": v2.digest})).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(
        out.value,
        json!({
            "catalogId": v2.catalog_id, "version": 2, "digest": v2.digest, "unchanged": true,
        })
    );

    // An older screen joins: recorded, but the thread keeps the newest.
    h.show(thread, &catalog(1, "b")).await;
    let out = call(&client, "get_ui_catalog", json!({})).await;
    assert_eq!(out.value["version"], 2);
}

#[tokio::test]
async fn each_thread_has_its_own_catalog() {
    let h = Harness::start().await;
    let (a, b) = (h.thread("plain").await, h.thread("coder").await);
    h.show(a, &catalog(1, "a")).await;
    h.show(b, &catalog(2, "b")).await;
    let a = call(
        &client_for(&h, a, "plain").await,
        "get_ui_catalog",
        json!({}),
    )
    .await;
    let b = call(
        &client_for(&h, b, "coder").await,
        "get_ui_catalog",
        json!({}),
    )
    .await;
    assert_eq!(a.value["version"], 1);
    assert_eq!(b.value["version"], 2);
}

#[tokio::test]
async fn arguments_that_do_not_parse_are_a_protocol_error() {
    let h = Harness::start().await;
    let thread = h.thread("plain").await;
    let client = client_for(&h, thread, "plain").await;
    for bad in [
        json!({"knownDigest": "sha256:abc"}),
        json!({"knownDigest": 3}),
        json!({"unknown": true}),
    ] {
        let err = try_call(&client, "get_ui_catalog", bad.clone())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("-32602"), "{bad}: {err}");
    }
}

#[tokio::test]
async fn a_name_nobody_owns_is_an_unknown_tool() {
    let h = Harness::start().await;
    let thread = h.thread("plain").await;
    let client = client_for(&h, thread, "plain").await;
    let err = try_call(&client, "start_job", json!({"text": "x"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("-32602"), "{err}");
    assert!(err.to_string().contains("unknown tool"), "{err}");
}

#[tokio::test]
async fn providers_add_tools_after_the_built_in_ones_in_the_order_they_were_added() {
    let (first, second) = (Probe::new("first_probe"), Probe::new("second_probe"));
    let h = Harness::start_with(keys(), {
        let (first, second) = (first.clone(), second.clone());
        |config| config.with_provider(first).with_provider(second)
    })
    .await;
    let thread = h.thread("plain").await;
    let client = client_for(&h, thread, "plain").await;
    assert_eq!(
        tool_names(&client).await,
        [
            "get_ui_catalog",
            "turn_output",
            "first_probe",
            "second_probe"
        ]
    );

    // each call goes to the provider that owns the name, with the call's context
    let out = call(&client, "second_probe", json!({"x": 1})).await;
    assert_eq!(
        out.value,
        json!({"by": "second_probe", "caller": "main", "args": {"x": 1}})
    );
    assert!(first.seen().iter().all(|s| s.what == "list"));
    let calls: Vec<_> = second
        .seen()
        .into_iter()
        .filter(|s| s.what == "call")
        .collect();
    assert_eq!(calls.len(), 1);
    let seen = &calls[0];
    assert_eq!(seen.owner, ALICE);
    assert_eq!(seen.claims.thread, thread);
    assert_eq!(seen.claims.agent.as_str(), "plain");
    assert_eq!(seen.claims.caller, Caller::Main);

    // the built-in tool still answers
    let out = call(&client, "get_ui_catalog", json!({})).await;
    assert!(out.is_error);
    // and an unknown name is still unknown
    let err = try_call(&client, "third_probe", json!({}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("-32602"), "{err}");
}

#[tokio::test]
async fn a_provider_cannot_take_a_name_a_built_in_tool_owns() {
    let thief = Probe::new("get_ui_catalog");
    let h = Harness::start_with(keys(), {
        let thief = thief.clone();
        |config| config.with_provider(thief)
    })
    .await;
    let thread = h.thread("plain").await;
    let client = client_for(&h, thread, "plain").await;
    assert_eq!(tool_names(&client).await, ["get_ui_catalog", "turn_output"]);
    let out = call(&client, "get_ui_catalog", json!({})).await;
    assert!(out.is_error, "the built-in answered, not the provider");
    assert!(thief.seen().iter().all(|s| s.what == "list"));
}

#[tokio::test]
async fn a_providers_tools_are_listed_for_each_request_not_remembered() {
    // The same provider listed twice: the list is built per request, nothing is cached.
    let probe = Probe::new("probe");
    let h = Harness::start_with(keys(), {
        let probe = probe.clone();
        |config| config.with_provider(probe)
    })
    .await;
    let thread = h.thread("plain").await;
    let client = client_for(&h, thread, "plain").await;
    tool_names(&client).await;
    tool_names(&client).await;
    assert_eq!(probe.seen().iter().filter(|s| s.what == "list").count(), 2);
}

/// `get_ui_catalog` still answers the thread's current catalog when more than 32 catalogs of
/// lower versions were recorded after it (the ledger records every new digest; only a version at
/// least the current one becomes current).
#[tokio::test]
async fn the_current_catalog_is_still_given_after_many_older_ones_were_recorded() {
    let h = Harness::start().await;
    let thread = h.thread("plain").await;
    let client = client_for(&h, thread, "plain").await;
    let current = catalog(50, "current");
    h.show(thread, &current).await;
    for n in 0..40 {
        h.show(thread, &catalog(1 + n % 9, &format!("old-{n}")))
            .await;
    }
    let out = call(&client, "get_ui_catalog", json!({})).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.value["version"], 50);
    assert_eq!(out.value["digest"], json!(current.digest));
    assert_eq!(out.value["catalog"], current.catalog);
    let out = call(
        &client,
        "get_ui_catalog",
        json!({"knownDigest": current.digest}),
    )
    .await;
    assert_eq!(out.value["unchanged"], true);
}
