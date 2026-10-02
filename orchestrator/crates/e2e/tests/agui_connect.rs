//! The AG-UI connect stream and the capabilities document against the real stack: routes over
//! HTTP, the dispatcher, the A2A adapter and an in-process fake A2A agent, on the in-memory
//! store and on Postgres, with several orchestrator processes on one database. Every event that
//! leaves a route is checked against the vendored AG-UI schema.
//!
//! The goldens (`docs/api/examples/agui/connect-<name>.agui.json`, `capabilities-<agent>.json`)
//! are what a viewer reads for each scripted behaviour. `UPDATE_GOLDEN=1 cargo test -p orch-e2e
//! --test agui_connect` rewrites them; without it any difference fails the test.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_agui_proto::testkit::{assert_capabilities_json_conforms, assert_json_conforms};
use orch_testsupport::{Chat, Frame, SseClient, VerifierScript, eventually, with_ui_catalog};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);

fn thread_id(n: u32) -> String {
    format!("00000000-0000-7000-8000-{:012}", 200 + n)
}

fn input(thread: &str, run: &str, messages: &[(&str, &str)], extra: Value) -> Value {
    Chat::agui_input(thread, run, messages, extra)
}

fn conforming(frames: Vec<Frame>) -> Vec<Frame> {
    for frame in &frames {
        assert_json_conforms(&frame.event);
    }
    frames
}

/// Frames up to and including the first one `stop` accepts; the stream stays open.
async fn until(sse: &mut SseClient, stop: impl Fn(&Frame) -> bool) -> Vec<Frame> {
    conforming(sse.frames_until(WAIT, stop).await)
}

fn is_terminal(frame: &Frame) -> bool {
    matches!(
        frame.event["type"].as_str(),
        Some("RUN_FINISHED" | "RUN_ERROR")
    )
}

/// Frames up to and including the next terminal event of a run.
async fn through_run(sse: &mut SseClient) -> Vec<Frame> {
    until(sse, is_terminal).await
}

/// Everything up to the end of the stream.
async fn whole(mut sse: SseClient) -> Vec<Frame> {
    assert_eq!(sse.status, 200);
    conforming(sse.collect_frames(WAIT).await)
}

fn kind(frame: &Frame) -> &str {
    frame.event["type"].as_str().unwrap()
}

fn ids(frames: &[Frame]) -> Vec<i64> {
    frames.iter().filter_map(|f| f.id).collect()
}

/// The viewer's dispatching replica dies with its viewer connected and the agent's task parked;
/// the client reconnects to another replica with the last id it saw and is sent exactly what
/// it missed: the run opened again, then the rest, without a gap or a repeat.
async fn a_client_reconnects_to_another_replica_after_its_replica_dies(backend: Backend) {
    let world = World::start(backend).await;
    let a = world.instance("orch-a").await;
    let chat_a = world.chat(&a);
    let thread = thread_id(1);

    let _requester = chat_a
        .agui_run(
            "plain",
            &input(
                &thread,
                "run-1",
                &[("m1", "messages gate crash")],
                json!({}),
            ),
        )
        .await;
    let mut viewer = chat_a.agui_connect(&thread, None, false).await;
    // The log: 1 user message, 2 the agent works, 3 and 4 what it said; then it waits at its gate.
    let held = until(&mut viewer, |f| f.id == Some(4)).await;
    assert_eq!(kind(&held[0]), "RUN_STARTED");
    assert_eq!(ids(&held), [1, 2, 3, 4]);

    // The replica dies: nothing is released, and the client's connection breaks.
    a.kill();
    assert!(viewer.next_frame(WAIT).await.is_none());
    assert!(viewer.ended(), "the connection broke");

    let b = world.instance("orch-b").await;
    let chat_b = world.chat(&b);
    let mut resumed = chat_b.agui_connect(&thread, Some(4), false).await;
    let opening = until(&mut resumed, |f| kind(f) == "STATE_SNAPSHOT").await;
    assert_eq!(
        opening.iter().map(kind).collect::<Vec<_>>(),
        ["RUN_STARTED", "SUBAGENT_STARTED", "STATE_SNAPSHOT"],
        "the run, opened again for a client that missed its start"
    );
    assert_eq!(opening[0].event["runId"], "run-1");
    assert!(opening.iter().all(|f| f.id.is_none()));

    // The new replica takes the task over; the agent finishes while the client is connected to it.
    eventually(
        "the new replica re-attaches to the running task",
        || async { (world.plain.rpc_count("subscribe_to_task") >= 1).then_some(()) },
    )
    .await;
    world.plain.release_gate();
    let rest = through_run(&mut resumed).await;
    assert_eq!(kind(rest.last().unwrap()), "RUN_FINISHED");
    assert_eq!(
        rest.last().unwrap().event["outcome"],
        json!({"type": "success"})
    );

    // What the client held plus what the reconnect added is the stream, once.
    let full = whole(chat_b.agui_connect(&thread, None, true).await).await;
    let mut joined = held;
    joined.extend(rest);
    assert_eq!(joined, full, "no gap and no duplicate");
    let seen = ids(&joined);
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
    assert_eq!(*seen.last().unwrap(), 7);

    let events = chat_b.events(&thread).await;
    assert_contiguous(&events);
    assert_eq!(world.plain.executions().len(), 1, "delivered once");
}

