//! Reading a shared thread over AG-UI (ADR 0040, section 8): the connect stream over the reader
//! projection, read-only, for a signed-in reader and for anybody; what it says and does not say;
//! the one 404; a stream that ends when the link does; and the ceiling on public streams.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_api::PublicLimits;
use orch_app::SharingMode;
use orch_core::ShareLevel;
use serde_json::json;
use support::*;

/// A thread that has run `echo hi` to its end, shared at `level`: `(thread, token)`.
async fn shared(h: &Harness, level: ShareLevel) -> (String, String) {
    let thread = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &input(&thread, "run-1", &[("m1", "echo hi")]),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    let token = h.share(&thread, level).await;
    (thread, token)
}

#[tokio::test]
async fn a_signed_in_reader_is_replayed_the_thread_without_its_owner() {
    let h = Harness::start_shared(SharingMode::Internal).await;
    let (thread, token) = shared(&h, ShareLevel::Internal).await;

    let mut stream = h.connect_shared(&token, false, Some(BOB)).await;
    assert_eq!(
        stream.headers["cache-control"].to_str().unwrap(),
        "no-store, no-transform"
    );
    assert_eq!(
        stream.headers["x-robots-tag"].to_str().unwrap(),
        "noindex, nofollow"
    );
    let replay = stream.through_run().await;
    assert_eq!(replay.first().unwrap().kind(), "RUN_STARTED");
    assert_eq!(replay.first().unwrap().event["threadId"], thread.as_str());
    assert_eq!(
        replay.last().unwrap().event["outcome"],
        json!({"type": "success"})
    );
    // what the person said is there, and never whose it is
    let text = serde_json::to_string(&replay.iter().map(|f| &f.event).collect::<Vec<_>>()).unwrap();
    assert!(text.contains("echo hi"), "the messages are shown");
    assert!(
        !text.contains(ALICE),
        "the owner's address is shown: {text}"
    );
    assert!(text.contains("the owner"));

    // the same replay as the owner's own connect, frame for frame, but for who said it
    let mut own = h.connect(&thread, ALICE, None).await;
    let own = own.through_run().await;
    assert_eq!(kinds(&replay), kinds(&own));

    // a cursor resumes after it, as on any connect
    let last = replay.iter().rev().find_map(|f| f.id).unwrap();
    let resp = h
        .connect_shared_raw(
            &token,
            false,
            Some(BOB),
            Some(&last.to_string()),
            "text/event-stream",
        )
        .await;
    assert_eq!(resp.status().as_u16(), 200);
    let mut resumed = Stream::new(resp);
    assert!(resumed.is_quiet_for(Duration::from_millis(400)).await);
}

#[tokio::test]
async fn a_reader_follows_what_the_owner_writes_after_sharing() {
    let h = Harness::start_shared(SharingMode::Internal).await;
    let (thread, token) = shared(&h, ShareLevel::Internal).await;
    let mut stream = h.connect_shared(&token, false, Some(BOB)).await;
    stream.through_run().await;

    h.run(
        "plain",
        ALICE,
        &input(&thread, "run-2", &[("m1", "echo hi"), ("m2", "echo more")]),
    )
    .await
    .all()
    .await;
    let next = stream.through_run().await;
    assert_eq!(next.first().unwrap().kind(), "RUN_STARTED");
    let text = serde_json::to_string(&next.iter().map(|f| &f.event).collect::<Vec<_>>()).unwrap();
    assert!(text.contains("echo more"));
    assert!(!text.contains(ALICE));
}

#[tokio::test]
async fn every_refusal_is_before_the_stream_and_a_dead_link_is_one_404() {
    let h = Harness::start_shared(SharingMode::Public).await;
    let (_, token) = shared(&h, ShareLevel::Internal).await;

    // no identity on the signed-in route
    let r = resp_of(
        h.connect_shared_raw(&token, false, None, None, "text/event-stream")
            .await,
    )
    .await;
    r.problem(401);
    // a bad cursor, an Accept that excludes the stream
    let r = resp_of(
        h.connect_shared_raw(&token, false, Some(BOB), Some("x"), "text/event-stream")
            .await,
    )
    .await;
    r.problem(400);
    let r = resp_of(
        h.connect_shared_raw(&token, false, Some(BOB), None, "application/json")
            .await,
    )
    .await;
    r.problem(406);

    // every link that does not work: the same 404, and the same body, on both routes
    let mut bodies = Vec::new();
    for (public, who) in [(false, Some(BOB)), (true, None)] {
        for bad in ["A".repeat(43), "short".to_owned(), new_thread_id()] {
            let r = resp_of(
                h.connect_shared_raw(&bad, public, who, None, "text/event-stream")
                    .await,
            )
            .await;
            r.problem(404);
            bodies.push((public, r.body.clone()));
        }
    }
    // an internal link is no public one
    let r = resp_of(
        h.connect_shared_raw(&token, true, None, None, "text/event-stream")
            .await,
    )
    .await;
    r.problem(404);
    bodies.push((true, r.body.clone()));
    for public in [false, true] {
        let mut same = bodies.iter().filter(|(p, _)| *p == public).map(|(_, b)| b);
        let first = same.next().unwrap();
        assert!(same.all(|b| b == first), "one body for every cause");
    }

    // a person whose roles grant nothing is refused before the link is looked at
    let denied = Harness::start_sharing(
        orch_api::ApiConfig::default(),
        orch_auth_header::HeaderAuth::new(),
        orch_app::Policy::deny_all(),
        sharing(SharingMode::Public),
    )
    .await;
    let r = resp_of(
        denied
            .connect_shared_raw(&token, false, Some(BOB), None, "text/event-stream")
            .await,
    )
    .await;
    r.problem(403);
}

