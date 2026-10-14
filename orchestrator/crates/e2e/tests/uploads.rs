//! Large files by a presigned upload (`uploads/v1`, ADR 0058, `docs/api/uploads-v1.md`), end to end:
//! the real dispatcher and A2A adapter mint a grant and offer the extension, the fake agent asks the
//! thread's endpoint for a slot with `request_upload`, sends its bytes to the URL it got (a bucket,
//! the S3 adapter over the in-process S3 stub, whose public endpoint is the stub's own address),
//! and names the upload in its artifact; the dispatcher reads the object back from the bucket,
//! keeps it like any file and the API serves it. Each scenario runs on the in-memory store and on
//! Postgres.
//!
//! What the stub cannot prove, a real S3-compatible server in `compose.yaml` does: that it enforces
//! the signed checksum (`crates/artifacts-s3/tests/s3.rs`, with `ORCH_TEST_S3_URL`). There is no
//! `dev/upload-e2e.sh`: a WireMock agent can neither call the tool nor `PUT`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use base64::Engine as _;
use common::*;
use orch_core::{THREAD_TOOLS_EXTENSION, UPLOADS_EXTENSION};
use orch_testsupport::fake::{UPLOAD_FILENAME, UPLOAD_MEDIA_TYPE, upload_content};
use orch_testsupport::{Chat, FakeAgentOptions, TestInstance};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

const NOT_KEPT: &str = "the file could not be kept";
const TTL: Duration = Duration::from_secs(300);
const GIB: u64 = 1 << 30;

/// A world whose `plain` agent lists `thread-tools/v1` and, when `lists`, `uploads/v1`, and whose
/// deployment offers uploads when `offers`.
async fn world(backend: Backend, lists: bool, offers: bool) -> World {
    let mut extensions = vec![THREAD_TOOLS_EXTENSION.to_owned()];
    if lists {
        extensions.push(UPLOADS_EXTENSION.to_owned());
    }
    World::with(
        backend,
        Setup {
            thread_tools: true,
            uploads: offers.then_some((GIB, TTL)),
            plain: FakeAgentOptions {
                extensions,
                ..FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `GET path` as `user`: `(status, body)`.
async fn fetch(instance: &TestInstance, path: &str, user: &str) -> (u16, Vec<u8>) {
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .get(format!("{}{path}", instance.base_url))
        .header("X-Auth-Request-Email", user)
        .send()
        .await
        .unwrap();
    (
        response.status().as_u16(),
        response.bytes().await.unwrap().to_vec(),
    )
}

/// The agent's own text artifact of its `jobs`-th job, in order: what it says it did.
fn reports(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["kind"] == "artifact" && e["data"]["file"].is_null())
        .filter_map(|e| e["data"]["text"].as_str().map(str::to_owned))
        .collect()
}

/// The artifacts that are kept files.
fn files(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .filter(|e| e["kind"] == "artifact" && !e["data"]["file"].is_null())
        .map(|e| &e["data"])
        .collect()
}

fn errors(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["kind"] == "error")
        .map(|e| e["data"]["message"].as_str().unwrap().to_owned())
        .collect()
}

/// Waits until the thread is `done` in its `jobs`-th job (one `done` state per job).
async fn wait_for_jobs(chat: &Chat, thread: &str, jobs: usize) -> Vec<Value> {
    orch_testsupport::eventually("the job to finish", || async {
        let events = chat.events(thread).await;
        let done = events
            .iter()
            .filter(|e| e["kind"] == "thread_state" && e["data"]["state"] == "done")
            .count();
        (done == jobs).then_some(events)
    })
    .await
}

async fn a_file_the_agent_puts_in_the_bucket_is_kept_served_and_never_in_the_log(backend: Backend) {
    let world = world(backend, true, true).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let alice = world.chat(&orch);
    let content = upload_content();
    let sha = hex(&Sha256::digest(&content));

    let thread = alice.seed_thread("plain", "upload", None).await;
    let events = wait_for_jobs(&alice, &thread, 1).await;

    // the agent was offered uploads and activated them, and was told the limits of its slot
    let calls = world.plain.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].activates(UPLOADS_EXTENSION));
    assert_eq!(
        calls[0].uploads.as_ref().unwrap(),
        &serde_json::json!({"maxBytes": GIB, "ttlSecs": 300})
    );

    // it got a slot and its PUT was taken; the file is a reference in the log
    assert_eq!(
        reports(&events),
        [format!(
            "upload: slot for {UPLOAD_FILENAME} ({} bytes); put 200",
            content.len()
        )]
    );
    assert_eq!(errors(&events), Vec::<String>::new());
    let [file] = files(&events)[..] else {
        panic!("one file: {events:#?}")
    };
    assert_eq!(file["mimeType"], UPLOAD_MEDIA_TYPE);
    assert_eq!(file["file"]["sha256"], sha);
    assert_eq!(file["file"]["size"], content.len());
    assert_eq!(file["file"]["filename"], UPLOAD_FILENAME);

    // the object is in the bucket once, under the thread, and its bytes are the agent's
    let keys = world.s3().keys();
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert!(
        keys[0].contains(&thread) && keys[0].contains(&sha),
        "{keys:?}"
    );
    assert_eq!(world.s3().object(&keys[0]).unwrap().body, content);

    // the PUT carried the signed checksum the tool gave, and the log holds no id, URL or signature
    let put = world
        .s3()
        .seen()
        .into_iter()
        .find(|s| s.method == axum::http::Method::PUT)
        .expect("the agent's PUT");
    assert_eq!(
        put.headers["x-amz-checksum-sha256"].to_str().unwrap(),
        base64::engine::general_purpose::STANDARD.encode(Sha256::digest(&content))
    );
    let log = serde_json::to_string(&events).unwrap();
    for secret in ["uploadId", "X-Amz", "127.0.0.1"] {
        assert!(!log.contains(secret), "{secret} is in the log: {log}");
    }

    // the API serves the bytes to the thread's owner, and to nobody else
    let path = format!("/api/threads/{thread}/artifacts/{sha}");
    assert_eq!(fetch(&orch, &path, ALICE).await, (200, content.clone()));
    assert_eq!(fetch(&orch, &path, BOB).await.0, 404);
    // another process serves it as well: the bucket is the shared state
    let other = world.instance_with("orch-2", false).await;
    assert_eq!(fetch(&other, &path, ALICE).await, (200, content));

    // the same content again, in another job: a second reference and still one object
    alice.seed_message(&thread, "upload").await;
    let events = wait_for_jobs(&alice, &thread, 2).await;
    assert_eq!(files(&events).len(), 2);
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(world.s3().keys().len(), 1);
}