/// Viewers on different replicas, and the requester on a third connection, each get the whole
/// stream; a viewer that joins in the middle of the run gets the preamble and the same rest.
async fn viewers_on_several_replicas_each_get_the_whole_stream(backend: Backend) {
    let world = World::start(backend).await;
    let api_only = world.instance_with("orch-a", false).await;
    let dispatching = world.instance("orch-b").await;
    let (chat_a, chat_b) = (world.chat(&api_only), world.chat(&dispatching));
    let thread = thread_id(2);

    let mut requester = chat_b
        .agui_run(
            "plain",
            &input(&thread, "run-1", &[("m1", "gate together")], json!({})),
        )
        .await;
    let mut viewer_a = chat_a.agui_connect(&thread, None, false).await;
    let mut viewer_b = chat_b.agui_connect(&thread, None, false).await;
    let mut tab_a = chat_a.agui_connect(&thread, None, false).await;
    // The log: 1 user message, 2 the agent works; then it waits at its gate.
    let mut seen_a = until(&mut viewer_a, |f| f.id == Some(2)).await;
    let mut seen_b = until(&mut viewer_b, |f| f.id == Some(2)).await;
    let mut seen_tab = until(&mut tab_a, |f| f.id == Some(2)).await;
    let mut joined = chat_a.agui_connect(&thread, Some(2), true).await;
    let opening = until(&mut joined, |f| kind(f) == "STATE_SNAPSHOT").await;
    assert_eq!(kind(&opening[0]), "RUN_STARTED");

    world.plain.release_gate();
    seen_a.extend(through_run(&mut viewer_a).await);
    seen_b.extend(through_run(&mut viewer_b).await);
    seen_tab.extend(through_run(&mut tab_a).await);
    let rest = through_run(&mut joined).await;
    assert_eq!(seen_a, seen_b, "a viewer on either replica");
    assert_eq!(seen_a, seen_tab, "and one in another tab");
    assert_eq!(rest[..], seen_a[seen_a.len() - rest.len()..]);
    assert_eq!(kind(seen_a.last().unwrap()), "RUN_FINISHED");

    // The requester's response is the same run, minus the message it sent.
    let posted = conforming(requester.collect_frames(WAIT).await);
    let without_user = |frames: &[Frame]| -> Vec<Value> {
        frames
            .iter()
            .filter(|f| !kind(f).starts_with("TEXT_MESSAGE"))
            .map(|f| f.event.clone())
            .collect()
    };
    assert_eq!(without_user(&posted), without_user(&seen_a));
    assert_eq!(
        world.plain.executions().len(),
        1,
        "viewing delivers nothing"
    );
}

