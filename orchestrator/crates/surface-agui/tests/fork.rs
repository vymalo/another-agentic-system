//! A run that creates its thread as a fork (ADR 0042, decisions 8 and 9):
//! `forwardedProps["vymalo.fork"] = {from, after}` makes the fork and its first message in one
//! transaction, streams from the run the message opens, replays for the same request again, and is
//! refused before anything is written when it cannot be done. The real router on a real port over
//! the in-memory stack.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_ports::memory::Call;
use serde_json::{Value, json};
use support::*;

/// The body of the run that makes `thread` a fork of `from` after the turn holding `after`.
fn fork_run(thread: &str, run: &str, text: &str, from: &str, after: i64) -> Value {
    input_with(
        thread,
        run,
        &[(&format!("m-{run}"), text)],
        json!({"forwardedProps": {"vymalo.fork": {"from": from, "after": after}}}),
    )
}

fn kinds_of(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["kind"].as_str().unwrap()).collect()
}

/// A finished thread of one turn, `echo one`, and its id.
async fn parent(h: &Harness) -> String {
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-p", &[("m-p", "echo one")]),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    h.wait_state(ALICE, &thread, "done").await;
    thread
}

async fn exists(h: &Harness, user: &str, thread: &str) -> bool {
    h.get(&format!("/api/threads/{thread}"), Some(user))
        .await
        .status
        == 200
}

#[tokio::test]
async fn a_run_makes_the_fork_with_its_first_message_and_streams_from_its_own_run() {
    let h = Harness::start().await;
    let from = parent(&h).await;
    let fork = new_thread_id();
    assert!(
        !exists(&h, ALICE, &fork).await,
        "nothing exists before the send"
    );

    let frames = h
        .run(
            "plain",
            ALICE,
            &fork_run(&fork, "run-f", "echo two", &from, 1),
        )
        .await
        .all()
        .await;
    // the response is the run the message opened: no frame of the turns it copied
    let kinds = kinds(&frames);
    assert_eq!(kinds[0], "RUN_STARTED", "{kinds:?}");
    assert_eq!(frames[0].event["runId"], "run-f");
    assert_eq!(kinds.iter().filter(|k| **k == "RUN_STARTED").count(), 1);
    assert_eq!(*kinds.last().unwrap(), "RUN_FINISHED");
    assert!(
        !frames
            .iter()
            .any(|f| f.event.to_string().contains("echo: echo one")),
        "the parent's answer was copied, not sent again"
    );
    assert!(
        frames
            .iter()
            .any(|f| f.event.to_string().contains("echo: echo two")),
        "the fork's own answer"
    );

    // the fork: the copy of the first turn, `thread_forked`, then its own message and job
    let events = h.events(ALICE, &fork).await;
    let kinds_log = kinds_of(&events);
    let forked_at = kinds_log
        .iter()
        .position(|k| *k == "thread_forked")
        .unwrap();
    assert_eq!(
        kinds_log[forked_at..forked_at + 3],
        ["thread_forked", "user_message", "job_started"]
    );
    let message = &events[forked_at + 1];
    assert_eq!(message["data"]["text"], "echo two");
    assert_eq!(message["data"]["messageId"], "m-run-f");
    assert_eq!(message["data"]["runId"], "run-f");
    let record = h.thread(ALICE, &fork).await;
    assert_eq!(record["forkedFrom"]["threadId"], from);
    assert_eq!(record["forkedFrom"]["kind"], "fork");
    assert_eq!(record["state"], "done");

    // its agent was told the conversation it continues, the parent's copy of it
    let sends = h.agent.sends();
    let told = sends
        .iter()
        .find_map(|c| match c {
            Call::Send {
                context_id,
                history,
                text,
                ..
            } if *context_id == fork => Some((history.clone(), text.clone())),
            _ => None,
        })
        .expect("a send in the fork's context");
    assert_eq!(told.1, "echo two");
    let history = told.0.expect("the conversation");
    assert_eq!(history.entries.len(), 1);
    assert_eq!(history.entries[0].text, "echo one");
}

