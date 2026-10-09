//! `GET /agui/threads/{threadId}/history` and its shared variants (ADR 0059): the pages tile the
//! connect stream, `end` is a cursor a connect resumes from, the chain still open is not in a page,
//! and the authorisation, the limits and the permit of a public read are those of the connect
//! routes.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_api::PublicLimits;
use orch_app::SharingMode;
use orch_core::ShareLevel;
use serde_json::{Value, json};
use support::*;

/// A thread that has run `turns` turns of `echo`: one job each, one run each.
async fn turns(h: &Harness, turns: usize) -> String {
    let thread = new_thread_id();
    let mut said: Vec<(String, String)> = Vec::new();
    for n in 1..=turns {
        said.push((format!("m{n}"), format!("echo turn {n}")));
        let messages: Vec<(&str, &str)> = said
            .iter()
            .map(|(id, text)| (id.as_str(), text.as_str()))
            .collect();
        h.run(
            "plain",
            ALICE,
            &input(&thread, &format!("run-{n}"), &messages),
        )
        .await
        .all()
        .await;
        h.wait_state(ALICE, &thread, "done").await;
    }
    thread
}

/// `(id, event)` of the frames of a page, as the connect stream writes them.
fn frames_of(page: &Value) -> Vec<(Option<i64>, Value)> {
    page["frames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["id"].as_i64(), f["event"].clone()))
        .collect()
}

fn replay_of(frames: &[Frame]) -> Vec<(Option<i64>, Value)> {
    frames.iter().map(|f| (f.id, f.event.clone())).collect()
}

async fn page(h: &Harness, thread: &str, query: &str) -> Value {
    let r = h.history(thread, Some(ALICE), query, None).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    r.json()
}

#[tokio::test]
async fn the_pages_of_a_thread_tile_its_connect_stream() {
    let h = Harness::start().await;
    let thread = turns(&h, 5).await;
    let whole = replay_of(&h.connect_run(&thread, ALICE, None).await.all().await);
    assert!(!whole.is_empty());

    for limit in [1, 2, 3, 100] {
        let mut pages: Vec<Value> = Vec::new();
        let mut before: Option<i64> = None;
        loop {
            let mut query = format!("limit={limit}");
            if let Some(before) = before {
                query.push_str(&format!("&before={before}"));
            }
            let p = page(&h, &thread, &query).await;
            before = p["start"].as_i64();
            let earlier = p["earlier"].as_bool().unwrap();
            pages.push(p);
            if !earlier {
                break;
            }
        }
        let got: Vec<_> = pages.iter().rev().flat_map(frames_of).collect();
        assert_eq!(got, whole, "limit {limit}");
        for pair in pages.windows(2) {
            assert_eq!(
                pair[1]["end"].as_i64().unwrap() + 1,
                pair[0]["start"].as_i64().unwrap()
            );
        }
        if limit == 1 {
            assert_eq!(pages.len(), 5, "one turn a page");
        }
    }
}

#[tokio::test]
async fn the_newest_page_is_a_settled_point_a_connect_resumes_from() {
    let h = Harness::start().await;
    let thread = turns(&h, 3).await;
    let p = page(&h, &thread, "limit=2").await;
    assert_eq!(p["threadId"], thread.as_str());
    assert_eq!(p["projection"], orch_surface_agui::PROJECTION_VERSION);
    assert_eq!(p["end"], p["head"], "a settled thread is one page's end");
    assert_eq!(p["earlier"], true);
    let frames = p["frames"].as_array().unwrap();
    assert_eq!(frames[0]["event"]["type"], "RUN_STARTED");
    assert_eq!(frames.last().unwrap()["event"]["type"], "RUN_FINISHED");
    // every frame of the page is a frame of the stream, and the id is the stream's
    for f in frames {
        assert!(f["event"]["type"].is_string());
    }
    // a connect from `end` says nothing of what the page held, and the next run, in full
    let end = p["end"].as_i64().unwrap();
    let mut rest = h.connect(&thread, ALICE, Some(end)).await;
    assert!(
        rest.is_quiet_for(std::time::Duration::from_millis(300))
            .await
    );
    h.run(
        "plain",
        ALICE,
        &input(
            &thread,
            "run-9",
            &[
                ("m1", "echo turn 1"),
                ("m2", "echo turn 2"),
                ("m3", "echo turn 3"),
                ("m9", "echo next"),
            ],
        ),
    )
    .await
    .all()
    .await;
    let next = rest.through_run().await;
    assert_eq!(next.first().unwrap().kind(), "RUN_STARTED");
    // and the page, read again, has the new turn and the same older ones
    let again = page(&h, &thread, "limit=2").await;
    assert!(again["end"].as_i64().unwrap() > end);
}

#[tokio::test]
async fn the_chain_still_open_is_not_in_the_page_and_the_stream_says_it() {
    let h = Harness::start().await;
    let thread = turns(&h, 2).await;
    let settled = page(&h, &thread, "").await;
    // a turn that works for a while: its run is open
    let mut running = h
        .run(
            "plain",
            ALICE,
            &input(
                &thread,
                "run-3",
                &[
                    ("m1", "echo turn 1"),
                    ("m2", "echo turn 2"),
                    ("m3", "slow work"),
                ],
            ),
        )
        .await;
    running.until(|f| f.kind() == "SUBAGENT_STARTED").await;
    let p = page(&h, &thread, "").await;
    assert_eq!(p["end"], settled["end"], "the open chain is the stream's");
    assert!(p["head"].as_i64().unwrap() > p["end"].as_i64().unwrap());
    assert_eq!(frames_of(&p), frames_of(&settled));
    // the stream from `end` opens the run again, in full, from its RUN_STARTED
    let mut rest = h
        .connect(&thread, ALICE, Some(p["end"].as_i64().unwrap()))
        .await;
    let first = rest.next(T).await.unwrap();
    assert_eq!(first.kind(), "RUN_STARTED");
    h.post_empty(&format!("/api/threads/{thread}/cancel"), ALICE)
        .await;
}

#[tokio::test]
async fn a_catch_up_gives_the_chains_after_a_point_and_the_anchor_of_the_last_run() {
    let h = Harness::start().await;
    let thread = turns(&h, 3).await;
    let all = page(&h, &thread, "limit=100").await;
    let first = page(&h, &thread, "limit=1&before=1").await;
    assert_eq!(first["frames"].as_array().unwrap().len(), 0);
    let oldest = {
        let mut before = None::<i64>;
        loop {
            let q = before.map_or("limit=1".to_owned(), |b| format!("limit=1&before={b}"));
            let p = page(&h, &thread, &q).await;
            before = p["start"].as_i64();
            if !p["earlier"].as_bool().unwrap() {
                break p;
            }
        }
    };
    let after = oldest["end"].as_i64().unwrap();
    let rest = page(&h, &thread, &format!("after={after}")).await;
    assert_eq!(rest["start"].as_i64().unwrap(), after + 1);
    assert_eq!(rest["end"], all["end"]);
    assert!(rest["anchor"]["runId"].is_string(), "{rest}");
    assert!(rest["anchor"]["seq"].as_i64().unwrap() <= after);
    let tail: Vec<_> = frames_of(&oldest)
        .into_iter()
        .chain(frames_of(&rest))
        .collect();
    assert_eq!(tail, frames_of(&all));
}

#[tokio::test]
async fn every_refusal_comes_with_a_problem_and_reveals_nothing() {
    let h = Harness::start().await;
    let thread = turns(&h, 1).await;
    // 401: no identity
    h.history(&thread, None, "", None).await.problem(401);
    // 404: someone else's thread, one that does not exist, an id that is not one
    h.history(&thread, Some(BOB), "", None).await.problem(404);
    h.history(&new_thread_id(), Some(ALICE), "", None)
        .await
        .problem(404);
    h.history("not-a-uuid", Some(ALICE), "", None)
        .await
        .problem(404);
    // 400
    for query in [
        "before=0",
        "before=abc",
        "limit=0",
        "limit=101",
        "since=0",
        "after=0",
        "limit=1&since=2",
        "limit=1&after=2",
        "since=1&after=2",
        "before=3&after=2",
    ] {
        let p = h
            .history(&thread, Some(ALICE), query, None)
            .await
            .problem(400);
        assert!(p["detail"].is_string(), "{query}");
    }
    // 406: only JSON is answered
    h.history(&thread, Some(ALICE), "", Some("text/event-stream"))
        .await
        .problem(406);
    // 200 and what it says about itself
    let ok = h
        .history(&thread, Some(ALICE), "", Some("application/json"))
        .await;
    assert_eq!(ok.status, 200);
    assert!(ok.content_type.starts_with("application/json"));
    assert_eq!(ok.headers["cache-control"], "no-store");
    // 403: roles that hold nothing
    let denied = Harness::start_with_policy(orch_app::Policy::deny_all()).await;
    denied
        .history(&thread, Some(ALICE), "", None)
        .await
        .problem(403);
}

#[tokio::test]
async fn a_deleted_thread_is_a_404() {
    let h = Harness::start().await;
    let thread = turns(&h, 1).await;
    assert_eq!(h.history(&thread, Some(ALICE), "", None).await.status, 200);
    h.app
        .delete_thread(&orch_core::UserId::new(ALICE), thread.parse().unwrap())
        .await
        .unwrap();
    h.history(&thread, Some(ALICE), "", None).await.problem(404);
}

#[tokio::test]
async fn the_reads_are_counted_without_naming_anybody() {
    let h = Harness::start().await;
    let thread = turns(&h, 2).await;
    let before = h.app.history_stats();
    page(&h, &thread, "limit=1").await;
    let after = h.app.history_stats();
    assert_eq!(after.pages, before.pages + 1);
    assert!(after.events_folded > before.events_folded);
    let r = h.get("/metrics", None).await;
    let text = String::from_utf8(r.body).unwrap();
    assert!(text.contains("history_pages_total 1"), "{text}");
    assert!(text.contains("history_fold_seconds_total"), "{text}");
    assert!(!text.contains(ALICE) && !text.contains(&thread));
}

#[tokio::test]
async fn a_shared_thread_is_paged_over_the_reader_projection() {
    let h = Harness::start_shared(SharingMode::Public).await;
    let thread = turns(&h, 2).await;
    let token = h.share(&thread, ShareLevel::Public).await;

    // a signed-in reader
    let r = h.history_shared(&token, false, Some(BOB), "limit=1").await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.headers["cache-control"], "no-store");
    assert_eq!(r.headers["x-robots-tag"], "noindex, nofollow");
    let page = r.json();
    let text = page.to_string();
    assert!(!text.contains(ALICE), "the owner's address is shown");
    assert!(text.contains("the owner"));
    // the reader's pages tile the reader's stream
    let stream = h.connect_shared(&token, false, Some(BOB)).await;
    let whole = replay_of(&stream.until_quiet().await);
    let mut got = Vec::new();
    let mut before: Option<i64> = None;
    let mut pages = Vec::new();
    loop {
        let q = before.map_or("limit=1".to_owned(), |b| format!("limit=1&before={b}"));
        let r = h.history_shared(&token, false, Some(BOB), &q).await;
        let p = r.json();
        before = p["start"].as_i64();
        let earlier = p["earlier"].as_bool().unwrap();
        pages.push(p);
        if !earlier {
            break;
        }
    }
    for p in pages.iter().rev() {
        got.extend(frames_of(p));
    }
    assert_eq!(got, whole);

    // 401 without an identity, 404 for a link that does not work, one body for all
    h.history_shared(&token, false, None, "").await.problem(401);
    let bad = "A".repeat(43);
    let dead = h.history_shared(&bad, false, Some(BOB), "").await;
    dead.problem(404);
    // anybody, for a public link
    let r = h.history_shared(&token, true, None, "limit=1").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.headers["cache-control"], "no-store");
    assert_eq!(r.headers["x-robots-tag"], "noindex, nofollow");
    let dead_public = h.history_shared(&bad, true, None, "").await;
    dead_public.problem(404);
    assert_eq!(dead.body, dead_public.body, "one body for every dead link");
    // a link that is revoked is the same 404
    h.app
        .unshare_thread(&orch_core::UserId::new(ALICE), thread.parse().unwrap())
        .await
        .unwrap();
    h.history_shared(&token, true, None, "").await.problem(404);
    h.history_shared(&token, false, Some(BOB), "")
        .await
        .problem(404);
}