/// From every resume point of a thread with agent messages and two runs, on either replica, the
/// stream is exactly what follows it.
async fn from_every_resume_point_the_rest_and_nothing_else(backend: Backend) {
    let world = World::start(backend).await;
    let a = world.instance_with("orch-a", false).await;
    let b = world.instance("orch-b").await;
    let (chat_a, chat_b) = (world.chat(&a), world.chat(&b));
    let thread = thread_id(3);

    let first = conforming(
        chat_b
            .agui_run(
                "plain",
                &input(&thread, "run-1", &[("m1", "ask about it")], json!({})),
            )
            .await
            .collect_frames(WAIT)
            .await,
    );
    let interrupt = first.last().unwrap().event["outcome"]["interrupts"][0]["id"].clone();
    conforming(
        chat_b
            .agui_run(
                "plain",
                &input(
                    &thread,
                    "run-2",
                    &[("m1", "ask about it")],
                    json!({"resume": [{
                        "interruptId": interrupt, "status": "resolved", "payload": {"text": "main"}
                    }]}),
                ),
            )
            .await
            .collect_frames(WAIT)
            .await,
    );
    chat_b.wait_state(&thread, "done").await;

    let full = whole(chat_a.agui_connect(&thread, None, true).await).await;
    assert_eq!(full.iter().filter(|f| kind(f) == "RUN_STARTED").count(), 2);
    for (at, frame) in full.iter().enumerate() {
        let Some(cursor) = frame.id else { continue };
        // Alternate the replica: the answer is the same from either.
        let chat = if cursor % 2 == 0 { &chat_a } else { &chat_b };
        let got = whole(chat.agui_connect(&thread, Some(cursor), true).await).await;
        let want = &full[at + 1..];
        let skipped = got.len().checked_sub(want.len()).unwrap();
        assert!(skipped <= 3, "cursor {cursor}: preamble of {skipped}");
        assert_eq!(&got[skipped..], want, "cursor {cursor}");
        assert!(got[..skipped].iter().all(|f| f.id.is_none()));
    }
}

/// A message sent while the agent works ends the run it arrives in and opens its own (ADR 0036):
/// a client that reconnects with `Last-Event-ID`, from any resume point and on either replica, gets
/// exactly the rest, whether the message was a steer or a stop.
async fn from_every_resume_point_of_a_thread_with_a_mid_run_message_the_rest_and_nothing_else(
    backend: Backend,
) {
    let world = World::start(backend).await;
    let a = world.instance_with("orch-a", false).await;
    let b = world.instance("orch-b").await;
    let (chat_a, chat_b) = (world.chat(&a), world.chat(&b));
    for (n, (first, how, text)) in [
        ("gate hold", "steer", "echo hurry"),
        ("slow work", "interrupt", "echo do X instead"),
    ]
    .into_iter()
    .enumerate()
    {
        let thread = thread_id(30 + u32::try_from(n).unwrap());
        let open = chat_b
            .agui_run(
                "plain",
                &input(&thread, "run-1", &[("m1", first)], json!({})),
            )
            .await;
        chat_b.wait_state(&thread, "working").await;
        let second = chat_b
            .agui_run(
                "plain",
                &input(
                    &thread,
                    "run-2",
                    &[("m1", first), ("m2", text)],
                    json!({"forwardedProps": {"vymalo.send": how}}),
                ),
            )
            .await;
        whole(open).await;
        if how == "steer" {
            world.plain.release_gate();
        }
        whole(second).await;
        // the job the message started, through to its end
        eventually("job 2 to be done", || async {
            let events = chat_b.events(&thread).await;
            let at = events.iter().position(|e| e["kind"] == "job_started")?;
            events[at..]
                .iter()
                .any(|e| e["kind"] == "thread_state" && e["data"]["state"] == "done")
                .then_some(())
        })
        .await;

        let full = whole(chat_a.agui_connect(&thread, None, true).await).await;
        let runs: Vec<&str> = full
            .iter()
            .filter(|f| kind(f) == "RUN_STARTED")
            .map(|f| f.event["runId"].as_str().unwrap())
            .collect();
        assert_eq!(runs[..2], ["run-1", "run-2"], "{how}: {runs:?}");
        for (at, frame) in full.iter().enumerate() {
            let Some(cursor) = frame.id else { continue };
            let chat = if cursor % 2 == 0 { &chat_a } else { &chat_b };
            let got = whole(chat.agui_connect(&thread, Some(cursor), true).await).await;
            let want = &full[at + 1..];
            let skipped = got.len().checked_sub(want.len()).unwrap();
            assert!(
                skipped <= 3,
                "{how}, cursor {cursor}: preamble of {skipped}"
            );
            assert_eq!(&got[skipped..], want, "{how}, cursor {cursor}");
            assert!(got[..skipped].iter().all(|f| f.id.is_none()));
        }
    }
}

