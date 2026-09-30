//! What bounds `wait_for_job`: the per-process and per-user caps, a wait without a progress token,
//! a client that hangs up, and a replica that is shutting down. A wait is a request that stays
//! open on a route with no request timeout, so each of these has to end it or refuse it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_testsupport::eventually;
use serde_json::{Value, json};
use support::*;
use tokio::task::JoinHandle;

fn wait_args(job: &str, after_seq: i64, secs: u64) -> Value {
    json!({"job_id": job, "after_seq": after_seq, "timeout_secs": secs})
}

/// A wait through an MCP client of its own (so it has a progress token and a connection of its
/// own), kept open in the background.
fn hold(h: &Harness, token: &'static str, job: &str) -> JoinHandle<Outcome> {
    let (url, job) = (h.mcp_url.clone(), job.to_owned());
    tokio::spawn(async move {
        let client = connect(&url, token).await;
        call(&client, "wait_for_job", wait_args(&job, 0, 3600)).await
    })
}

/// Retries a zero-second wait until it is accepted: the slot it needed has been given back.
async fn until_a_slot_is_free(client: &Client, job: &str) {
    eventually("a wait slot is free again", || async {
        let out = call(client, "wait_for_job", wait_args(job, 0, 0)).await;
        (!out.is_error).then_some(())
    })
    .await;
}

async fn is_refused_as_busy(client: &Client, job: &str) -> Option<Outcome> {
    let out = call(client, "wait_for_job", wait_args(job, 0, 0)).await;
    (out.is_error && out.text.starts_with("too many waits")).then_some(out)
}

#[tokio::test]
async fn one_user_cannot_hold_more_waits_than_the_per_user_cap() {
    let h = Harness::start_with(Options {
        wait_limits: Some((10, 2)),
        ..Options::default()
    })
    .await;
    let alice = h.client(ALICE_TOKEN).await;
    let bob = h.client(BOB_TOKEN).await;
    let job = start(&alice, "gate caps").await;
    wait_state(&alice, &job, "working").await;
    let bobs = start(&bob, "gate bobs").await;
    wait_state(&bob, &bobs, "working").await;

    let held = [hold(&h, ALICE_TOKEN, &job), hold(&h, ALICE_TOKEN, &job)];
    let refused = eventually("the third wait is turned away", || async {
        is_refused_as_busy(&alice, &job).await
    })
    .await;
    assert!(
        refused.text.contains("you already have 2"),
        "{}",
        refused.text
    );
    assert!(refused.value.is_null());

    // Another user's share is their own.
    let theirs = call(&bob, "wait_for_job", wait_args(&bobs, 0, 0)).await;
    assert!(!theirs.is_error, "{theirs:?}");
    // The other tools are not waits.
    let get = call(&alice, "get_job", json!({"job_id": job})).await;
    assert!(!get.is_error, "{get:?}");

    // The slots come back when the waits end.
    h.agent.release_gate();
    for wait in held {
        assert_eq!(wait.await.unwrap().value["outcome"], "finished");
    }
    let again = call(&alice, "wait_for_job", wait_args(&job, 0, 0)).await;
    assert!(!again.is_error, "{again:?}");
}

#[tokio::test]
async fn the_process_cap_holds_whoever_asks() {
    let h = Harness::start_with(Options {
        wait_limits: Some((2, 2)),
        ..Options::default()
    })
    .await;
    let alice = h.client(ALICE_TOKEN).await;
    let bob = h.client(BOB_TOKEN).await;
    let ajob = start(&alice, "gate a").await;
    let bjob = start(&bob, "gate b").await;
    wait_state(&alice, &ajob, "working").await;
    wait_state(&bob, &bjob, "working").await;

    let held = [hold(&h, ALICE_TOKEN, &ajob), hold(&h, BOB_TOKEN, &bjob)];
    // Each of them holds one wait and is within their own cap, but the process is full.
    let refused = eventually("the process turns the next wait away", || async {
        is_refused_as_busy(&alice, &ajob).await
    })
    .await;
    assert!(
        refused.text.contains("this server already holds 2"),
        "{}",
        refused.text
    );
    assert!(is_refused_as_busy(&bob, &bjob).await.is_some());

    // One permit per held job.
    h.agent.release_gate();
    h.agent.release_gate();
    for wait in held {
        assert_eq!(wait.await.unwrap().value["outcome"], "finished");
    }
    until_a_slot_is_free(&alice, &ajob).await;
}

/// A wait over bare HTTP that is left open in the background.
fn raw_hold(h: &Harness, job: &str, progress_token: Option<&'static str>) -> JoinHandle<()> {
    let (http, url) = (h.http.clone(), h.mcp_url.clone());
    let body = tool_call("wait_for_job", wait_args(job, 0, 3600), progress_token);
    tokio::spawn(async move {
        let (name, value) = bearer(ALICE_TOKEN);
        let resp = http
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header(name, value)
            .json(&body)
            .send()
            .await
            .unwrap();
        // Read until the connection ends, which it does not before the test drops it.
        let _ = resp.bytes().await;
    })
}