#[tokio::test]
async fn the_mentions_and_the_catalog_of_the_run_apply_to_the_first_message() {
    let h = Harness::start().await;
    let from = parent(&h).await;
    let fork = new_thread_id();
    let catalog = json!({
        "catalogId": "https://agents.vymalo.com/a2ui/catalogs/chat",
        "components": {"Note": {
            "type": "object",
            "properties": {"component": {"const": "Note"}},
        }},
    });
    let body = input_with(
        &fork,
        "run-f",
        &[("m-f", "@coder echo two")],
        json!({"forwardedProps": {
            "vymalo.fork": {"from": from, "after": 1},
            "vymalo.mentions": [{"agentId": "coder", "label": "@coder", "start": 0, "end": 6}],
            "vymalo.uiCatalog": {
                "catalogId": "https://agents.vymalo.com/a2ui/catalogs/chat",
                "version": 1,
                "digest": orch_core::catalog_digest(&catalog).unwrap(),
                "catalog": catalog,
            },
        }}),
    );
    let frames = h.run("plain", ALICE, &body).await.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    let events = h.events(ALICE, &fork).await;
    let forked_at = events
        .iter()
        .position(|e| e["kind"] == "thread_forked")
        .unwrap();
    let own = &events[forked_at..];
    assert_eq!(
        kinds_of(own)[..4],
        ["thread_forked", "ui_catalog", "user_message", "job_started"],
        "the catalog is recorded after the fork, for the first message"
    );
    assert_eq!(own[2]["data"]["mentions"][0]["agentId"], "coder");
}

#[tokio::test]
async fn the_resend_of_the_request_attaches_to_the_fork_and_writes_no_second_message() {
    let h = Harness::start().await;
    let from = parent(&h).await;
    let fork = new_thread_id();
    let body = fork_run(&fork, "run-f", "echo two", &from, 1);
    let first = h.run("plain", ALICE, &body).await.all().await;
    assert_eq!(first.last().unwrap().kind(), "RUN_FINISHED");
    let log = h.events(ALICE, &fork).await;
    let sends = h.agent.sends().len();

    // the response was lost: the same request again, the parent has gone on since
    h.run(
        "plain",
        ALICE,
        &input(
            &from,
            "run-p2",
            &[("m-p", "echo one"), ("m-p2", "echo more")],
        ),
    )
    .await
    .all()
    .await;
    let again = h.run("plain", ALICE, &body).await.all().await;
    let kinds = kinds(&again);
    assert_eq!(kinds[0], "RUN_STARTED", "{kinds:?}");
    assert_eq!(again[0].event["runId"], "run-f");
    assert_eq!(*kinds.last().unwrap(), "RUN_FINISHED");
    assert_eq!(h.events(ALICE, &fork).await, log, "no second message");
    assert_eq!(
        h.agent.sends().len(),
        sends + 1,
        "only the parent's own send"
    );
}

#[tokio::test]
async fn another_thread_with_the_id_is_a_409_and_somebody_elses_is_a_404() {
    let h = Harness::start().await;
    let from = parent(&h).await;
    let fork = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &fork_run(&fork, "run-f", "echo two", &from, 1),
    )
    .await
    .all()
    .await;
    let other = parent(&h).await;
    let before = h.events(ALICE, &other).await;

    for (why, body) in [
        (
            "the parent itself",
            fork_run(&from, "run-x", "echo x", &from, 1),
        ),
        (
            "a thread that is no fork",
            fork_run(&other, "run-x", "echo x", &from, 1),
        ),
        (
            "another message",
            fork_run(&fork, "run-x", "echo x", &from, 1),
        ),
        (
            "another cut",
            fork_run(&fork, "run-f", "echo two", &from, 9),
        ),
        (
            "another parent",
            fork_run(&fork, "run-f", "echo two", &other, 1),
        ),
    ] {
        let resp = h.refused("plain", Some(ALICE), &body).await;
        assert_eq!(
            resp.status,
            409,
            "{why}: {}",
            String::from_utf8_lossy(&resp.body)
        );
    }
    assert_eq!(h.events(ALICE, &other).await, before);

    // the id of a thread that is somebody else's looks like any other refusal
    let bobs = new_thread_id();
    h.run("plain", BOB, &input(&bobs, "run-b", &[("m-b", "echo hi")]))
        .await
        .all()
        .await;
    h.refused(
        "plain",
        Some(ALICE),
        &fork_run(&bobs, "run-x", "echo x", &from, 1),
    )
    .await
    .problem(404);
}