/// Another owner's thread, a thread nobody has, and no identity: refused as problems before the
/// stream, the same on every replica.
async fn a_thread_that_is_not_yours_is_a_404_before_the_stream(backend: Backend) {
    let world = World::start(backend).await;
    let a = world.instance("orch-a").await;
    let b = world.instance_with("orch-b", false).await;
    let alice = world.chat(&a);
    let bob = alice.as_user(BOB);
    let thread = thread_id(4);
    conforming(
        bob.agui_run(
            "plain",
            &input(&thread, "run-b", &[("mb", "ask about it")], json!({})),
        )
        .await
        .collect_frames(WAIT)
        .await,
    );

    let mut bodies = Vec::new();
    for chat in [alice.clone(), world.chat(&b)] {
        for id in [thread.as_str(), &thread_id(99), "nope"] {
            let resp = chat.agui_connect_raw(id, None, None).await;
            assert_eq!(resp.status().as_u16(), 404, "{id}");
            let content_type = resp.headers()["content-type"].to_str().unwrap().to_owned();
            assert!(content_type.starts_with("application/problem+json"));
            let mut body: Value = resp.json().await.unwrap();
            assert!(!body.to_string().contains(BOB), "{body}");
            body.as_object_mut().unwrap().remove("instance");
            bodies.push(body);
        }
    }
    assert!(bodies.windows(2).all(|w| w[0] == w[1]), "{bodies:#?}");
    let anonymous = alice
        .anonymous()
        .agui_connect_raw(&thread, None, None)
        .await;
    assert_eq!(anonymous.status().as_u16(), 401);
    // Bob still reads his thread.
    let replay = whole(bob.agui_connect(&thread, None, true).await).await;
    assert_eq!(kind(replay.last().unwrap()), "RUN_FINISHED");
}

backends!(
    a_client_reconnects_to_another_replica_after_its_replica_dies,
    viewers_on_several_replicas_each_get_the_whole_stream,
    from_every_resume_point_the_rest_and_nothing_else,
    from_every_resume_point_of_a_thread_with_a_mid_run_message_the_rest_and_nothing_else,
    a_thread_that_is_not_yours_is_a_404_before_the_stream,
);

// ---- capabilities -----------------------------------------------------------------------

async fn capabilities_are_the_live_card_in_the_spec_shape(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);

    let (status, coder) = chat.agui_capabilities("coder").await;
    assert_eq!(status, 200);
    assert_capabilities_json_conforms(&coder);
    assert_eq!(coder["identity"]["name"], "Coder");
    assert_eq!(coder["identity"]["version"], "1.0.0");
    assert_eq!(
        coder["transport"],
        json!({"streaming": true, "resumable": true})
    );
    let releases = &coder["custom"]["https://agents.vymalo.com/a2a/extensions/release-channels/v1"];
    assert_eq!(releases["defaultChannel"], "production");
    assert_eq!(releases["channels"]["staging"], "coder-r51");

    let (status, plain) = chat.agui_capabilities("plain").await;
    assert_eq!(status, 200);
    assert_capabilities_json_conforms(&plain);
    assert!(plain.get("custom").is_none(), "no extension, none declared");

    assert_eq!(chat.agui_capabilities("nobody").await.0, 404);
    assert_eq!(chat.anonymous().agui_capabilities("plain").await.0, 401);
}

