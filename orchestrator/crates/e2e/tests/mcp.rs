//! The MCP server end to end: an rmcp client over streamable HTTP, the real router, the real
//! dispatcher, the A2A adapter and an in-process fake A2A agent, on the in-memory store and on
//! Postgres. Any replica serves any call (ADR 0019): the tests start a job on one and follow it
//! on another, and kill the replica that a `wait_for_job` runs on.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::sync::{Arc, Mutex};

use common::*;
use rmcp::model::{CallToolRequestParams, ProgressNotificationParam};
use rmcp::service::{NotificationContext, RunningService};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::{Value, json};

type Client = RunningService<RoleClient, Recorder>;

/// A client that keeps the progress notifications it receives, as messages.
#[derive(Clone, Default)]
struct Recorder {
    messages: Arc<Mutex<Vec<String>>>,
}

impl ClientHandler for Recorder {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.messages
            .lock()
            .unwrap()
            .push(params.message.unwrap_or_default());
    }
}

impl Recorder {
    /// The `seq` of each event notification (`#7 ...`), heartbeats left out.
    fn seqs(&self) -> Vec<i64> {
        self.messages
            .lock()
            .unwrap()
            .iter()
            .filter_map(|m| m.strip_prefix('#'))
            .filter_map(|m| m.split(' ').next()?.parse().ok())
            .collect()
    }

    fn says(&self, needle: &str) -> bool {
        self.messages
            .lock()
            .unwrap()
            .iter()
            .any(|m| m.contains(needle))
    }
}

async fn client(instance: &orch_testsupport::TestInstance, token: &str) -> Client {
    client_recording(instance, token).await.0
}

async fn client_recording(
    instance: &orch_testsupport::TestInstance,
    token: &str,
) -> (Client, Recorder) {
    let config =
        StreamableHttpClientTransportConfig::with_uri(format!("{}/mcp", instance.base_url))
            .auth_header(token);
    let recorder = Recorder::default();
    let client = recorder
        .clone()
        .serve(StreamableHttpClientTransport::from_config(config))
        .await
        .expect("the MCP handshake");
    (client, recorder)
}

/// Calls a tool through a peer that a spawned task can own.
async fn call_peer(
    peer: &rmcp::Peer<RoleClient>,
    tool: &'static str,
    args: Value,
) -> Result<(bool, Value), rmcp::ServiceError> {
    let Value::Object(args) = args else {
        panic!("arguments must be an object")
    };
    let result = peer
        .call_tool(CallToolRequestParams::new(tool).with_arguments(args))
        .await?;
    Ok((
        result.is_error == Some(true),
        result.structured_content.unwrap_or(Value::Null),
    ))
}

/// Calls a tool; returns `(is_error, structured result)`.
async fn call(client: &Client, tool: &'static str, args: Value) -> (bool, Value) {
    let Value::Object(args) = args else {
        panic!("arguments must be an object")
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool).with_arguments(args))
        .await
        .unwrap_or_else(|e| panic!("{tool}: {e}"));
    (
        result.is_error == Some(true),
        result.structured_content.unwrap_or(Value::Null),
    )
}

async fn state_of(client: &Client, job: &str) -> String {
    let (_, summary) = call(client, "get_job", json!({ "job_id": job })).await;
    summary["state"].as_str().unwrap().to_owned()
}

async fn wait_state(client: &Client, job: &str, state: &str) {
    eventually(&format!("job {job} is {state}"), || async {
        (state_of(client, job).await == state).then_some(())
    })
    .await;
}