#[tokio::test]
async fn a_reader_can_send_nothing_and_the_threads_own_routes_stay_the_owners() {
    let h = Harness::start_shared(SharingMode::Public).await;
    let (thread, _token) = shared(&h, ShareLevel::Public).await;
    // Bob has the link and not the thread: every route of the thread is the 404 it always was
    let r = h
        .refused(
            "plain",
            Some(BOB),
            &input(&thread, "run-9", &[("m1", "hi bob")]),
        )
        .await;
    r.problem(404);
    let resp = h.connect_raw(&thread, Some(BOB), None, None).await;
    resp_of(resp).await.problem(404);
    let r = h
        .get(&format!("/agui/threads/{thread}/connect"), None)
        .await;
    assert_eq!(r.status, 401);
    // nothing was written by the refusals
    let events = h.events(ALICE, &thread).await;
    assert!(
        !events.iter().any(|e| e["data"]["text"] == "hi bob"),
        "{events:?}"
    );
}

#[tokio::test]
async fn anybody_reads_a_public_link_and_the_identity_they_send_is_ignored() {
    let h = Harness::start_shared(SharingMode::Public).await;
    let (thread, token) = shared(&h, ShareLevel::Public).await;

    let mut stream = h.connect_shared(&token, true, None).await;
    assert_eq!(
        stream.headers["cache-control"].to_str().unwrap(),
        "no-store, no-transform"
    );
    let replay = stream.through_run().await;
    assert_eq!(replay.first().unwrap().event["threadId"], thread.as_str());
    let text = serde_json::to_string(&replay.iter().map(|f| &f.event).collect::<Vec<_>>()).unwrap();
    assert!(
        !text.contains(ALICE) && text.contains("the owner"),
        "{text}"
    );

    // an identity header changes nothing: no 401 for a bad one, no owner for a good one
    for who in [ALICE, "nobody@example.com"] {
        let mut s = h.connect_shared(&token, true, Some(who)).await;
        let frames = s.through_run().await;
        assert_eq!(kinds(&frames), kinds(&replay), "{who} reads as nobody");
    }
}

#[tokio::test]
async fn a_revocation_ends_an_open_stream_and_the_reconnect_is_a_404() {
    let h = Harness::start_shared(SharingMode::Public).await;
    let (thread, token) = shared(&h, ShareLevel::Public).await;
    let mut public = h.connect_shared(&token, true, None).await;
    let mut internal = h.connect_shared(&token, false, Some(BOB)).await;
    public.through_run().await;
    internal.through_run().await;

    h.app
        .unshare_thread(&orch_core::UserId::new(ALICE), thread.parse().unwrap())
        .await
        .unwrap();
    for stream in [&mut public, &mut internal] {
        while stream.next(Duration::from_secs(5)).await.is_some() {}
        assert!(stream.ended(), "the stream ends with the link");
    }
    for (public, who) in [(true, None), (false, Some(BOB))] {
        let r = resp_of(
            h.connect_shared_raw(&token, public, who, None, "text/event-stream")
                .await,
        )
        .await;
        r.problem(404);
    }
}

#[tokio::test]
async fn public_streams_are_held_per_link_and_in_all() {
    let h = Harness::start_shared_with(
        SharingMode::Public,
        PublicLimits {
            per_link_per_second: 100,
            total_per_second: 100,
            streams_per_link: 2,
            streams_total: 3,
        },
    )
    .await;
    let (_, token) = shared(&h, ShareLevel::Public).await;
    let (_, other) = shared(&h, ShareLevel::Public).await;

    let mut a = h.connect_shared(&token, true, None).await;
    let _b = h.connect_shared(&token, true, None).await;
    a.through_run().await;
    // a third stream of the same link: refused before any stream byte
    let r = resp_of(
        h.connect_shared_raw(&token, true, None, None, "text/event-stream")
            .await,
    )
    .await;
    let problem = r.problem(429);
    assert_eq!(problem["code"], "too_many_streams");
    assert!(r.headers.get("retry-after").is_some());
    assert_eq!(r.headers["cache-control"], "no-store");
    // another link has its own, up to the total
    let _c = h.connect_shared(&other, true, None).await;
    let r = resp_of(
        h.connect_shared_raw(&other, true, None, None, "text/event-stream")
            .await,
    )
    .await;
    r.problem(429);
    // a closed stream gives its room back
    drop(a);
    let mut again = None;
    for _ in 0..50 {
        let resp = h
            .connect_shared_raw(&token, true, None, None, "text/event-stream")
            .await;
        if resp.status().as_u16() == 200 {
            again = Some(Stream::new(resp));
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(again.is_some(), "the room of a closed stream is given back");
    // the signed-in route holds no permit: any number of readers
    for _ in 0..5 {
        let _s = h.connect_shared(&token, false, Some(BOB)).await;
    }
}
