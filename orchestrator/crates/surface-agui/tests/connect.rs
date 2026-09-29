//! `GET /agui/threads/{threadId}/connect`: replay, cursor, following across runs, several
//! viewers, `?mode=run`, and the refusals before the stream. Every frame that arrives is checked
//! against the vendored AG-UI schema by the harness.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use serde_json::json;
use support::*;

const QUIET: Duration = Duration::from_millis(400);

fn ids(frames: &[Frame]) -> Vec<i64> {
    frames.iter().filter_map(|f| f.id).collect()
}

/// The thread after a question and its answer: two runs, the first ends in an interrupt.
async fn asked_and_answered(h: &Harness, user: &str) -> String {
    let thread = new_thread_id();
    let first = h
        .run("plain", user, &input(&thread, "run-1", &[("m1", "ask me")]))
        .await
        .all()
        .await;
    assert_eq!(first.last().unwrap().event["outcome"]["type"], "interrupt");
    let answer = input_with(
        &thread,
        "run-2",
        &[("m1", "ask me")],
        json!({"resume": [{
            "interruptId": "int-3", "status": "resolved", "payload": {"text": "main"}
        }]}),
    );
    let second = h.run("plain", user, &answer).await.all().await;
    assert_eq!(second.last().unwrap().event["outcome"]["type"], "success");
    thread
}

#[tokio::test]
async fn a_connect_replays_a_finished_thread_from_the_start_and_stays_open() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let posted = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m1", "echo hi")]),
        )
        .await
        .all()
        .await;
    h.wait_state(ALICE, &thread, "done").await;

    let mut stream = h.connect(&thread, ALICE, None).await;
    assert_eq!(stream.status, 200);
    assert!(
        stream
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let replay = stream.through_run().await;
    assert_eq!(replay.first().unwrap().kind(), "RUN_STARTED");
    assert_eq!(replay.first().unwrap().event["runId"], "run-1");
    assert_eq!(replay.first().unwrap().event["threadId"], thread.as_str());
    assert_eq!(
        replay.last().unwrap().event["outcome"],
        json!({"type": "success"})
    );

    // A viewer is shown everything: the user's message too, which the requester holds already.
    let has_user_message = |frames: &[Frame]| {
        frames
            .iter()
            .any(|f| f.kind() == "TEXT_MESSAGE_START" && f.event["role"] == "user")
    };
    assert!(has_user_message(&replay));
    assert!(!has_user_message(&posted));
    let without_message: Vec<_> = replay
        .iter()
        .filter(|f| !f.kind().starts_with("TEXT_MESSAGE"))
        .map(|f| &f.event)
        .collect();
    let posted_events: Vec<_> = posted
        .iter()
        .filter(|f| !f.kind().starts_with("TEXT_MESSAGE"))
        .map(|f| &f.event)
        .collect();
    assert_eq!(
        without_message, posted_events,
        "the same run, the same events"
    );

    // Resume points are log sequence numbers, strictly increasing.
    let seen = ids(&replay);
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
    assert_eq!(*seen.last().unwrap(), 5);

    // Between runs the stream is idle, not closed: only keepalive comments arrive.
    assert!(stream.is_quiet_for(QUIET).await);
    assert!(stream.comments > 0, "no keepalive in {QUIET:?}");
}

#[tokio::test]
async fn a_connect_follows_the_runs_that_come_later() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &input(&thread, "run-1", &[("m1", "ask me")]),
    )
    .await
    .all()
    .await;

    let mut stream = h.connect(&thread, ALICE, None).await;
    let first = stream.through_run().await;
    assert_eq!(first.last().unwrap().event["outcome"]["type"], "interrupt");
    assert!(stream.is_quiet_for(Duration::from_millis(200)).await);

    // The answer arrives from another request (another tab, a replica): the stream carries it.
    let answer = input_with(
        &thread,
        "run-2",
        &[("m1", "ask me")],
        json!({"resume": [{
            "interruptId": first.last().unwrap().event["outcome"]["interrupts"][0]["id"],
            "status": "resolved", "payload": {"text": "main"}
        }]}),
    );
    let posted = h.run("plain", ALICE, &answer).await.all().await;
    let second = stream.through_run().await;
    assert_eq!(second.first().unwrap().kind(), "RUN_STARTED");
    assert_eq!(second.first().unwrap().event["runId"], "run-2");
    assert_eq!(
        second.last().unwrap().event["outcome"],
        json!({"type": "success"})
    );
    assert_eq!(second.last().unwrap().id, posted.last().unwrap().id);
    assert!(
        second
            .iter()
            .any(|f| f.event["activityType"] == "vymalo.artifact"),
        "the answered run produced its artifact"
    );
    // The projection is one and the same fold: what a fresh connect replays is what the stream
    // said, run after run.
    let fresh = h.connect_run(&thread, ALICE, None).await.all().await;
    let mut said = first;
    said.extend(second);
    assert_eq!(fresh, said);
}

