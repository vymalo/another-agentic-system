//! End-to-end flows over HTTP against the in-memory stack and the scripted agent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_ports::memory::Call;
use serde_json::json;
use support::*;

const FIVE: [&str; 5] = [
    "user_message",
    "agent_status:working",
    "artifact",
    "agent_status:completed",
    "thread_state:done",
];

#[tokio::test]
async fn creating_a_thread_streams_the_expected_sequence_and_events_agree() {
    let h = Harness::start().await;
    let id = h.create(ALICE, "plain", "echo hi").await;
    let mut sse = h.stream(ALICE, &id, None).await;
    let mut streamed = Vec::new();
    loop {
        let (_, _, data) = sse.next_event(Duration::from_secs(5)).await.expect("event");
        let done = data["kind"] == "thread_state" && data["data"]["state"] == "done";
        streamed.push(data);
        if done {
            break;
        }
    }
    assert_eq!(shape(&streamed), FIVE);
    let listed = h.events(ALICE, &id).await;
    assert_eq!(listed, streamed);
    let seqs: Vec<i64> = listed.iter().map(|e| e["seq"].as_i64().unwrap()).collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
    assert_eq!(
        listed[2]["data"]["uri"],
        "https://github.com/acme/demo/pull/1"
    );
    let thread = h
        .get(&format!("/api/threads/{id}"), Some(ALICE))
        .await
        .json();
    assert_eq!(thread["state"], "done");
    assert_eq!(thread["lastSeq"], 5);
}

#[tokio::test]
async fn an_input_required_agent_blocks_the_thread_and_a_follow_up_resumes_the_task() {
    let h = Harness::start().await;
    let id = h.create(ALICE, "plain", "ask which branch").await;
    h.wait_state(ALICE, &id, "blocked").await;
    let ev = h.events(ALICE, &id).await;
    assert_eq!(
        shape(&ev),
        [
            "user_message",
            "agent_status:working",
            "agent_status:input_required",
            "thread_state:blocked"
        ]
    );
    assert_eq!(ev[2]["data"]["detail"], "Which branch?");

    let r = h
        .post(
            &format!("/api/threads/{id}/messages"),
            Some(ALICE),
            json!({"text": "main"}),
        )
        .await;
    assert_eq!(r.status, 202);
    assert_eq!(r.json()["seq"], 5);
    h.wait_state(ALICE, &id, "done").await;

    let sends = h.agent.sends();
    assert_eq!(sends.len(), 2);
    match (&sends[0], &sends[1]) {
        (
            Call::Send { task_id: first, .. },
            Call::Send {
                task_id: second,
                text,
                ..
            },
        ) => {
            assert_eq!(first, &None);
            assert_eq!(
                second.as_deref(),
                Some("task-1"),
                "the same A2A task is resumed"
            );
            assert_eq!(text, "main");
        }
        other => panic!("{other:?}"),
    }
    let ev = h.events(ALICE, &id).await;
    assert_eq!(ev.last().unwrap()["data"]["state"], "done");
}

#[tokio::test]
async fn cancel_stops_a_running_thread_and_the_agent_sees_it() {
    let h = Harness::start().await;
    let id = h.create(ALICE, "plain", "slow work").await;
    h.wait_state(ALICE, &id, "working").await;
    let r = h
        .post_empty(&format!("/api/threads/{id}/cancel"), Some(ALICE))
        .await;
    assert_eq!(r.status, 202);
    h.wait_state(ALICE, &id, "cancelled").await;
    assert_eq!(
        shape(&h.events(ALICE, &id).await),
        [
            "user_message",
            "agent_status:working",
            "agent_status:canceled",
            "thread_state:cancelled"
        ]
    );
    assert!(h.agent.calls().contains(&Call::Cancel {
        task_id: "task-1".into()
    }));
    // A finished thread rejects messages (409) but cancel stays a quiet 202.
    let r = h
        .post(
            &format!("/api/threads/{id}/messages"),
            Some(ALICE),
            json!({"text": "more"}),
        )
        .await;
    assert_eq!(r.status, 409);
    let before = h.events(ALICE, &id).await.len();
    assert_eq!(
        h.post_empty(&format!("/api/threads/{id}/cancel"), Some(ALICE))
            .await
            .status,
        202
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.events(ALICE, &id).await.len(), before);
}