#[tokio::test]
async fn a_public_read_holds_a_stream_permit_while_it_folds_and_gives_it_back() {
    let h = Harness::start_shared_with(
        SharingMode::Public,
        PublicLimits {
            streams_per_link: 1,
            ..PublicLimits::default()
        },
    )
    .await;
    let thread = turns(&h, 2).await;
    let token = h.share(&thread, ShareLevel::Public).await;

    // reads one after the other each get the permit (it is released on every exit)
    for _ in 0..3 {
        let r = h.history_shared(&token, true, None, "limit=1").await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    }
    // a refusal releases it too
    h.history_shared(&token, true, None, "limit=0")
        .await
        .problem(400);
    assert_eq!(
        h.history_shared(&token, true, None, "").await.status,
        200,
        "the permit of the refused read was given back"
    );

    // while the link's one stream is open, a read is refused before the fold
    let open = h.connect_shared(&token, true, None).await;
    let r = h.history_shared(&token, true, None, "").await;
    r.problem(429);
    assert!(r.headers.contains_key("retry-after"));
    assert_eq!(r.json()["code"], "too_many_streams");
    drop(open);
    // and gets in again once it is closed (the permit comes back when the stream is dropped)
    let mut status = 0;
    for _ in 0..20 {
        status = h.history_shared(&token, true, None, "").await.status;
        if status == 200 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(status, 200);
}

#[tokio::test]
async fn config_says_the_route_exists_only_when_the_application_is_told_to() {
    let h = Harness::start().await;
    let r = h.get("/api/config", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    // this harness builds the application without the binary's settings: no `ui.history`
    assert_eq!(r.json(), json!({"ui": {"showDescriptions": true}}));
}

#[tokio::test]
async fn a_public_read_takes_its_permit_before_it_reads_anything() {
    // a link that may hold no stream at all: the read is refused with the connect's own 429, where
    // the same link's plain read (no fold, no permit) is served
    let h = Harness::start_shared_with(
        SharingMode::Public,
        PublicLimits {
            streams_per_link: 0,
            ..PublicLimits::default()
        },
    )
    .await;
    let thread = turns(&h, 1).await;
    let token = h.share(&thread, ShareLevel::Public).await;
    let r = h.history_shared(&token, true, None, "").await;
    r.problem(429);
    assert_eq!(r.json()["code"], "too_many_streams");
    assert!(r.headers.contains_key("retry-after"));
    let plain = h.get(&format!("/api/public/shared/{token}"), None).await;
    assert_eq!(
        plain.status,
        200,
        "{}",
        String::from_utf8_lossy(&plain.body)
    );
}
