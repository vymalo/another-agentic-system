//! The real AG-UI run route, dispatcher and A2A adapter against the WireMock stand-in agents of
//! `compose.yaml` (`dev/wiremock/agent*`), to keep the mocks honest.
//!
//! Gated: each test skips, printing a notice, unless its environment variable holds the
//! mock's base URL, for example after `docker compose up -d --wait mock-agent mock-agent-releases`:
//!
//! ```sh
//! ORCH_TEST_MOCK_AGENT_URL=http://127.0.0.1:8081 \
//! ORCH_TEST_MOCK_AGENT_RELEASES_URL=http://127.0.0.1:8082 \
//! ORCH_TEST_MOCK_VERIFIER_URL=http://127.0.0.1:8083 \
//!   cargo test -p orch-e2e --test wiremock_agent
//! ```
//!
//! (`mock-verifier` too, for the tests of the verifier agent of the gate.)
//!
//! The store is the in-memory one: the mocks, not persistence, are under test.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, GateLayer};
use orch_core::AgentId;
use orch_ports::memory::{MemoryStore, MemoryWakeup};
use orch_ports::{AgentEndpoint, PortSet, SystemClock, UuidV7Ids};
use orch_testsupport::{Chat, TestInstance, fast_dispatcher, shape};

const MOCK_URL: &str = "ORCH_TEST_MOCK_AGENT_URL";
const RELEASES_URL: &str = "ORCH_TEST_MOCK_AGENT_RELEASES_URL";
const VERIFIER_URL: &str = "ORCH_TEST_MOCK_VERIFIER_URL";
/// Any non-empty bearer token is accepted by the mocks; this one is what `dev/agents.yaml` uses.
const DUMMY_TOKEN: &str = "dev-mock-token";
const PR_URL: &str = "https://github.com/example/sandbox/pull/1";

/// An orchestrator serving one agent per configured mock, plus a chat client for it.
struct Rig {
    chat: Chat,
    _instance: TestInstance,
}

/// The base URL in `var`, or `None` (with a notice) when the test is not enabled.
fn mock(var: &str) -> Option<String> {
    let url = std::env::var(var).ok().filter(|u| !u.trim().is_empty());
    if url.is_none() {
        eprintln!("skipping: {var} is not set (see the header of this file)");
    }
    url
}

async fn rig(agents: &[(&str, &str)]) -> Rig {
    let entries = agents
        .iter()
        .map(|(id, base)| AgentEntry {
            name: (*id).to_owned(),
            endpoint: AgentEndpoint::a2a(
                AgentId::new(*id),
                format!("{}/.well-known/agent-card.json", base.trim_end_matches('/')),
                Some(DUMMY_TOKEN.to_owned()),
            ),
        })
        .collect();
    let client = A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap();
    let directory = AgentDirectory::new(entries);
    let app = Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: client,
                clock: SystemClock,
                ids: UuidV7Ids,
                model: orch_ports::NoModel,
                auth: orch_auth_header::HeaderAuth::new(),
                registry: directory.fixed_registry(),
            },
            directory,
            AppConfig {
                stream_poll: Duration::from_millis(100),
                // `dev/agents.yaml` gates `mock-coder-gated` (its own checks must pass) and
                // `mock-coder-verified` (the agent `verifier` must pass its commit).
                target_gates: [
                    (
                        AgentId::new("mock-coder-gated"),
                        GateLayer::from_json(&serde_json::json!({"require": ["agent-checks"]}))
                            .unwrap()
                            .unwrap(),
                    ),
                    (
                        AgentId::new("mock-coder-verified"),
                        GateLayer::from_json(
                            &serde_json::json!({"require": ["verifier"], "verifier": "verifier"}),
                        )
                        .unwrap()
                        .unwrap(),
                    ),
                ]
                .into(),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    );
    let api = ApiConfig {
        sse_keepalive: Duration::from_millis(150),
        ..ApiConfig::default()
    };
    let instance = TestInstance::spawn(app, api, fast_dispatcher(), "orch-mock").await;
    Rig {
        chat: instance.chat("dev@example.com"),
        _instance: instance,
    }
}