#[tokio::test]
async fn a_release_is_offered_validated_sent_and_echoed() {
    let h = Harness::start().await;
    let agents = h.get("/api/agents", Some(ALICE)).await.json();
    let coder = agents
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "coder")
        .unwrap();
    assert_eq!(coder["releases"]["defaultChannel"], "stable");

    let r = h
        .post(
            "/api/threads",
            Some(ALICE),
            json!({"target": {"agentId": "coder", "release": "staging"}, "text": "echo release"}),
        )
        .await;
    assert_eq!(r.status, 201);
    let id = r.json()["id"].as_str().unwrap().to_owned();
    h.wait_state(ALICE, &id, "done").await;
    match &h.agent.sends()[0] {
        Call::Send { release, .. } => assert_eq!(release.as_deref(), Some("staging")),
        other => panic!("{other:?}"),
    }
    assert_eq!(h.events(ALICE, &id).await[1]["actor"]["revision"], "rev-2");

    for bad in [
        json!({"target": {"agentId": "coder", "release": "nightly"}, "text": "x"}),
        json!({"target": {"agentId": "plain", "release": "stable"}, "text": "x"}),
    ] {
        assert_eq!(h.post("/api/threads", Some(ALICE), bad).await.status, 400);
    }

    // ADR 0008: the picker is read live. A card that goes dark hides the releases and blocks
    // creating a thread with one.
    h.agent.set_card_down("coder", true);
    let agents = h.get("/api/agents", Some(ALICE)).await.json();
    let coder = agents
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "coder")
        .unwrap();
    assert!(coder.get("releases").is_none());
    let r = h
        .post(
            "/api/threads",
            Some(ALICE),
            json!({"target": {"agentId": "coder", "release": "stable"}, "text": "x"}),
        )
        .await;
    // The agent's failure, not the caller's mistake: 502 (it used to be a 400).
    assert_eq!(r.status, 502);
    assert_eq!(r.json()["detail"], "agent coder is unavailable");
}

#[tokio::test]
async fn delivery_failures_show_up_in_the_chat() {
    let h = Harness::start().await;
    let id = h.create(ALICE, "plain", "fail immediately").await;
    h.wait_state(ALICE, &id, "failed").await;
    let ev = h.events(ALICE, &id).await;
    assert_eq!(shape(&ev), ["user_message", "error", "thread_state:failed"]);
    assert_eq!(ev[1]["data"]["retryable"], false);
    assert_eq!(
        ev[1]["actor"],
        json!({"type": "system", "name": "orchestrator"})
    );
    assert!(
        ev[1]["data"]["message"]
            .as_str()
            .unwrap()
            .contains("scripted failure")
    );
}

#[tokio::test]
async fn concurrent_threads_of_two_users_do_not_interfere() {
    let h = Harness::start().await;
    let mut ids = Vec::new();
    for i in 0..4 {
        ids.push((ALICE, h.create(ALICE, "plain", &format!("echo a{i}")).await));
        ids.push((BOB, h.create(BOB, "plain", &format!("echo b{i}")).await));
    }
    for (user, id) in &ids {
        h.wait_state(user, id, "done").await;
        assert_eq!(shape(&h.events(user, id).await), FIVE);
    }
    assert_eq!(h.agent.sends().len(), 8);
}

#[tokio::test]
async fn failures_of_others_are_502_and_503_with_a_retry_after_and_no_internals() {
    let h = Harness::start().await;
    let post = |body: serde_json::Value| {
        h.client
            .post(h.url("/api/threads"))
            .header("X-Auth-Request-Email", ALICE)
            .json(&body)
            .send()
    };
    let release = json!({"target": {"agentId": "coder", "release": "staging"}, "text": "x"});

    // The agent's card is down: the agent's failure, not the caller's.
    h.agent.set_card_down("coder", true);
    let r = post(release.clone()).await.unwrap();
    assert_eq!(r.status().as_u16(), 502);
    assert!(r.headers().get("retry-after").is_none());
    assert_eq!(r.headers()["content-type"], "application/problem+json");
    h.agent.set_card_down("coder", false);

    // Storage is down: 503 with a Retry-After, and the cause stays in the log.
    h.store.fail_next_creates(1, || {
        orch_ports::StoreError::unavailable(std::io::Error::other(
            "postgres://user:hunter2@db.internal/orch",
        ))
    });
    let r = post(json!({"target": {"agentId": "plain"}, "text": "x"}))
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 503);
    assert_eq!(r.headers()["retry-after"], "5");
    let body = r.text().await.unwrap();
    assert!(
        !body.contains("hunter2") && !body.contains("db.internal"),
        "{body}"
    );
}