/// A job started over MCP is delivered by the dispatcher, ends `done`, is in the user's thread
/// list of the web surface, and its first message says where it came from.
async fn a_job_runs_to_done_and_is_the_users_own_thread(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance_with_mcp("orch-1", true).await;
    let chat = world.chat(&orch);
    let alice = client(&orch, ALICE_TOKEN).await;

    let (is_error, started) = call(
        &alice,
        "start_job",
        json!({"text": "echo add a test", "agent": "plain", "title": "Add a test"}),
    )
    .await;
    assert!(!is_error, "{started}");
    let job = started["job_id"].as_str().unwrap().to_owned();
    wait_state(&alice, &job, "done").await;

    let events = chat.events(&job).await;
    assert_eq!(shape(&events), FIVE);
    assert_contiguous(&events);
    assert_eq!(events[0]["data"]["origin"], "mcp");
    assert_eq!(events[0]["actor"]["name"], ALICE);
    assert_eq!(world.plain.executions().len(), 1);

    // The chat sees it: same owner, same thread, through the identity route.
    let (status, listed) = chat.get("/api/threads").await;
    assert_eq!(status, 200);
    let listed: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(listed, [job.as_str()]);
    assert_eq!(chat.state(&job).await, "done");
    // And Bob does not: not found, on both surfaces.
    let bob = client(&orch, BOB_TOKEN).await;
    let (theirs, _) = call(&bob, "get_job", json!({ "job_id": job })).await;
    assert!(theirs);
    assert_eq!(
        chat.as_user(BOB)
            .get(&format!("/api/threads/{job}"))
            .await
            .0,
        404
    );
}

/// `answer` continues a blocked job, and a message to a finished one is refused.
async fn an_answer_unblocks_a_job(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance_with_mcp("orch-1", true).await;
    let alice = client(&orch, ALICE_TOKEN).await;
    let (_, started) = call(
        &alice,
        "start_job",
        json!({"text": "ask about the branch", "agent": "plain"}),
    )
    .await;
    let job = started["job_id"].as_str().unwrap().to_owned();
    wait_state(&alice, &job, "blocked").await;

    let (is_error, answered) = call(&alice, "answer", json!({"job_id": job, "text": "main"})).await;
    assert!(!is_error, "{answered}");
    wait_state(&alice, &job, "done").await;
    let events = world.chat(&orch).events(&job).await;
    assert_eq!(
        events
            .iter()
            .filter(|e| e["kind"] == "user_message")
            .map(|e| e["data"]["origin"].clone())
            .collect::<Vec<_>>(),
        [json!("mcp"), json!("mcp")]
    );
    let (is_error, _) = call(&alice, "answer", json!({"job_id": job, "text": "again"})).await;
    assert!(is_error);
}

/// `cancel_job` reaches the agent through the dispatcher.
async fn a_cancel_reaches_the_agent(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance_with_mcp("orch-1", true).await;
    let alice = client(&orch, ALICE_TOKEN).await;
    let (_, started) = call(
        &alice,
        "start_job",
        json!({"text": "slow", "agent": "plain"}),
    )
    .await;
    let job = started["job_id"].as_str().unwrap().to_owned();
    wait_state(&alice, &job, "working").await;
    let (is_error, _) = call(&alice, "cancel_job", json!({"job_id": job})).await;
    assert!(!is_error);
    wait_state(&alice, &job, "cancelled").await;
    // Cancelling again changes nothing.
    let (is_error, again) = call(&alice, "cancel_job", json!({"job_id": job})).await;
    assert!(!is_error);
    assert_eq!(again["state"], "cancelled");
}

/// A retried `start_job` finds the job whichever replica gets the retry.
async fn a_retry_on_another_replica_is_the_same_job(backend: Backend) {
    let world = World::start(backend).await;
    let one = world.instance_with_mcp("orch-1", true).await;
    let two = world.instance_with_mcp("orch-2", false).await;
    let on_one = client(&one, ALICE_TOKEN).await;
    let on_two = client(&two, ALICE_TOKEN).await;

    let args = json!({"text": "gate the retry", "agent": "plain", "client_request_id": "retry-1"});
    let (_, first) = call(&on_one, "start_job", args.clone()).await;
    let (_, second) = call(&on_two, "start_job", args).await;
    assert_eq!(first["job_id"], second["job_id"]);
    assert_eq!(
        (&first["created"], &second["created"]),
        (&json!(true), &json!(false))
    );
    let job = first["job_id"].as_str().unwrap();
    // The replica that runs no dispatcher sees the state the other one writes.
    wait_state(&on_two, job, "working").await;
    assert_eq!(world.plain.executions().len(), 1);
    world.plain.release_gate();
    wait_state(&on_two, job, "done").await;
}

/// The seqs of a job's whole log.
async fn log_seqs(world: &World, instance: &orch_testsupport::TestInstance, job: &str) -> Vec<i64> {
    world
        .chat(instance)
        .events(job)
        .await
        .iter()
        .map(|e| e["seq"].as_i64().unwrap())
        .collect()
}

