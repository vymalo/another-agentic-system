//! `wait_for_job` over MCP: an in-process rmcp client that receives the progress notifications,
//! the real router, the real dispatcher and the in-memory stack.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_core::EventKind;
use orch_ports::ThreadStore;
use orch_testsupport::eventually;
use serde_json::{Value, json};
use support::*;

async fn log_seqs(h: &Harness, job: &str) -> Vec<i64> {
    h.store
        .list_events(thread(job), 0, 100)
        .await
        .unwrap()
        .iter()
        .map(|e| e.seq)
        .collect()
}

fn strictly_increasing_integers(progress: &Progress) {
    let counters: Vec<f64> = progress.all().iter().map(|(n, _)| *n).collect();
    assert!(!counters.is_empty());
    for (i, n) in counters.iter().enumerate() {
        assert_eq!(n.fract(), 0.0, "an integer counter: {counters:?}");
        assert_eq!(*n, (i + 1) as f64, "1, 2, 3, ...: {counters:?}");
    }
}

#[tokio::test]
async fn progress_notifications_count_up_and_the_result_is_the_finished_job() {
    let h = Harness::start().await;
    let (client, progress) = connect_recording(&h.mcp_url, ALICE_TOKEN).await;
    let job = start(&client, "gate the wait").await;

    // The call starts from the beginning of the log; the agent is held until it has reported
    // that the agent works.
    let waiting = {
        let peer = client.peer().clone();
        let job = job.clone();
        tokio::spawn(async move {
            call_peer(
                &peer,
                "wait_for_job",
                json!({"job_id": job, "after_seq": 0, "timeout_secs": 60}),
            )
            .await
        })
    };
    eventually("the wait has reported the agent working", || async {
        progress
            .all()
            .iter()
            .any(|(_, m)| m.ends_with("agent working"))
            .then_some(())
    })
    .await;
    assert!(!waiting.is_finished(), "the job is held");
    h.agent.release_gate();

    let out = waiting.await.unwrap();
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.value["outcome"], "finished");
    assert_eq!(out.value["state"], "done");
    assert_eq!(out.value["finished"], true);
    let seqs = log_seqs(&h, &job).await;
    assert_eq!(out.value["resume_after_seq"], *seqs.last().unwrap());
    assert_eq!(out.value["last_seq"], *seqs.last().unwrap());
    // One notification per event, in order, with a counter that only increases.
    strictly_increasing_integers(&progress);
    assert_eq!(progress.seqs(), seqs);
    assert_eq!(progress.heartbeats(), 0);
}

#[tokio::test]
async fn a_timeout_returns_where_to_resume_and_a_second_call_loses_nothing() {
    let h = Harness::start().await;
    let (client, first) = connect_recording(&h.mcp_url, ALICE_TOKEN).await;
    let job = start(&client, "gate resume").await;
    wait_state(&client, &job, "working").await;

    // Zero seconds: report what the log holds, then say where to resume.
    let out = call(
        &client,
        "wait_for_job",
        json!({"job_id": job, "after_seq": 0, "timeout_secs": 0}),
    )
    .await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.value["outcome"], "timed_out");
    assert_eq!(out.value["state"], "working");
    let resume = out.value["resume_after_seq"].as_i64().unwrap();
    assert_eq!(first.seqs(), (1..=resume).collect::<Vec<_>>());

    // The job goes on while nobody waits.
    h.agent.release_gate();
    wait_state(&client, &job, "done").await;

    let (again, second) = connect_recording(&h.mcp_url, ALICE_TOKEN).await;
    let out = call(
        &again,
        "wait_for_job",
        json!({"job_id": job, "after_seq": resume, "timeout_secs": 5}),
    )
    .await;
    assert_eq!(out.value["outcome"], "finished", "{out:?}");
    let mut seen = first.seqs();
    seen.extend(second.seqs());
    assert_eq!(
        seen,
        log_seqs(&h, &job).await,
        "every event once, none missed"
    );
}

#[tokio::test]
async fn a_finished_job_returns_at_once_and_omitting_after_seq_reports_only_what_comes_next() {
    let h = Harness::start().await;
    let (client, progress) = connect_recording(&h.mcp_url, ALICE_TOKEN).await;
    let job = start(&client, "echo quick").await;
    wait_state(&client, &job, "done").await;
    let out = call(
        &client,
        "wait_for_job",
        json!({"job_id": job, "timeout_secs": 3600}),
    )
    .await;
    assert_eq!(out.value["outcome"], "finished");
    assert!(progress.all().is_empty(), "there is nothing after the end");
}