#[tokio::test]
async fn the_default_script_completes_with_the_pull_request_artifact() {
    let Some(url) = mock(MOCK_URL) else { return };
    let rig = rig(&[("mock-coder", &url)]).await;
    let id = rig
        .chat
        .create_thread("mock-coder", "add a health endpoint", None)
        .await;
    rig.chat.wait_state(&id, "done").await;
    let events = rig.chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    assert_eq!(events[2]["data"]["uri"], PR_URL);
    assert_eq!(events[2]["data"]["name"], "Pull request");
}

#[tokio::test]
async fn the_stream_keyword_streams_a_reply_that_a_viewer_reads_grow_and_the_log_holds_once() {
    // text-stream/v1 (ADR 0027): the card lists it, the six chunks of the reply are relayed and
    // never logged, and the log holds the whole text once, from the status that ends the turn
    let Some(url) = mock(MOCK_URL) else { return };
    let rig = rig(&[("mock-coder", &url)]).await;
    let id = rig
        .chat
        .create_thread("mock-coder", "stream tell me", None)
        .await;
    let mut viewer = rig.chat.agui_connect(&id, None, false).await;
    let frames = viewer
        .frames_until(Duration::from_secs(60), |f| {
            f.event["type"] == "RUN_FINISHED"
        })
        .await;
    rig.chat.wait_state(&id, "done").await;
    let events = rig.chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_message",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    let whole = "Streaming a reply, word by word, so the chat can show it grow.";
    assert_eq!(events[2]["data"]["text"], whole);
    assert_eq!(events[2]["data"]["final"], true);
    let reply = events[2]["data"]["messageId"].as_str().unwrap();
    assert!(reply.ends_with("-reply"), "{reply}");

    // the dribbled reply reached the viewer in pieces before the log's message completed it
    let live = |f: &&orch_testsupport::Frame| {
        f.event["messageId"] == reply
            && f.event["type"] == "TEXT_MESSAGE_CONTENT"
            && f.event["metadata"]["vymalo.live"].is_object()
            && f.event["metadata"]["vymalo.live"]["final"].is_null()
    };
    let grown = frames.iter().filter(live).count();
    assert!(grown >= 3, "{grown} live deltas: {frames:?}");
    let delta_of = |f: &orch_testsupport::Frame| f.event["delta"].as_str().unwrap().to_owned();
    let text: String = frames
        .iter()
        .filter(|f| f.event["messageId"] == reply && f.event["type"] == "TEXT_MESSAGE_CONTENT")
        .map(delta_of)
        .collect();
    assert_eq!(text, whole);
}

#[tokio::test]
async fn the_steps_keyword_reports_a_sub_agent_with_a_command_that_fails_under_it() {
    // steps/v1 (ADR 0025): the card lists it, and the metadata of each `working` status message
    // is a step, which the adapter reads and the core logs with its path
    let Some(url) = mock(MOCK_URL) else { return };
    let rig = rig(&[("mock-coder", &url)]).await;
    let id = rig
        .chat
        .create_thread("mock-coder", "steps run the tests", None)
        .await;
    rig.chat.wait_state(&id, "done").await;
    let events = rig.chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_step",
            "agent_step",
            "agent_step",
            "agent_step",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    let task = events[2]["data"]["id"]
        .as_str()
        .and_then(|id| id.split_once('/'))
        .map(|(task, _)| task.to_owned())
        .expect("a step id is `<task>/<the agent's id>`");
    let story: Vec<(String, Vec<String>, &str, &str)> = events[2..6]
        .iter()
        .map(|e| {
            let d = &e["data"];
            (
                d["id"].as_str().unwrap().to_owned(),
                d["path"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p.as_str().unwrap().to_owned())
                    .collect(),
                d["state"].as_str().unwrap(),
                d["phase"].as_str().unwrap(),
            )
        })
        .collect();
    let (o, c) = (format!("{task}/tool:c2"), format!("{task}/acp:c2:1"));
    assert_eq!(
        story,
        [
            (o.clone(), vec![], "running", "start"),
            (c.clone(), vec![o.clone()], "running", "start"),
            (c, vec![o.clone()], "failed", "end"),
            (o, vec![], "completed", "end"),
        ]
    );
    assert_eq!(events[4]["data"]["detail"], "1 failed");
    assert_eq!(events[5]["data"]["label"], "OpenCode");
}