backends!(capabilities_are_the_live_card_in_the_spec_shape);

// ---- goldens ----------------------------------------------------------------------------

/// A fork (ADR 0029): a viewer of it reads the parent's frames up to the cut, in the same order and
/// with the same ids, then the marker `vymalo.fork` and a snapshot that says where the thread came
/// from; and a screen that goes on with the messages it holds (the copy's ids) is accepted.
async fn a_fork_replays_the_parents_frames_then_the_marker_and_takes_a_run_with_the_histories_ids(
    backend: Backend,
) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let parent = chat.create_thread("plain", "echo one", None).await;
    chat.wait_state(&parent, "done").await;
    let log = chat.events(&parent).await;
    let cut = log.last().unwrap()["seq"].as_i64().unwrap();
    let (status, forked) = chat
        .post(
            &format!("/api/threads/{parent}/fork"),
            Some(json!({"after": cut})),
        )
        .await;
    assert_eq!(status, 201, "{forked}");
    let fork = forked["id"].as_str().unwrap().to_owned();

    let theirs = whole(chat.agui_connect(&parent, None, true).await).await;
    let ours = whole(chat.agui_connect(&fork, None, true).await).await;
    let shape = |frames: &[Frame]| -> Vec<(String, Option<i64>)> {
        frames.iter().map(|f| (kind(f).to_owned(), f.id)).collect()
    };
    assert_eq!(
        shape(&ours[..theirs.len()]),
        shape(&theirs),
        "the parent's frames come first, with the parent's resume points"
    );
    let marker = &ours[theirs.len()..];
    assert_eq!(
        marker.iter().map(kind).collect::<Vec<_>>(),
        [
            "RUN_STARTED",
            "ACTIVITY_SNAPSHOT",
            "STATE_SNAPSHOT",
            "RUN_FINISHED"
        ]
    );
    assert_eq!(marker[1].event["activityType"], "vymalo.fork");
    assert_eq!(marker[1].event["messageId"], format!("fork-{}", cut + 1));
    assert_eq!(marker[1].event["content"]["from"]["threadId"], parent);
    assert_eq!(marker[1].event["content"]["from"]["seq"], cut);
    assert_eq!(marker[1].event["content"]["kind"], "fork");
    let thread = &marker[2].event["snapshot"]["thread"];
    assert_eq!(thread["state"], "done");
    assert_eq!(
        thread["forkedFrom"],
        json!({"threadId": parent, "seq": cut, "kind": "fork"})
    );
    assert_eq!(marker[3].id, Some(cut + 1));

    // The screen holds what it was shown: every message and activity of the replay. A run with
    // all of them and one more is accepted, and starts the fork's next job.
    let mut held: Vec<Value> = Vec::new();
    for frame in &ours {
        let e = &frame.event;
        match e["type"].as_str().unwrap() {
            "TEXT_MESSAGE_START" => held.push(json!({
                "id": e["messageId"], "role": e["role"], "content": "text",
            })),
            "ACTIVITY_SNAPSHOT" => held.push(json!({
                "id": e["messageId"], "role": "activity",
                "activityType": e["activityType"], "content": e["content"],
            })),
            _ => {}
        }
    }
    assert!(held.len() >= 3, "{held:?}");
    let mut body = input(&fork, "run-2", &[("msg-new", "echo more")], json!({}));
    let messages = body["messages"].as_array_mut().unwrap();
    for (i, message) in held.into_iter().enumerate() {
        messages.insert(i, message);
    }
    let frames = conforming(
        chat.agui_run("plain", &body)
            .await
            .collect_frames(WAIT)
            .await,
    );
    assert_eq!(
        frames.last().unwrap().event["outcome"],
        json!({"type": "success"}),
        "{:?}",
        frames.iter().map(kind).collect::<Vec<_>>()
    );
    let first = frames.iter().find(|f| kind(f) == "STATE_SNAPSHOT").unwrap();
    assert_eq!(first.event["snapshot"]["thread"]["jobNumber"], 2);
    assert_eq!(
        first.event["snapshot"]["thread"]["forkedFrom"]["kind"],
        "fork"
    );
}

