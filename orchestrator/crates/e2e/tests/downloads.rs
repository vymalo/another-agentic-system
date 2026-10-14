//! Downloads by a redirect to a presigned `GET` (ADR 0058 A1, step 6b), end to end: a fake A2A agent
//! hands over a file, the dispatcher keeps it in the bucket, and the API, with `artifacts.s3.downloads:
//! redirect`, answers the owner with a `302` to the bucket instead of sending the bytes; the test
//! then plays the browser, which follows the `Location` with nothing but the URL.
//!
//! The bucket is the S3 stub in process (a real HTTP server that answers a presigned `GET` with the
//! signed `response-*` members as its headers), or, when `ORCH_TEST_S3_URL` names one, a real
//! S3-compatible server (RustFS in `compose.yaml`), which also checks the signature. Each scenario
//! runs on the in-memory store and on Postgres.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_testsupport::fake::{PNG, SVG_WITH_SCRIPT};
use orch_testsupport::{Chat, TestInstance};
use serde_json::Value;

const TTL: Duration = Duration::from_secs(300);

async fn world(backend: Backend) -> World {
    World::with(
        backend,
        Setup {
            downloads: Some(TTL),
            ..Setup::default()
        },
    )
    .await
}

/// What the owner's request got, without following a redirect: `(status, headers, body)`.
async fn fetch(
    instance: &TestInstance,
    path: &str,
    user: &str,
) -> (u16, reqwest::header::HeaderMap, Vec<u8>) {
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!("{}{path}", instance.base_url))
        .header("X-Auth-Request-Email", user)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    (status, headers, response.bytes().await.unwrap().to_vec())
}

/// The browser's `GET` of a `Location`: the URL alone, no credential of ours.
async fn follow(
    location: &reqwest::header::HeaderValue,
) -> (u16, reqwest::header::HeaderMap, Vec<u8>) {
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(location.to_str().unwrap())
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    (status, headers, response.bytes().await.unwrap().to_vec())
}

/// Runs one turn and returns the thread and the hash of the file the agent handed over.
async fn handed_over(chat: &Chat, text: &str) -> (String, String) {
    let id = chat.seed_thread("plain", text, None).await;
    chat.wait_state(&id, "done").await;
    let events = chat.events(&id).await;
    let files: Vec<&Value> = events
        .iter()
        .filter(|e| e["kind"] == "artifact" && !e["data"]["file"].is_null())
        .map(|e| &e["data"])
        .collect();
    let [file] = files[..] else {
        panic!("one file: {events:#?}")
    };
    (id, file["file"]["sha256"].as_str().unwrap().to_owned())
}

async fn the_owner_is_sent_to_the_bucket_and_the_bucket_serves_what_the_route_signed(
    backend: Backend,
) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);
    let (id, sha) = handed_over(&alice, "file make a chart").await;
    let path = format!("/api/threads/{id}/artifacts/{sha}");

    // the route answers with a redirect and no bytes, and did not read the file to do it
    let reads_before = world.s3_stub().map(|s| gets_of(s, &sha));
    let (status, headers, body) = fetch(&orch, &path, ALICE).await;
    assert_eq!(status, 302);
    assert!(body.is_empty());
    assert_eq!(headers["cache-control"], "private, no-store");
    let location = &headers["location"];
    let url = reqwest::Url::parse(location.to_str().unwrap()).unwrap();
    assert!(
        url.path().contains(&format!("threads/{id}/{sha}")),
        "the thread's own folder: {url}"
    );
    assert_eq!(
        url.query_pairs()
            .find(|(k, _)| k == "X-Amz-Expires")
            .map(|(_, v)| v.into_owned()),
        Some("300".to_owned())
    );
    if let (Some(stub), Some(before)) = (world.s3_stub(), reads_before) {
        assert_eq!(
            gets_of(stub, &sha),
            before,
            "the orchestrator did not read the object"
        );
        assert_eq!(url.host_str(), Some("127.0.0.1"));
    }

    // the browser follows it with nothing else and gets the bytes under the signed headers
    let (status, headers, body) = follow(location).await;
    assert_eq!(status, 200);
    assert_eq!(body, PNG);
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(
        headers["content-disposition"],
        "inline; filename=\"chart.png\""
    );
    assert_eq!(headers["cache-control"], "private, max-age=300");

    // a download of it is an attachment
    let (status, headers, _) = fetch(&orch, &format!("{path}?download=1"), ALICE).await;
    assert_eq!(status, 302);
    let (status, headers, body) = follow(&headers["location"]).await;
    assert_eq!((status, body.as_slice()), (200, &PNG[..]));
    assert_eq!(
        headers["content-disposition"],
        "attachment; filename=\"chart.png\""
    );
    assert_eq!(headers["content-type"], "image/png");

    // nobody else is sent anywhere, and another process (the same bucket) sends the owner too
    let (status, headers, _) = fetch(&orch, &path, BOB).await;
    assert_eq!(status, 404);
    assert!(!headers.contains_key("location"));
    let other = world.instance_with("orch-2", false).await;
    let (status, headers, _) = fetch(&other, &path, ALICE).await;
    assert_eq!(status, 302);
    assert_eq!(follow(&headers["location"]).await.2, PNG);
}

async fn a_type_that_lies_and_an_svg_are_what_the_route_always_said(backend: Backend) {
    let world = world(backend).await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);

    // a type that is not the file's: octet-stream and an attachment, from the bucket
    let (id, sha) = handed_over(&alice, "file-lie a photo").await;
    let (status, headers, _) =
        fetch(&orch, &format!("/api/threads/{id}/artifacts/{sha}"), ALICE).await;
    assert_eq!(status, 302);
    let (status, headers, _) = follow(&headers["location"]).await;
    assert_eq!(status, 200);
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );

    // an SVG shown inline is sanitized by the orchestrator and never leaves it; its download is
    // the original, from the bucket
    let (id, sha) = handed_over(&alice, "file-svg a mark").await;
    let path = format!("/api/threads/{id}/artifacts/{sha}");
    let (status, headers, body) = fetch(&orch, &path, ALICE).await;
    assert_eq!(status, 200);
    assert!(!headers.contains_key("location"));
    let shown = String::from_utf8(body).unwrap();
    assert!(
        shown.contains("<circle") && !shown.contains("script") && !shown.contains("onload"),
        "{shown}"
    );
    let (status, headers, _) = fetch(&orch, &format!("{path}?download=1"), ALICE).await;
    assert_eq!(status, 302);
    let (status, headers, body) = follow(&headers["location"]).await;
    assert_eq!(status, 200);
    assert_eq!(body, SVG_WITH_SCRIPT.as_bytes());
    assert_eq!(headers["content-type"], "image/svg+xml");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
}

/// How many times the stub was asked to `GET` the object of `sha` (any thread).
fn gets_of(stub: &common::s3stub::Stub, sha: &str) -> usize {
    stub.seen()
        .iter()
        .filter(|s| s.method == axum::http::Method::GET && s.path.ends_with(sha))
        .count()
}

backends!(
    the_owner_is_sent_to_the_bucket_and_the_bucket_serves_what_the_route_signed,
    a_type_that_lies_and_an_svg_are_what_the_route_always_said,
);