#[tokio::test]
async fn a_blocked_job_returns_blocked_and_the_answer_lets_the_next_wait_finish() {
    let h = Harness::start().await;
    let (client, progress) = connect_recording(&h.mcp_url, ALICE_TOKEN).await;
    let job = start(&client, "ask which branch").await;
    let out = call(
        &client,
        "wait_for_job",
        json!({"job_id": job, "after_seq": 0, "timeout_secs": 30}),
    )
    .await;
    assert_eq!(out.value["outcome"], "blocked", "{out:?}");
    assert_eq!(out.value["state"], "blocked");
    let resume = out.value["resume_after_seq"].as_i64().unwrap();
    assert!(
        progress
            .all()
            .iter()
            .any(|(_, m)| m.contains("input_required"))
    );

    let answered = call(&client, "answer", json!({"job_id": job, "text": "main"})).await;
    assert!(!answered.is_error, "{answered:?}");
    let out = call(
        &client,
        "wait_for_job",
        json!({"job_id": job, "after_seq": resume, "timeout_secs": 30}),
    )
    .await;
    assert_eq!(out.value["outcome"], "finished", "{out:?}");
    assert_eq!(out.value["state"], "done");
    assert_eq!(
        progress.seqs(),
        log_seqs(&h, &job).await,
        "both calls together reported the whole log once"
    );
}

#[tokio::test]
async fn a_heartbeat_keeps_going_while_nothing_happens() {
    let h = Harness::start_with(Options {
        heartbeat: Some(Duration::from_millis(20)),
        ..Options::default()
    })
    .await;
    let (client, progress) = connect_recording(&h.mcp_url, ALICE_TOKEN).await;
    let job = start(&client, "gate slowly").await;
    wait_state(&client, &job, "working").await;
    let last = log_seqs(&h, &job).await.last().copied().unwrap();

    let waiting = {
        let peer = client.peer().clone();
        let job = job.clone();
        tokio::spawn(async move {
            call_peer(
                &peer,
                "wait_for_job",
                json!({"job_id": job, "after_seq": last, "timeout_secs": 60}),
            )
            .await
        })
    };
    eventually(
        "three heartbeats arrived while the agent was held",
        || async { (progress.heartbeats() >= 3).then_some(()) },
    )
    .await;
    h.agent.release_gate();
    let out = waiting.await.unwrap();
    assert_eq!(out.value["outcome"], "finished", "{out:?}");
    strictly_increasing_integers(&progress);
    // Heartbeats and events share the one counter.
    assert!(progress.all().len() > progress.heartbeats());
}

#[tokio::test]
async fn timeout_secs_is_cut_to_the_configured_bound() {
    let h = Harness::start_with(Options {
        wait_max: Some(Duration::from_millis(50)),
        ..Options::default()
    })
    .await;
    let client = h.client(ALICE_TOKEN).await;
    let job = start(&client, "gate forever").await;
    wait_state(&client, &job, "working").await;
    // An hour asked, a twentieth of a second allowed.
    let out = call(
        &client,
        "wait_for_job",
        json!({"job_id": job, "timeout_secs": 3600}),
    )
    .await;
    assert_eq!(out.value["outcome"], "timed_out", "{out:?}");
    h.agent.release_gate();
}

#[tokio::test]
async fn refusals_are_tool_errors_and_a_foreign_job_is_not_found() {
    let h = Harness::start().await;
    let alice = h.client(ALICE_TOKEN).await;
    let bob = h.client(BOB_TOKEN).await;
    let job = start(&alice, "gate private").await;
    wait_state(&alice, &job, "working").await;

    let theirs = call(
        &bob,
        "wait_for_job",
        json!({"job_id": job, "timeout_secs": 1}),
    )
    .await;
    assert!(theirs.is_error);
    assert_eq!(theirs.text, "no such job");
    let unknown = call(
        &bob,
        "wait_for_job",
        json!({"job_id": "00000000-0000-7000-8000-00000000ffff", "timeout_secs": 1}),
    )
    .await;
    assert_eq!(theirs.text, unknown.text);
    let negative = call(
        &alice,
        "wait_for_job",
        json!({"job_id": job, "after_seq": -1, "timeout_secs": 1}),
    )
    .await;
    assert!(negative.is_error);
    // The timeout is required; a bad type is a protocol error.
    for args in [
        json!({"job_id": job}),
        json!({"job_id": job, "timeout_secs": "soon"}),
    ] {
        let err = try_call(&alice, "wait_for_job", args).await.unwrap_err();
        assert!(err.to_string().contains("invalid arguments"), "{err}");
    }
    // Nothing was written by any of them.
    let kinds: Vec<EventKind> = h
        .store
        .list_events(thread(&job), 0, 100)
        .await
        .unwrap()
        .iter()
        .map(orch_core::Event::kind)
        .collect();
    assert!(kinds.iter().all(|k| *k != EventKind::Error), "{kinds:?}");
    h.agent.release_gate();
    let _: Value = json!(null);
}