backends!(a_fork_replays_the_parents_frames_then_the_marker_and_takes_a_run_with_the_histories_ids);

/// What a viewer reads for each scripted behaviour, in order.
async fn viewer_frames(world: &World, name: &str, thread: &str) -> Vec<Vec<Frame>> {
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let run = |body: Value| {
        let chat = chat.clone();
        async move {
            conforming(
                chat.agui_run("plain", &body)
                    .await
                    .collect_frames(WAIT)
                    .await,
            )
        }
    };
    match name {
        "echo" => {
            run(input(thread, "run-1", &[("msg-1", "echo hi")], json!({}))).await;
            chat.wait_state(thread, "done").await;
        }
        "ask" => {
            let first = run(input(
                thread,
                "run-1",
                &[("msg-1", "ask about branches")],
                json!({}),
            ))
            .await;
            let interrupt = first.last().unwrap().event["outcome"]["interrupts"][0]["id"].clone();
            run(input(
                thread,
                "run-2",
                &[("msg-1", "ask about branches")],
                json!({"resume": [{
                    "interruptId": interrupt, "status": "resolved", "payload": {"text": "main"}
                }]}),
            ))
            .await;
            chat.wait_state(thread, "done").await;
        }
        // Nested steps (ADR 0025): a sub-agent with a command that fails; and a step that is
        // waiting when the agent asks, ended in the next run.
        "steps" => {
            run(input(
                thread,
                "run-1",
                &[("msg-1", "steps run the tests")],
                json!({}),
            ))
            .await;
            chat.wait_state(thread, "done").await;
        }
        "steps-ask" => {
            let first = run(input(
                thread,
                "run-1",
                &[("msg-1", "steps-ask clean the build")],
                json!({}),
            ))
            .await;
            let interrupt = first.last().unwrap().event["outcome"]["interrupts"][0]["id"].clone();
            run(input(
                thread,
                "run-2",
                &[("msg-1", "steps-ask clean the build")],
                json!({"resume": [{
                    "interruptId": interrupt, "status": "resolved", "payload": {"text": "yes"}
                }]}),
            ))
            .await;
            chat.wait_state(thread, "done").await;
        }
        // A person renames the thread after it is done (`patchThread`); a viewer that connects
        // afterwards reads the whole log, with the title the thread has now.
        "title" => {
            run(input(thread, "run-1", &[("msg-1", "echo hi")], json!({}))).await;
            chat.wait_state(thread, "done").await;
            let (status, renamed) = chat.rename(thread, "Fix the build").await;
            assert_eq!(status, 200, "{renamed}");
        }
        // A person writes the thread's description after it is done (`patchThread`, ADR 0035); a
        // viewer that connects afterwards reads the whole log, with the description the thread has
        // now in every snapshot.
        "description" => {
            run(input(thread, "run-1", &[("msg-1", "echo hi")], json!({}))).await;
            chat.wait_state(thread, "done").await;
            let (status, described) = chat.describe(thread, "Saying hi to the agent.").await;
            assert_eq!(status, 200, "{described}");
        }
        "cancel" => {
            let sse = chat
                .agui_run(
                    "plain",
                    &input(thread, "run-1", &[("msg-1", "slow work")], json!({})),
                )
                .await;
            chat.wait_state(thread, "working").await;
            assert_eq!(chat.cancel(thread).await, 202);
            whole(sse).await;
            chat.wait_state(thread, "cancelled").await;
        }
        // Forking a thread (ADR 0029): what a viewer of the fork reads is the parent's frames up to
        // the cut, then the marker, then the fork's own life. `fork`: the second message of a
        // finished thread is edited into a branch, which starts job 2 at once.
        "fork" => {
            run(input(thread, "run-1", &[("msg-1", "echo one")], json!({}))).await;
            chat.wait_state(thread, "done").await;
            run(input(
                thread,
                "run-2",
                &[("msg-1", "echo one"), ("msg-2", "echo two")],
                json!({}),
            ))
            .await;
            chat.wait_state(thread, "done").await;
            let second = chat
                .events(thread)
                .await
                .into_iter()
                .filter(|e| e["kind"] == "user_message")
                .nth(1)
                .unwrap()["seq"]
                .clone();
            let fork = fork_of(thread);
            let (status, forked) = chat
                .post(
                    &format!("/api/threads/{thread}/fork"),
                    Some(json!({"replace": second, "text": "echo three", "messageId": "msg-3", "id": fork})),
                )
                .await;
            assert_eq!(status, 201, "{forked}");
            chat.wait_state(&fork, "done").await;
            return vec![whole(chat.agui_connect(&fork, None, true).await).await];
        }
        // A thread that waits for an answer is forked as it is, with its question, and the screen
        // goes on in the fork with the messages it holds: the run is accepted (no 422 for ids the
        // copy has), the question is not an interrupt of the fork, and the message starts job 2.
        "fork-blocked" => {
            run(input(
                thread,
                "run-1",
                &[("msg-1", "ask about branches")],
                json!({}),
            ))
            .await;
            chat.wait_state(thread, "blocked").await;
            let fork = fork_of(thread);
            let (status, forked) = chat
                .post(
                    &format!("/api/threads/{thread}/fork"),
                    Some(json!({"after": 1, "id": fork})),
                )
                .await;
            assert_eq!(status, 201, "{forked}");
            let frames = conforming(
                chat.agui_run(
                    "plain",
                    &input(
                        &fork,
                        "run-2",
                        &[("msg-1", "ask about branches"), ("msg-2", "echo thanks")],
                        json!({}),
                    ),
                )
                .await
                .collect_frames(WAIT)
                .await,
            );
            assert_eq!(
                frames.last().unwrap().event["outcome"],
                json!({"type": "success"})
            );
            chat.wait_state(&fork, "done").await;
            return vec![whole(chat.agui_connect(&fork, None, true).await).await];
        }
        "cursor" => {
            // A client that read up to the agent starting to work (log event 2), lost its
            // connection, and reconnects with that id while the run goes on.
            let _requester = chat
                .agui_run(
                    "plain",
                    &input(thread, "run-1", &[("msg-1", "gate hold")], json!({})),
                )
                .await;
            chat.wait_state(thread, "working").await;
            let mut first = chat.agui_connect(thread, None, false).await;
            until(&mut first, |f| f.id == Some(2)).await;
            drop(first);
            let mut resumed = chat.agui_connect(thread, Some(2), false).await;
            let mut frames = until(&mut resumed, |f| kind(f) == "STATE_SNAPSHOT").await;
            world.plain.release_gate();
            frames.extend(through_run(&mut resumed).await);
            return vec![frames];
        }
        // The verification gate (ADR 0018): what a viewer reads of a job that is sent back once
        // and then passes, and of one that runs out of attempts.
        "verify-green" | "verify-red" => {
            let (script, last) = if name == "verify-green" {
                ("verify-red-once", "done")
            } else {
                ("verify-red", "failed")
            };
            run(input(
                thread,
                "run-1",
                &[("msg-1", &format!("{script} fix the login"))],
                json!({"forwardedProps": {"vymalo.gate": {"require": ["agent-checks"]}}}),
            ))
            .await;
            chat.wait_state(thread, last).await;
        }
        // The verifier agent in the gate: `plain` requires it in its own entry.
        "verify-verifier-green" | "verify-verifier-red" => {
            run(input(
                thread,
                "run-1",
                &[("msg-1", "verify-reviewed fix the login")],
                json!({}),
            ))
            .await;
            let last = if name == "verify-verifier-green" {
                "done"
            } else {
                "failed"
            };
            chat.wait_state(thread, last).await;
        }
        // CI on the pushed commit (ADR 0017): what a viewer reads of a job that waits for CI, is
        // sent back by a red report and finishes on a green one.
        "ci" => {
            let node = world.node("ci-node").await;
            let inbox = node.spawn_inbox(fast_inbox(), "ci-node");
            let sse = chat
                .agui_run(
                    "plain",
                    &input(
                        thread,
                        "run-1",
                        &[("msg-1", "verify-ci fix the login")],
                        json!({"forwardedProps": {"vymalo.gate": {"require": ["ci"]}}}),
                    ),
                )
                .await;
            drive_ci(&chat, &node, thread).await;
            whole(sse).await;
            chat.wait_state(thread, "done").await;
            inbox.shutdown().await;
        }
        // The UI's catalog (ADR 0023): three jobs, carrying version 1, 2 and 1 again. A viewer
        // reads no frame of its own for a catalog, and every snapshot names the current one.
        "catalog" => {
            for (n, (text, version)) in [("echo hi", 1), ("echo again", 2), ("echo once more", 1)]
                .into_iter()
                .enumerate()
            {
                let n = n + 1;
                run(input(
                    thread,
                    &format!("run-{n}"),
                    &[(&format!("msg-{n}"), text)],
                    with_ui_catalog(version),
                ))
                .await;
                chat.wait_state(thread, "done").await;
            }
        }
        other => panic!("unknown scenario {other}"),
    }
    vec![whole(chat.agui_connect(thread, None, true).await).await]
}