async fn bytes_the_bucket_refuses_and_an_upload_nobody_made_are_not_kept(backend: Backend) {
    let world = world(backend, true, true).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let alice = world.chat(&orch);

    // other bytes than the ones the slot was asked for: the bucket checks the signed checksum
    let thread = alice.seed_thread("plain", "upload other", None).await;
    let events = wait_for_jobs(&alice, &thread, 1).await;
    let [report] = &reports(&events)[..] else {
        panic!("{events:#?}")
    };
    assert!(report.ends_with("put 400"), "{report}");
    assert!(files(&events).is_empty());
    assert_eq!(errors(&events), [NOT_KEPT]);
    assert!(world.s3().keys().is_empty(), "{:?}", world.s3().keys());

    // a file that was never put: the agent names an upload that is not there
    alice.seed_message(&thread, "upload nothing").await;
    let events = wait_for_jobs(&alice, &thread, 2).await;
    assert!(reports(&events)[1].ends_with("nothing put"));
    assert!(files(&events).is_empty());
    assert_eq!(errors(&events), [NOT_KEPT, NOT_KEPT]);
    assert!(world.s3().keys().is_empty());
}

async fn a_file_over_the_most_one_may_be_is_refused_by_the_tool_and_the_agent_hears_why(
    backend: Backend,
) {
    let world = world(backend, true, true).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let alice = world.chat(&orch);

    let thread = alice.seed_thread("plain", "upload big", None).await;
    let events = wait_for_jobs(&alice, &thread, 1).await;
    let [report] = &reports(&events)[..] else {
        panic!("{events:#?}")
    };
    assert!(
        report.starts_with("upload: refused: too_large: "),
        "{report}"
    );
    // no slot, so nothing is named, kept or signed
    assert!(files(&events).is_empty());
    assert_eq!(errors(&events), Vec::<String>::new());
    assert!(world.s3().keys().is_empty());
    assert!(
        world
            .s3()
            .seen()
            .iter()
            .all(|s| s.method != axum::http::Method::PUT)
    );
}

async fn an_agent_whose_card_does_not_list_uploads_is_told_of_none_and_offered_no_tool(
    backend: Backend,
) {
    let world = world(backend, false, true).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let alice = world.chat(&orch);

    let thread = alice.seed_thread("plain", "upload", None).await;
    let events = wait_for_jobs(&alice, &thread, 1).await;
    let calls = world.plain.calls();
    assert!(!calls[0].activates(UPLOADS_EXTENSION));
    assert!(calls[0].uploads.is_none());
    let [report] = &reports(&events)[..] else {
        panic!("{events:#?}")
    };
    assert!(
        report.starts_with("upload: request_upload is not offered; offered="),
        "{report}"
    );
    assert!(files(&events).is_empty());
}

async fn a_deployment_that_offers_no_uploads_offers_nothing_even_to_an_agent_that_lists_them(
    backend: Backend,
) {
    let world = world(backend, true, false).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let alice = world.chat(&orch);

    let thread = alice.seed_thread("plain", "upload", None).await;
    let events = wait_for_jobs(&alice, &thread, 1).await;
    let calls = world.plain.calls();
    assert!(!calls[0].activates(UPLOADS_EXTENSION));
    assert!(calls[0].uploads.is_none());
    let [report] = &reports(&events)[..] else {
        panic!("{events:#?}")
    };
    assert!(
        report.starts_with("upload: request_upload is not offered; offered="),
        "{report}"
    );
    assert!(files(&events).is_empty());
}

backends!(
    a_file_the_agent_puts_in_the_bucket_is_kept_served_and_never_in_the_log,
    bytes_the_bucket_refuses_and_an_upload_nobody_made_are_not_kept,
    a_file_over_the_most_one_may_be_is_refused_by_the_tool_and_the_agent_hears_why,
    an_agent_whose_card_does_not_list_uploads_is_told_of_none_and_offered_no_tool,
    a_deployment_that_offers_no_uploads_offers_nothing_even_to_an_agent_that_lists_them,
);