#[tokio::test]
async fn a_parent_that_is_not_the_callers_is_a_404_and_no_thread_is_made() {
    let h = Harness::start().await;
    let from = parent(&h).await;
    let fork = new_thread_id();
    h.refused(
        "plain",
        Some(BOB),
        &fork_run(&fork, "run-f", "echo x", &from, 1),
    )
    .await
    .problem(404);
    let nothing = new_thread_id();
    h.refused(
        "plain",
        Some(ALICE),
        &fork_run(&fork, "run-f", "echo x", &nothing, 1),
    )
    .await
    .problem(404);
    assert!(!exists(&h, ALICE, &fork).await);
    assert!(!exists(&h, BOB, &fork).await);
}

#[tokio::test]
async fn a_turn_that_is_going_on_is_a_409_turn_open_and_a_cut_outside_the_log_a_422() {
    let h = Harness::start().await;
    let busy = new_thread_id();
    let mut stream = h
        .run(
            "plain",
            ALICE,
            &input(&busy, "run-s", &[("m-s", "slow work")]),
        )
        .await;
    assert_eq!(stream.next(T).await.unwrap().kind(), "RUN_STARTED");
    h.wait_state(ALICE, &busy, "working").await;
    let fork = new_thread_id();
    let problem = h
        .refused(
            "plain",
            Some(ALICE),
            &fork_run(&fork, "run-f", "echo x", &busy, 1),
        )
        .await
        .problem(409);
    assert_eq!(problem["code"], "turn_open");
    assert!(!exists(&h, ALICE, &fork).await);
    h.post_empty(&format!("/api/threads/{busy}/cancel"), ALICE)
        .await;

    let from = parent(&h).await;
    for after in [0, -1, 99] {
        h.refused(
            "plain",
            Some(ALICE),
            &fork_run(&fork, "run-f", "echo x", &from, after),
        )
        .await
        .problem(422);
    }
    assert!(!exists(&h, ALICE, &fork).await);
}

#[tokio::test]
async fn a_member_that_is_malformed_or_comes_with_a_gate_or_tools_is_a_400_and_writes_nothing() {
    let h = Harness::start().await;
    let from = parent(&h).await;
    let fork = new_thread_id();
    let run = |props: Value| {
        input_with(
            &fork,
            "run-f",
            &[("m-f", "echo x")],
            json!({"forwardedProps": props}),
        )
    };
    for (why, props) in [
        ("not an object", json!({"vymalo.fork": "x"})),
        ("an array", json!({"vymalo.fork": [from, 1]})),
        ("no from", json!({"vymalo.fork": {"after": 1}})),
        ("no after", json!({"vymalo.fork": {"from": from}})),
        (
            "from is no uuid",
            json!({"vymalo.fork": {"from": "nope", "after": 1}}),
        ),
        (
            "after is no integer",
            json!({"vymalo.fork": {"from": from, "after": "1"}}),
        ),
        (
            "after is a fraction",
            json!({"vymalo.fork": {"from": from, "after": 1.5}}),
        ),
        (
            "a member it does not know",
            json!({"vymalo.fork": {"from": from, "after": 1, "kind": "edit"}}),
        ),
        (
            "a gate",
            json!({"vymalo.fork": {"from": from, "after": 1}, "vymalo.gate": {"maxAttempts": 2}}),
        ),
        (
            "tools",
            json!({"vymalo.fork": {"from": from, "after": 1}, "vymalo.tools": ["websearch"]}),
        ),
    ] {
        let resp = h.refused("plain", Some(ALICE), &run(props)).await;
        assert_eq!(
            resp.status,
            400,
            "{why}: {}",
            String::from_utf8_lossy(&resp.body)
        );
    }
    assert!(!exists(&h, ALICE, &fork).await);
    // null and no tools are not "together with"
    let frames = h
        .run(
            "plain",
            ALICE,
            &run(json!({"vymalo.fork": {"from": from, "after": 1}, "vymalo.tools": [], "vymalo.gate": null})),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
}

#[tokio::test]
async fn a_null_member_creates_an_ordinary_thread() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &input_with(
                &thread,
                "run-n",
                &[("m-n", "echo plain")],
                json!({"forwardedProps": {"vymalo.fork": null}}),
            ),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    let record = h.thread(ALICE, &thread).await;
    assert!(record.get("forkedFrom").is_none(), "{record}");
}