async fn a_hung_up_wait_gives_its_slot_back(progress_token: Option<&'static str>) {
    let h = Harness::start_with(Options {
        wait_limits: Some((10, 1)),
        ..Options::default()
    })
    .await;
    let alice = h.client(ALICE_TOKEN).await;
    let job = start(&alice, "gate hangup").await;
    wait_state(&alice, &job, "working").await;

    let open = raw_hold(&h, &job, progress_token);
    eventually("the wait holds the user's only slot", || async {
        is_refused_as_busy(&alice, &job).await
    })
    .await;

    // The client goes away: dropping the request closes its connection.
    open.abort();
    let _ = open.await;
    tokio::time::timeout(T, until_a_slot_is_free(&alice, &job))
        .await
        .expect("the server noticed the client was gone and ended the wait");
    h.agent.release_gate();
}

#[tokio::test]
async fn a_client_that_hangs_up_during_a_wait_with_a_progress_token_frees_its_slot() {
    a_hung_up_wait_gives_its_slot_back(Some("p-1")).await;
}

#[tokio::test]
async fn a_client_that_hangs_up_during_a_wait_without_a_progress_token_frees_its_slot() {
    a_hung_up_wait_gives_its_slot_back(None).await;
}

#[tokio::test]
async fn a_wait_without_a_progress_token_returns_within_a_heartbeat_with_where_to_resume() {
    let h = Harness::start_with(Options {
        heartbeat: Some(Duration::from_millis(150)),
        ..Options::default()
    })
    .await;
    let alice = h.client(ALICE_TOKEN).await;
    let job = start(&alice, "gate silent").await;
    wait_state(&alice, &job, "working").await;
    let last = wait_state(&alice, &job, "working").await["last_seq"]
        .as_i64()
        .unwrap();

    // An hour asked, nobody listening for progress: the server gives up after one heartbeat
    // interval, with the cursor, instead of holding a connection that would look dead.
    let started = std::time::Instant::now();
    let resp = h
        .post(
            &[(bearer(ALICE_TOKEN).0, &bearer(ALICE_TOKEN).1)],
            &tool_call("wait_for_job", wait_args(&job, last, 3600), None),
        )
        .await;
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let messages = messages(resp).await;
    assert!(started.elapsed() < T, "{:?}", started.elapsed());
    assert!(
        messages
            .iter()
            .all(|m| m["method"] != "notifications/progress"),
        "nothing to notify: {messages:?}"
    );
    let result = messages
        .iter()
        .find_map(|m| m.get("result"))
        .unwrap_or_else(|| panic!("a result: {messages:?}"));
    let out = &result["structuredContent"];
    assert_eq!(out["outcome"], "timed_out", "{result}");
    assert_eq!(out["resume_after_seq"], last, "{result}");
    assert_eq!(out["state"], "working");

    // With a progress token the same call stays open past that interval.
    let (client, progress) = connect_recording(&h.mcp_url, ALICE_TOKEN).await;
    let waiting = {
        let peer = client.peer().clone();
        let job = job.clone();
        tokio::spawn(
            async move { call_peer(&peer, "wait_for_job", wait_args(&job, last, 3600)).await },
        )
    };
    eventually("several heartbeats arrived on one call", || async {
        (progress.heartbeats() >= 3).then_some(())
    })
    .await;
    assert!(!waiting.is_finished());
    h.agent.release_gate();
    assert_eq!(waiting.await.unwrap().value["outcome"], "finished");
}

#[tokio::test]
async fn a_replica_that_is_shutting_down_says_interrupted_and_when_to_come_back() {
    let h = Harness::start().await;
    let alice = h.client(ALICE_TOKEN).await;
    let job = start(&alice, "gate going away").await;
    wait_state(&alice, &job, "working").await;

    // The other outcomes carry no such hint.
    let timed_out = call(&alice, "wait_for_job", wait_args(&job, 0, 0)).await;
    assert_eq!(timed_out.value["outcome"], "timed_out");
    assert!(timed_out.value.get("retry_after_secs").is_none());

    let waiting = hold(&h, ALICE_TOKEN, &job);
    eventually("the wait is open", || async {
        (!waiting.is_finished()).then_some(())
    })
    .await;
    h.app.set_shutting_down();
    let out = tokio::time::timeout(T, waiting).await.unwrap().unwrap();
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.value["outcome"], "interrupted");
    assert!(
        out.value["retry_after_secs"].as_u64().unwrap() >= 1,
        "{out:?}"
    );
    assert!(out.value["resume_after_seq"].as_i64().is_some());
}