#[tokio::test]
async fn several_viewers_each_get_the_whole_stream() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let mut requester = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m1", "gate hold")]),
        )
        .await;
    assert_eq!(requester.next(T).await.unwrap().kind(), "RUN_STARTED");
    h.wait_state(ALICE, &thread, "working").await;

    // Two viewers before the run ends, one that comes after.
    let mut early = h.connect(&thread, ALICE, None).await;
    let mut other_tab = h.connect(&thread, ALICE, None).await;
    let mut seen_early = early.until(|f| f.id == Some(2)).await;
    let mut seen_tab = other_tab.until(|f| f.id == Some(2)).await;
    h.agent.release_gate();
    seen_early.extend(early.through_run().await);
    seen_tab.extend(other_tab.through_run().await);
    h.wait_state(ALICE, &thread, "done").await;
    let late = h.connect_run(&thread, ALICE, None).await.all().await;

    assert_eq!(seen_early.first().unwrap().kind(), "RUN_STARTED");
    assert_eq!(seen_early.last().unwrap().kind(), "RUN_FINISHED");
    assert_eq!(seen_early, seen_tab, "two viewers, one stream");
    assert_eq!(seen_early, late, "and a late viewer is replayed the same");
    assert_eq!(h.agent.sends().len(), 1, "viewing delivers nothing");
}

#[tokio::test]
async fn reconnecting_in_the_middle_of_a_run_gets_the_preamble_and_the_rest_once() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let _requester = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m1", "gate hold")]),
        )
        .await;
    h.wait_state(ALICE, &thread, "working").await;

    // The client reads up to the agent starting to work (log events 1 and 2), then loses the
    // connection.
    let mut first = h.connect(&thread, ALICE, None).await;
    let held = first.until(|f| f.id == Some(2)).await;
    drop(first);

    let mut second = h.connect(&thread, ALICE, Some(2)).await;
    let opening = [
        second.next(T).await.unwrap(),
        second.next(T).await.unwrap(),
        second.next(T).await.unwrap(),
    ];
    assert_eq!(opening[0].kind(), "RUN_STARTED");
    assert_eq!(opening[0].event["runId"], "run-1", "the same run");
    assert_eq!(opening[1].kind(), "SUBAGENT_STARTED");
    assert_eq!(opening[1].event["subagentRunId"], "sub-2");
    assert_eq!(opening[2].kind(), "STATE_SNAPSHOT");
    assert!(opening.iter().all(|f| f.id.is_none()), "not resume points");
    assert!(second.is_quiet_for(Duration::from_millis(200)).await);

    h.agent.release_gate();
    let rest = second.through_run().await;
    assert_eq!(rest.last().unwrap().kind(), "RUN_FINISHED");

    // What the client held plus what the reconnect added is the stream, without a gap or a
    // repeat.
    let full = h.connect_run(&thread, ALICE, None).await.all().await;
    let mut joined = held;
    joined.extend(rest);
    assert_eq!(joined, full);
}

