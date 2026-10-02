//! Files an agent hands over (ADR 0032), end to end: a fake A2A agent sends an artifact whose part
//! is a file (or a link), the A2A adapter and the dispatcher keep it in the artifact store (a
//! directory store in a temporary directory), the log holds a reference, the API serves it and
//! AG-UI projects it. Each scenario runs on the in-memory store and on Postgres.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use common::*;
use orch_app::FileLimits;
use orch_testsupport::fake::{PNG, SVG_WITH_SCRIPT};
use orch_testsupport::{Chat, TestInstance};
use serde_json::Value;

/// `GET path` as `user`, whole: `(status, headers, body)`.
async fn fetch(
    instance: &TestInstance,
    path: &str,
    user: &str,
) -> (u16, reqwest::header::HeaderMap, Vec<u8>) {
    let response = reqwest::Client::builder()
        .no_proxy()
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

fn artifacts(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .filter(|e| e["kind"] == "artifact")
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

/// The number of files the world's store holds for `thread`.
fn stored(world: &World, thread: &str) -> usize {
    let dir = world.artifacts_root().join("threads").join(thread);
    std::fs::read_dir(dir).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|e| !e.file_name().to_string_lossy().contains('.'))
            .count()
    })
}

async fn run(chat: &Chat, text: &str) -> (String, Vec<Value>) {
    let id = chat.seed_thread("plain", text, None).await;
    chat.wait_state(&id, "done").await;
    let events = chat.events(&id).await;
    (id, events)
}

async fn a_file_is_kept_served_and_projected(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);
    let (id, events) = run(&alice, "file make a chart").await;
    assert_eq!(shape(&events), FIVE);
    let [artifact] = artifacts(&events)[..] else {
        panic!("one artifact")
    };
    let file = &artifact["file"];
    assert_eq!(artifact["name"], "chart");
    assert_eq!(artifact["mimeType"], "image/png");
    assert_eq!(file["size"], PNG.len());
    assert_eq!(file["filename"], "chart.png");
    let sha = file["sha256"].as_str().unwrap().to_owned();
    assert_eq!(sha.len(), 64);
    // the log holds the reference and none of the bytes
    let log = serde_json::to_string(&events).unwrap();
    assert!(!log.contains("IHDR") && !log.contains("\"bytes\""), "{log}");
    // the bytes are in the store, once
    assert_eq!(stored(&world, &id), 1);

    // the API serves them to the thread's owner
    let (status, headers, body) =
        fetch(&orch, &format!("/api/threads/{id}/artifacts/{sha}"), ALICE).await;
    assert_eq!(status, 200);
    assert_eq!(body, PNG);
    assert_eq!(headers["content-type"], "image/png");
    assert_eq!(
        headers["content-disposition"],
        "inline; filename=\"chart.png\""
    );
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(
        headers["cache-control"],
        "private, max-age=31536000, immutable"
    );
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .ends_with("; sandbox")
    );
    // and to nobody else
    let (status, _, _) = fetch(&orch, &format!("/api/threads/{id}/artifacts/{sha}"), BOB).await;
    assert_eq!(status, 404);
    // another instance (another process, the same store and database) serves it as well
    let other = world.instance_with("orch-2", false).await;
    let (status, _, body) = fetch(
        &other,
        &format!("/api/threads/{id}/artifacts/{sha}?download=1"),
        ALICE,
    )
    .await;
    assert_eq!((status, body), (200, PNG.to_vec()));

    // AG-UI projects it: where to fetch it, how big, and that it is an image
    let frames = alice
        .agui_connect(&id, None, true)
        .await
        .collect_frames(std::time::Duration::from_secs(20))
        .await;
    let activity = frames
        .iter()
        .find(|f| f.event["activityType"] == "vymalo.artifact")
        .expect("the artifact activity");
    let content = &activity.event["content"];
    assert_eq!(content["kind"], "file");
    assert_eq!(
        content["href"],
        format!("/api/threads/{id}/artifacts/{sha}")
    );
    assert_eq!(content["size"], PNG.len());
    assert_eq!(content["preview"], "image");
    assert_eq!(content["filename"], "chart.png");
    let wire = serde_json::to_string(&frames.iter().map(|f| &f.event).collect::<Vec<_>>()).unwrap();
    assert!(!wire.contains("IHDR"), "no byte of the file is in a frame");
}

async fn an_svg_is_sanitized_inline_and_whole_as_a_download(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);
    let (id, events) = run(&alice, "file-svg draw").await;
    let [artifact] = artifacts(&events)[..] else {
        panic!()
    };
    assert_eq!(artifact["mimeType"], "image/svg+xml");
    let sha = artifact["file"]["sha256"].as_str().unwrap().to_owned();
    let path = format!("/api/threads/{id}/artifacts/{sha}");
    let (status, headers, body) = fetch(&orch, &path, ALICE).await;
    assert_eq!(status, 200);
    assert_eq!(headers["content-type"], "image/svg+xml");
    let shown = String::from_utf8(body).unwrap();
    assert!(
        shown.contains("<circle") && !shown.contains("script") && !shown.contains("onload"),
        "{shown}"
    );
    let (_, headers, body) = fetch(&orch, &format!("{path}?download=1"), ALICE).await;
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
    assert_eq!(
        body,
        SVG_WITH_SCRIPT.as_bytes(),
        "the download is the original"
    );
    assert_eq!(
        alice
            .agui_connect(&id, None, true)
            .await
            .collect_frames(std::time::Duration::from_secs(20))
            .await
            .iter()
            .find(|f| f.event["activityType"] == "vymalo.artifact")
            .unwrap()
            .event["content"]["preview"],
        "image"
    );
}