/// The world a scenario runs in.
async fn world_for(name: &str) -> World {
    match name {
        "verify-verifier-green" => {
            World::with(
                Backend::Memory,
                verified_by_reviewer(VerifierScript::FindingsThenPass),
            )
            .await
        }
        "verify-verifier-red" => {
            World::with(
                Backend::Memory,
                verified_by_reviewer(VerifierScript::AlwaysFail),
            )
            .await
        }
        "steps" | "steps-ask" => world_with_steps().await,
        _ => World::start(Backend::Memory).await,
    }
}

#[tokio::test]
async fn connect_streams_match_docs_api_examples() {
    let dir = examples_dir();
    let mut stale = Vec::new();
    for (n, name) in [
        "echo",
        "ask",
        "cancel",
        "cursor",
        "verify-green",
        "verify-red",
        "verify-verifier-green",
        "verify-verifier-red",
        "ci",
        "catalog",
        "steps",
        "steps-ask",
        "title",
        "description",
        "fork",
        "fork-blocked",
    ]
    .into_iter()
    .enumerate()
    {
        let world = world_for(name).await;
        let thread = thread_id(100 + u32::try_from(n).unwrap());
        let frames = viewer_frames(&world, name, &thread).await;
        // The viewer of a fork reads the fork, which has an id of its own.
        let text = if name.starts_with("fork") {
            render_fork(&frames, &thread)
        } else {
            render(&frames, &thread)
        };
        stale.extend(check_golden(
            &dir.join(format!("connect-{name}.agui.json")),
            &text,
        ));
    }
    assert!(
        stale.is_empty(),
        "connect goldens are out of date; run `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test agui_connect` and review the diff:\n{}",
        stale.join("\n")
    );
}

#[tokio::test]
async fn capabilities_match_docs_api_examples() {
    let world = World::start(Backend::Memory).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let mut stale = Vec::new();
    for agent in ["plain", "coder"] {
        let (status, doc) = chat.agui_capabilities(agent).await;
        assert_eq!(status, 200);
        assert_capabilities_json_conforms(&doc);
        stale.extend(check_golden(
            &examples_dir().join(format!("capabilities-{agent}.json")),
            &pretty(&doc),
        ));
    }
    assert!(
        stale.is_empty(),
        "capabilities goldens are out of date; run `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test agui_connect` and review the diff:\n{}",
        stale.join("\n")
    );
}