#[tokio::test]
async fn from_any_resume_point_the_rest_of_the_stream_and_nothing_else() {
    let h = Harness::start().await;
    let thread = asked_and_answered(&h, ALICE).await;
    let full = h.connect_run(&thread, ALICE, None).await.all().await;
    assert!(ids(&full).len() > 4, "{:?}", ids(&full));

    for (at, frame) in full.iter().enumerate() {
        let Some(cursor) = frame.id else { continue };
        let got = h
            .connect_run(&thread, ALICE, Some(cursor))
            .await
            .all()
            .await;
        let want = &full[at + 1..];
        // Idle at the cursor: exactly the rest. Inside a run: the run is opened again first
        // (RUN_STARTED, SUBAGENT_STARTED when an invocation is open, STATE_SNAPSHOT), and none
        // of that is a resume point.
        let skipped = got.len().checked_sub(want.len()).unwrap_or_else(|| {
            panic!("cursor {cursor}: fewer frames than the rest\n got {got:?}\nwant {want:?}")
        });
        assert!(skipped <= 3, "cursor {cursor}: preamble of {skipped}");
        assert_eq!(&got[skipped..], want, "cursor {cursor}");
        assert!(got[..skipped].iter().all(|f| f.id.is_none()));
        if skipped > 0 {
            assert_eq!(got[0].kind(), "RUN_STARTED", "cursor {cursor}");
        }
    }
}

#[tokio::test]
async fn a_cursor_at_the_end_waits_for_the_next_run() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let first = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m1", "ask me")]),
        )
        .await
        .all()
        .await;
    let last = first.last().unwrap().id.unwrap();

    let mut stream = h.connect(&thread, ALICE, Some(last)).await;
    assert!(
        stream.is_quiet_for(QUIET).await,
        "idle at the cursor: nothing to say"
    );
    // A cursor beyond the log (stale, forged) is the same.
    let mut beyond = h.connect(&thread, ALICE, Some(9_999)).await;
    assert!(beyond.is_quiet_for(Duration::from_millis(200)).await);

    let answer = input_with(
        &thread,
        "run-2",
        &[("m1", "ask me")],
        json!({"resume": [{
            "interruptId": "int-3", "status": "resolved", "payload": {"text": "main"}
        }]}),
    );
    h.run("plain", ALICE, &answer).await.all().await;
    let next = stream.through_run().await;
    assert_eq!(next.first().unwrap().kind(), "RUN_STARTED");
    assert_eq!(next.first().unwrap().event["runId"], "run-2");
    let next_beyond = beyond.through_run().await;
    assert_eq!(next_beyond, next);
}

#[tokio::test]
async fn mode_run_ends_after_the_replay_of_an_idle_thread() {
    let h = Harness::start().await;
    let thread = asked_and_answered(&h, ALICE).await;
    h.wait_state(ALICE, &thread, "done").await;
    let frames = h.connect_run(&thread, ALICE, None).await.all().await;
    assert_eq!(
        frames
            .iter()
            .filter(|f| f.kind() == "RUN_STARTED")
            .map(|f| f.event["runId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["run-1", "run-2"]
    );
    // At the end of the log there is nothing to say and nothing to wait for.
    let last = frames.last().unwrap().id.unwrap();
    let mut stream = h.connect_run(&thread, ALICE, Some(last)).await;
    assert!(stream.next(T).await.is_none());
    assert!(stream.ended());
}

#[tokio::test]
async fn mode_run_follows_the_active_run_to_its_end() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let _requester = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m1", "gate hold")]),
        )
        .await;
    h.wait_state(ALICE, &thread, "working").await;

    let mut whole = h.connect_run(&thread, ALICE, None).await;
    let mut from_middle = h.connect_run(&thread, ALICE, Some(2)).await;
    let mut seen = whole.until(|f| f.id == Some(2)).await;
    let opening = from_middle.until(|f| f.kind() == "STATE_SNAPSHOT").await;
    assert_eq!(opening.first().unwrap().kind(), "RUN_STARTED");
    // Open run: neither stream is over.
    assert!(whole.is_quiet_for(Duration::from_millis(250)).await);
    assert!(from_middle.is_quiet_for(Duration::from_millis(100)).await);

    h.agent.release_gate();
    seen.extend(whole.through_run().await);
    let rest = from_middle.through_run().await;
    assert_eq!(seen.last().unwrap().kind(), "RUN_FINISHED");
    assert_eq!(rest.last(), seen.last());
    // The run's terminal event is the last thing said: the streams close.
    for stream in [&mut whole, &mut from_middle] {
        assert!(stream.next(T).await.is_none());
        assert!(stream.ended());
    }
}