#[tokio::test]
async fn ask_blocks_the_thread_and_the_answer_completes_the_same_task() {
    let Some(url) = mock(MOCK_URL) else { return };
    let rig = rig(&[("mock-coder", &url)]).await;
    let id = rig
        .chat
        .create_thread("mock-coder", "ask me about the branch", None)
        .await;
    rig.chat.wait_state(&id, "blocked").await;
    let events = rig.chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_status:input_required",
            "thread_state:blocked"
        ]
    );
    assert_eq!(
        events[2]["data"]["detail"],
        "Which branch should I base the change on?"
    );

    let run = rig.chat.follow_up(&id, "mock-coder", "main").await;
    assert_eq!(run.status, 200);
    rig.chat.wait_state(&id, "done").await;
    let events = rig.chat.events(&id).await;
    assert_eq!(
        shape(&events)[4..],
        [
            "user_message",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    assert_eq!(events[6]["data"]["uri"], PR_URL);
}

#[tokio::test]
async fn fail_fails_the_thread_with_the_agents_message() {
    let Some(url) = mock(MOCK_URL) else { return };
    let rig = rig(&[("mock-coder", &url)]).await;
    let id = rig
        .chat
        .create_thread("mock-coder", "fail please", None)
        .await;
    rig.chat.wait_state(&id, "failed").await;
    let events = rig.chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_status:failed",
            "thread_state:failed"
        ]
    );
    assert!(
        events[2]["data"]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("on purpose")),
        "{}",
        events[2]
    );
}

#[tokio::test]
async fn a_jsonrpc_error_is_a_permanent_rejection_and_reject_is_a_failed_task() {
    let Some(url) = mock(MOCK_URL) else { return };
    let rig = rig(&[("mock-coder", &url)]).await;

    let id = rig
        .chat
        .create_thread("mock-coder", "error out", None)
        .await;
    rig.chat.wait_state(&id, "failed").await;
    let events = rig.chat.events(&id).await;
    assert_eq!(
        shape(&events),
        ["user_message", "error", "thread_state:failed"]
    );
    assert_eq!(events[1]["data"]["retryable"], false);

    let id = rig
        .chat
        .create_thread("mock-coder", "reject it", None)
        .await;
    rig.chat.wait_state(&id, "failed").await;
    let last = rig.chat.events(&id).await;
    assert_eq!(shape(&last).last().unwrap(), "thread_state:failed");
}

#[tokio::test]
async fn a_thread_can_be_cancelled_while_blocked() {
    let Some(url) = mock(MOCK_URL) else { return };
    let rig = rig(&[("mock-coder", &url)]).await;
    let id = rig
        .chat
        .create_thread("mock-coder", "ask first", None)
        .await;
    rig.chat.wait_state(&id, "blocked").await;
    assert_eq!(rig.chat.cancel(&id).await, 202);
    rig.chat.wait_state(&id, "cancelled").await;
}

#[tokio::test]
async fn the_release_mock_offers_channels_and_echoes_the_resolved_revision() {
    let Some(url) = mock(RELEASES_URL) else {
        return;
    };
    let rig = rig(&[("mock-coder-releases", &url)]).await;

    let (status, agents) = rig.chat.get("/api/agents").await;
    assert_eq!(status, 200, "{agents}");
    let releases = &agents[0]["releases"];
    assert_eq!(releases["defaultChannel"], "production");
    assert_eq!(releases["channels"]["staging"], "coder-r51");
    assert_eq!(
        releases["revisions"],
        serde_json::json!(["coder-r53", "coder-r51", "coder-r47"])
    );

    for (release, revision) in [
        (Some("staging"), "coder-r51"),
        (Some("coder-r53"), "coder-r53"),
        (None, "coder-r47"),
    ] {
        let id = rig
            .chat
            .create_thread("mock-coder-releases", "ship it", release)
            .await;
        rig.chat.wait_state(&id, "done").await;
        let events = rig.chat.events(&id).await;
        assert_eq!(events[1]["actor"]["revision"], revision, "{release:?}");
    }

    // The orchestrator refuses an unknown release before it reaches the agent.
    let (status, _) = rig
        .chat
        .try_create_thread("mock-coder-releases", "ship it", Some("nope"))
        .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn red_once_is_sent_back_with_its_findings_and_passes_the_second_attempt() {
    let Some(url) = mock(MOCK_URL) else { return };
    let rig = rig(&[("mock-coder-gated", &url)]).await;
    let id = rig
        .chat
        .create_thread("mock-coder-gated", "red-once fix the login", None)
        .await;
    rig.chat.wait_state(&id, "done").await;
    let events = rig.chat.events(&id).await;
    let kinds = shape(&events);
    assert_eq!(
        kinds.iter().filter(|k| *k == "rework").count(),
        1,
        "{kinds:?}"
    );
    let checks: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "check_result")
        .map(|e| e["data"]["status"].as_str().unwrap())
        .collect();
    assert_eq!(checks, ["failed", "passed"], "{kinds:?}");
    let rework = events.iter().find(|e| e["kind"] == "rework").unwrap();
    assert_eq!(rework["data"]["attempt"], 2);
    assert!(
        rework["data"]["findings"][0]["findings"][0]
            .as_str()
            .is_some_and(|f| f.starts_with("red-once:")),
        "{rework}"
    );
    assert_eq!(rig.chat.thread(&id).await["job"]["attempt"], 2);
}