/// A wait on a replica that delivers nothing follows a job that another replica delivers: the
/// progress comes from the shared log, and the result is the finished job.
async fn a_wait_follows_a_job_that_another_replica_delivers(backend: Backend) {
    let world = World::start(backend).await;
    let waiting_on = world.instance_with_mcp("orch-1", false).await;
    let delivering = world.instance_with_mcp("orch-2", true).await;
    let (client, progress) = client_recording(&waiting_on, ALICE_TOKEN).await;

    let (_, started) = call(
        &client,
        "start_job",
        json!({"text": "gate the wait", "agent": "plain"}),
    )
    .await;
    let job = started["job_id"].as_str().unwrap().to_owned();
    let peer = client.peer().clone();
    let waiting = tokio::spawn({
        let job = job.clone();
        async move {
            call_peer(
                &peer,
                "wait_for_job",
                json!({"job_id": job, "after_seq": 0, "timeout_secs": 60}),
            )
            .await
        }
    });
    eventually("the wait reported the agent working", || async {
        progress.says("agent working").then_some(())
    })
    .await;
    world.plain.release_gate();
    let (is_error, result) = waiting.await.unwrap().unwrap();
    assert!(!is_error, "{result}");
    assert_eq!(result["outcome"], "finished");
    assert_eq!(result["state"], "done");
    assert_eq!(progress.seqs(), log_seqs(&world, &delivering, &job).await);
}

/// Kill the replica a `wait_for_job` runs on, mid-wait: the call breaks, and calling again on
/// the other replica with the cursor of the last notification loses nothing.
async fn killing_a_replica_mid_wait_costs_a_re_call(backend: Backend) {
    let world = World::start(backend).await;
    let doomed = world.instance_with_mcp("orch-1", false).await;
    let survivor = world.instance_with_mcp("orch-2", true).await;

    let (first, seen_first) = client_recording(&doomed, ALICE_TOKEN).await;
    let (_, started) = call(
        &first,
        "start_job",
        json!({"text": "gate the kill", "agent": "plain"}),
    )
    .await;
    let job = started["job_id"].as_str().unwrap().to_owned();
    let peer = first.peer().clone();
    let first_wait = tokio::spawn({
        let job = job.clone();
        async move {
            call_peer(
                &peer,
                "wait_for_job",
                json!({"job_id": job, "after_seq": 0, "timeout_secs": 60}),
            )
            .await
        }
    });
    eventually("the first wait reported the agent working", || async {
        seen_first.says("agent working").then_some(())
    })
    .await;

    // The process dies: no goodbye, the open response breaks.
    doomed.kill();
    first_wait.abort();
    let cursor = *seen_first.seqs().last().expect("the wait reported events");

    // Another client, another replica, the cursor of the last notification: it continues.
    let (second, seen_second) = client_recording(&survivor, ALICE_TOKEN).await;
    let peer = second.peer().clone();
    let second_wait = tokio::spawn({
        let job = job.clone();
        async move {
            call_peer(
                &peer,
                "wait_for_job",
                json!({"job_id": job, "after_seq": cursor, "timeout_secs": 60}),
            )
            .await
        }
    });
    world.plain.release_gate();
    let (is_error, result) = second_wait.await.unwrap().unwrap();
    assert!(!is_error, "{result}");
    assert_eq!(result["outcome"], "finished");
    assert_eq!(result["state"], "done");

    // Together the two calls reported every event of the log once (the first may have seen
    // more than the cursor it is resumed from: what it saw last is what counts).
    let mut seen: Vec<i64> = seen_first
        .seqs()
        .into_iter()
        .filter(|s| *s <= cursor)
        .collect();
    seen.extend(seen_second.seqs());
    assert_eq!(seen, log_seqs(&world, &survivor, &job).await);
}

backends!(
    a_wait_follows_a_job_that_another_replica_delivers,
    killing_a_replica_mid_wait_costs_a_re_call,
    a_job_runs_to_done_and_is_the_users_own_thread,
    an_answer_unblocks_a_job,
    a_cancel_reaches_the_agent,
    a_retry_on_another_replica_is_the_same_job,
);