#[tokio::test]
async fn closing_a_connection_never_cancels_the_run() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let requester = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m1", "gate hold")]),
        )
        .await;
    h.wait_state(ALICE, &thread, "working").await;
    let mut viewer = h.connect(&thread, ALICE, None).await;
    viewer.until(|f| f.id == Some(2)).await;
    drop(viewer);
    drop(requester);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.state(ALICE, &thread).await, "working");
    h.agent.release_gate();
    h.wait_state(ALICE, &thread, "done").await;
    assert!(
        !h.agent
            .calls()
            .iter()
            .any(|c| matches!(c, orch_ports::memory::Call::Cancel { .. })),
        "no CancelTask was sent"
    );
}

#[tokio::test]
async fn a_stream_ends_when_the_process_shuts_down_and_the_client_reconnects_elsewhere() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &input(&thread, "run-1", &[("m1", "ask me")]),
    )
    .await
    .all()
    .await;
    let mut stream = h.connect(&thread, ALICE, None).await;
    let seen = stream.through_run().await;
    h.app.set_shutting_down();
    // Caught up, and the process is going away: the stream ends (truncated, no terminal event
    // is invented) and the client reconnects with its cursor.
    assert!(stream.next(T).await.is_none());
    assert!(stream.ended());
    assert!(ids(&seen).last().is_some());
}

#[tokio::test]
async fn refusals_come_before_the_stream_and_reveal_nothing() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run("plain", BOB, &input(&thread, "run-b", &[("mb", "ask me")]))
        .await
        .all()
        .await;
    let mine = new_thread_id();
    h.run("plain", ALICE, &input(&mine, "run-a", &[("ma", "echo")]))
        .await
        .all()
        .await;

    let get = |thread: &str, user: Option<&str>, last: Option<&str>, query: Option<&str>| {
        let (thread, user, last, query) = (
            thread.to_owned(),
            user.map(str::to_owned),
            last.map(str::to_owned),
            query.map(str::to_owned),
        );
        let h = &h;
        async move {
            resp_of(
                h.connect_raw(&thread, user.as_deref(), last.as_deref(), query.as_deref())
                    .await,
            )
            .await
        }
    };

    // Someone else's thread, a thread that does not exist, and an id that is no UUID: one answer.
    let foreign = get(&thread, Some(ALICE), None, None).await.problem(404);
    let missing = get(&new_thread_id(), Some(ALICE), None, None)
        .await
        .problem(404);
    let malformed = get("not-a-uuid", Some(ALICE), None, None)
        .await
        .problem(404);
    for other in [&missing, &malformed] {
        assert_eq!(other["title"], foreign["title"]);
        assert_eq!(other["detail"], foreign["detail"]);
        assert_eq!(other["type"], foreign["type"]);
    }
    assert!(!foreign.to_string().contains(BOB), "{foreign}");
    assert!(!foreign.to_string().contains(&thread), "{foreign}");
    // With a cursor and a mode as well.
    get(&thread, Some(ALICE), Some("3"), Some("mode=run"))
        .await
        .problem(404);
    // The owner is still served.
    assert_eq!(
        get(&thread, Some(BOB), None, Some("mode=run")).await.status,
        200
    );

    // No identity: 401, fail closed.
    get(&mine, None, None, None).await.problem(401);
    // A cursor that is not a non-negative integer, a mode that is not `run`, an Accept that
    // excludes the stream.
    for bad in ["-1", "abc", "1.5", "0x10"] {
        get(&mine, Some(ALICE), Some(bad), None).await.problem(400);
    }
    for bad in ["mode=follow", "mode=", "mode=RUN"] {
        get(&mine, Some(ALICE), None, Some(bad)).await.problem(400);
    }
    let not_acceptable = h
        .client
        .get(h.url(&format!("/agui/threads/{mine}/connect")))
        .header("X-Auth-Request-Email", ALICE)
        .header("Accept", "application/json")
        .send()
        .await
        .unwrap();
    resp_of(not_acceptable).await.problem(406);
    // An empty cursor is no cursor.
    assert_eq!(
        get(&mine, Some(ALICE), Some(""), Some("mode=run"))
            .await
            .status,
        200
    );
}