#[tokio::test]
async fn red_always_runs_out_of_attempts_and_fails_the_thread() {
    let Some(url) = mock(MOCK_URL) else { return };
    let rig = rig(&[("mock-coder-gated", &url)]).await;
    let id = rig
        .chat
        .create_thread("mock-coder-gated", "red-always fix the login", None)
        .await;
    rig.chat.wait_state(&id, "failed").await;
    let events = rig.chat.events(&id).await;
    let kinds = shape(&events);
    assert_eq!(
        kinds.iter().filter(|k| *k == "rework").count(),
        2,
        "{kinds:?}"
    );
    assert_eq!(
        kinds.iter().filter(|k| *k == "check_result").count(),
        3,
        "{kinds:?}"
    );
    assert_eq!(kinds.last().unwrap(), "thread_state:failed");
    assert_eq!(rig.chat.thread(&id).await["job"]["attempt"], 3);
}

#[tokio::test]
async fn the_mock_verifier_finds_fault_once_and_passes_the_rework() {
    let (Some(agent), Some(verifier)) = (mock(MOCK_URL), mock(VERIFIER_URL)) else {
        return;
    };
    let rig = rig(&[("mock-coder-verified", &agent), ("verifier", &verifier)]).await;
    let id = rig
        .chat
        .create_thread("mock-coder-verified", "push-flawed fix the login", None)
        .await;
    rig.chat.wait_state(&id, "done").await;
    let events = rig.chat.events(&id).await;
    let kinds = shape(&events);
    assert_eq!(
        kinds.iter().filter(|k| *k == "rework").count(),
        1,
        "{kinds:?}"
    );
    let answers: Vec<(&str, &str)> = events
        .iter()
        .filter(|e| e["kind"] == "check_result" && e["data"]["status"] != "pending")
        .map(|e| {
            (
                e["data"]["source"].as_str().unwrap(),
                e["data"]["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(answers, [("verifier", "failed"), ("verifier", "passed")]);
    let rework = events.iter().find(|e| e["kind"] == "rework").unwrap();
    assert_eq!(
        rework["data"]["findings"][0]["findings"][0],
        "src/login.rs: the empty password is accepted; add a test that covers it"
    );
    let record = rig.chat.thread(&id).await;
    assert_eq!(record["job"]["attempt"], 2);
    assert_eq!(record["job"]["sha"], "b".repeat(40));
    assert!(
        events
            .iter()
            .all(|e| e["kind"] != "agent_status" || e["actor"]["name"] == "mock-coder-verified"),
        "nothing the verifier said became the coder's"
    );
}

#[tokio::test]
async fn a_clean_push_is_passed_by_the_mock_verifier_at_once() {
    let (Some(agent), Some(verifier)) = (mock(MOCK_URL), mock(VERIFIER_URL)) else {
        return;
    };
    let rig = rig(&[("mock-coder-verified", &agent), ("verifier", &verifier)]).await;
    let id = rig
        .chat
        .create_thread("mock-coder-verified", "push-clean fix the login", None)
        .await;
    rig.chat.wait_state(&id, "done").await;
    let events = rig.chat.events(&id).await;
    let kinds = shape(&events);
    assert!(!kinds.contains(&"rework".to_owned()), "{kinds:?}");
    assert_eq!(rig.chat.thread(&id).await["job"]["attempt"], 1);
}