async fn a_type_that_lies_is_octet_stream_and_never_inline(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);
    let (id, events) = run(&alice, "file-lie a photo").await;
    let [artifact] = artifacts(&events)[..] else {
        panic!()
    };
    assert_eq!(artifact["mimeType"], "application/octet-stream");
    let sha = artifact["file"]["sha256"].as_str().unwrap();
    let (_, headers, _) = fetch(&orch, &format!("/api/threads/{id}/artifacts/{sha}"), ALICE).await;
    assert_eq!(headers["content-type"], "application/octet-stream");
    assert!(
        headers["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );
}

async fn text_is_inline_and_the_same_file_twice_is_one_object(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);
    let (id, events) = run(&alice, "file-text notes").await;
    let [artifact] = artifacts(&events)[..] else {
        panic!()
    };
    let sha = artifact["file"]["sha256"].as_str().unwrap();
    let (_, headers, body) =
        fetch(&orch, &format!("/api/threads/{id}/artifacts/{sha}"), ALICE).await;
    assert_eq!(headers["content-type"], "text/plain; charset=utf-8");
    assert_eq!(body, b"hello from a file");

    let (id, events) = run(&alice, "file-twice again").await;
    let found = artifacts(&events);
    assert_eq!(found.len(), 2, "the agent sent two artifacts");
    assert_eq!(found[0]["file"]["sha256"], found[1]["file"]["sha256"]);
    assert_eq!(stored(&world, &id), 1, "one object for one content");
    assert!(errors(&events).is_empty());
}

async fn at_most_fifty_files_a_job_and_the_rest_are_refused_with_an_error(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);
    let (id, events) = run(&alice, "file-many all of them").await;
    let found = artifacts(&events);
    assert_eq!(found.len(), 52);
    assert_eq!(found.iter().filter(|a| !a["file"].is_null()).count(), 50);
    assert!(found[50]["file"].is_null() && found[51]["file"].is_null());
    assert_eq!(
        errors(&events),
        vec!["this job has reached its limit of files, so the file is not kept"; 2]
    );
    assert_eq!(stored(&world, &id), 50);
    // the turn went on to its end
    assert_eq!(events.last().unwrap()["data"]["state"], "done");
}

async fn a_file_over_the_cap_is_refused_and_the_turn_goes_on(backend: Backend) {
    let world = World::with(
        backend,
        Setup {
            files: FileLimits {
                max_file_bytes: 1024,
                ..FileLimits::default()
            },
            ..Setup::default()
        },
    )
    .await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);
    let (id, events) = run(&alice, "file-big a dump").await;
    let [artifact] = artifacts(&events)[..] else {
        panic!()
    };
    assert!(artifact["file"].is_null());
    assert_eq!(artifact["name"], "chart");
    assert_eq!(errors(&events), ["the file is too large to keep"]);
    assert_eq!(stored(&world, &id), 0);
    // the projection says it is a file without an address
    let frames = alice
        .agui_connect(&id, None, true)
        .await
        .collect_frames(std::time::Duration::from_secs(20))
        .await;
    let content = &frames
        .iter()
        .find(|f| f.event["activityType"] == "vymalo.artifact")
        .unwrap()
        .event["content"];
    assert!(content.get("href").is_none(), "{content}");
}

async fn a_url_is_a_link_unless_its_host_is_listed_and_then_it_is_fetched(backend: Backend) {
    // not listed: a link, as it always was
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);
    let (id, events) = run(&alice, "file-url a link").await;
    let [artifact] = artifacts(&events)[..] else {
        panic!()
    };
    assert!(artifact["file"].is_null());
    assert_eq!(
        artifact["uri"],
        format!("{}/files/chart.png", world.plain.base_url())
    );
    assert_eq!(stored(&world, &id), 0);

    // listed: fetched, kept and served like a raw part
    let world = World::with(
        backend,
        Setup {
            fetch_plain: true,
            ..Setup::default()
        },
    )
    .await;
    let orch = world.instance("orch-1").await;
    let alice = world.chat(&orch);
    let (id, events) = run(&alice, "file-url fetch it").await;
    let [artifact] = artifacts(&events)[..] else {
        panic!()
    };
    assert_eq!(artifact["mimeType"], "image/png");
    assert_eq!(artifact["file"]["filename"], "chart.png");
    assert!(artifact["uri"].is_null(), "no link beside the file");
    let sha = artifact["file"]["sha256"].as_str().unwrap();
    let (status, _, body) =
        fetch(&orch, &format!("/api/threads/{id}/artifacts/{sha}"), ALICE).await;
    assert_eq!((status, body), (200, PNG.to_vec()));
    // a host that is not on the list stays a link even then; a redirect is not followed
    let (_, events) = run(&alice, "file-url-other nowhere").await;
    assert_eq!(
        artifacts(&events)[0]["uri"],
        "https://other.example.com/chart.png"
    );
    let (_, events) = run(&alice, "file-url-redirect away").await;
    assert!(artifacts(&events)[0]["file"].is_null());
    assert_eq!(errors(&events), ["the file could not be kept"]);
}

backends!(
    a_file_is_kept_served_and_projected,
    an_svg_is_sanitized_inline_and_whole_as_a_download,
    a_type_that_lies_is_octet_stream_and_never_inline,
    text_is_inline_and_the_same_file_twice_is_one_object,
    at_most_fifty_files_a_job_and_the_rest_are_refused_with_an_error,
    a_file_over_the_cap_is_refused_and_the_turn_goes_on,
    a_url_is_a_link_unless_its_host_is_listed_and_then_it_is_fetched,
);
