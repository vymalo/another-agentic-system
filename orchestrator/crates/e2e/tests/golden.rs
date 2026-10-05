//! Golden transcripts: what the real orchestrator logs for each scripted agent behaviour,
//! written to `docs/api/examples/*.events.json`. `orch-agui-projection` projects them into the
//! AG-UI goldens the web and `tools/agui-conformance` read, so the chat surface is checked against
//! the orchestrator's real event sequences (kinds, status spellings, failure shape, message
//! finality), not only against the schema in `docs/api/chat-api.yaml`.
//!
//! The first message of a scenario enters the log through the application, with no surface in
//! between (`Chat::seed_thread`, `Chat::seed_message`), so the transcripts hold what the core and
//! the A2A adapter wrote and nothing a consumer chose (message and run ids). What a consumer
//! sends over AG-UI is the business of `agui_run.rs` and `agui_connect.rs`.
//!
//! `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test golden` rewrites the files; without it any
//! difference fails the test.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::*;
use orch_app::GateLayer;
use orch_core::{
    A2UI_EXTENSION_V0_9_1, AgentId, MENTIONS_EXTENSION, STEER_EXTENSION, STEPS_EXTENSION,
    TEXT_STREAM_EXTENSION, THREAD_TOOLS_EXTENSION,
};
use orch_testsupport::{
    Chat, FakeAgentOptions, FakeToolServer, FakeToolServerOptions, VerifierScript, with_ui_catalog,
};
use serde_json::{Value, json};

/// The bearer the golden's tool server wants: a made-up value. It is never in the transcript.
const GOLDEN_BEARER: &str = "golden-tool-server-bearer-not-a-real-credential";

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/examples")
}

/// Ids and clocks are not part of the story: threads, timestamps and agent message ids become
/// placeholders. `seq`, actors and every `data` field stay as emitted.
fn normalise(events: Vec<Value>) -> Value {
    Value::Array(
        events
            .into_iter()
            .map(|mut e| {
                e["threadId"] = json!("<thread-id>");
                e["at"] = json!("<timestamp>");
                // a fork names the thread it was cut from
                if e["kind"] == "thread_forked" {
                    e["data"]["from"]["threadId"] = json!("<parent-thread-id>");
                }
                if e["kind"] == "agent_message" {
                    e["data"]["messageId"] = json!("<message-id>");
                }
                // the reasoning stream's id is `<task id>-thinking`, and the task id is random
                if e["kind"] == "agent_reasoning" {
                    e["data"]["messageId"] = json!("<reasoning-id>");
                }
                // a step's id is `<task id>/<the agent's id>`; the task id is random
                if e["kind"] == "agent_step" {
                    let bare = |id: &Value| {
                        let id = id.as_str().unwrap_or_default();
                        json!(format!(
                            "T/{}",
                            id.split_once('/').map_or(id, |(_, rest)| rest)
                        ))
                    };
                    // a relayed call's step is `tool-<the agent's call id>`, which holds the task id
                    let id = e["data"]["id"].as_str().unwrap_or_default().to_owned();
                    if let Some(rest) = id.strip_prefix("tool-")
                        && let Some((_, call)) = rest.split_once(":call-")
                    {
                        e["data"]["id"] = json!(format!("tool-T:call-{call}"));
                    } else {
                        e["data"]["id"] = bare(&e["data"]["id"]);
                    }
                    if let Some(path) = e["data"]["path"].as_array_mut() {
                        for id in path {
                            *id = bare(id);
                        }
                    }
                }
                e
            })
            .collect(),
    )
}

/// One scripted run to its final state; returns the thread's events.
async fn run(world: &World, name: &str) -> Vec<Value> {
    // the agent of `turn-output` calls the orchestrator's thread tools back
    let orch = if matches!(name, "turn-output" | "tools-relay" | "ask-agent") {
        world.instance_with_thread_tools("orch-1", true).await
    } else {
        world.instance("orch-1").await
    };
    let chat = world.chat(&orch);
    let (id, last) = match name {
        "echo" => (chat.seed_thread("plain", "echo hi", None).await, "done"),
        "ask" => {
            let id = chat.seed_thread("plain", "ask about branches", None).await;
            chat.wait_state(&id, "blocked").await;
            let event = chat.seed_message(&id, "main").await;
            assert_eq!(event["kind"], "user_message");
            (id, "done")
        }
        "cancel" => {
            let id = chat.seed_thread("plain", "slow work", None).await;
            chat.wait_state(&id, "working").await;
            assert_eq!(chat.cancel(&id).await, 202);
            (id, "cancelled")
        }
        "fail" => (
            chat.seed_thread("plain", "fail please", None).await,
            "failed",
        ),
        "talk" => (chat.seed_thread("plain", "talk to me", None).await, "done"),
        // A file the agent hands over (ADR 0032): an artifact whose part is a PNG. The worker keeps
        // the bytes in the artifact store and the log holds the reference.
        "file" => (
            chat.seed_thread("plain", "file make a chart", None).await,
            "done",
        ),
        "release" => (
            chat.seed_thread("coder", "echo ship it", Some("staging"))
                .await,
            "done",
        ),
        // A2UI: the agent sends a surface and asks; the user acts on it through the AG-UI run
        // route (the only door an action has) and the agent finishes the same task.
        "a2ui" => {
            let id = chat.seed_thread("plain", "ui pick one", None).await;
            chat.wait_state(&id, "blocked").await;
            let body = Chat::agui_input(
                &id,
                "run-2",
                &[],
                json!({"forwardedProps": {"a2uiAction": {"userAction": {
                    "name": "go",
                    "surfaceId": "s1",
                    "sourceComponentId": "go",
                    "context": {"choice": "a"},
                }}}}),
            );
            let mut response = chat.agui_run("plain", &body).await;
            assert_eq!(response.status, 200);
            let frames = response
                .collect_frames(std::time::Duration::from_secs(20))
                .await;
            assert_eq!(
                frames.last().map(|f| f.event["outcome"]["type"].clone()),
                Some(json!("success"))
            );
            (id, "done")
        }
        // The verification gate (ADR 0018): `plain` runs under `gate: {require: [agent-checks]}`
        // (`world_for`). `verify-green` is red once: the agent's own checks fail, it is sent back
        // with the findings and passes on attempt 2. `verify-red` never passes and runs out of
        // attempts.
        "verify-green" => (
            chat.seed_thread("plain", "verify-red-once fix the login", None)
                .await,
            "done",
        ),
        "verify-red" => (
            chat.seed_thread("plain", "verify-red fix the login", None)
                .await,
            "failed",
        ),
        // The verifier agent in the gate (ADR 0018): `plain` runs under `gate: {require:
        // [verifier], verifier: reviewer}` (`world_for`) and pushes without checking itself.
        // `verify-verifier-green`: the verifier finds something in attempt 1 and is satisfied by
        // the rework. `verify-verifier-red`: it never is.
        "verify-verifier-green" | "verify-verifier-red" => (
            chat.seed_thread("plain", "verify-reviewed fix the login", None)
                .await,
            if name == "verify-verifier-green" {
                "done"
            } else {
                "failed"
            },
        ),
        // CI on the pushed commit (ADR 0017): `plain` runs under `gate: {require: [ci]}`
        // (`world_for`), and the test plays the CI system. The agent pushes commit 1 and
        // completes; CI says `ci/build` failed on it; the agent is sent back, pushes commit 2
        // and completes; CI says it passed.
        "ci" => {
            let node = world.node("ci-node").await;
            let inbox = node.spawn_inbox(fast_inbox(), "ci-node");
            let id = chat
                .seed_thread("plain", "verify-ci fix the login", None)
                .await;
            drive_ci(&chat, &node, &id).await;
            chat.wait_state(&id, "done").await;
            inbox.shutdown().await;
            (id, "done")
        }
        // A thread is a conversation (ADR 0020): a message on a finished thread starts the next
        // job. `followup`: the first job is done, the second is asked for after it.
        "followup" => {
            let id = chat.seed_thread("plain", "echo hi", None).await;
            chat.wait_state(&id, "done").await;
            let event = chat.seed_message(&id, "echo now add tests").await;
            assert_eq!(event["kind"], "user_message");
            (id, "done")
        }
        // `followup-after-cancel`: a stopped job is not closed either.
        "followup-after-cancel" => {
            let id = chat.seed_thread("plain", "slow work", None).await;
            chat.wait_state(&id, "working").await;
            assert_eq!(chat.cancel(&id).await, 202);
            chat.wait_state(&id, "cancelled").await;
            let event = chat.seed_message(&id, "echo never mind, do this").await;
            assert_eq!(event["kind"], "user_message");
            (id, "done")
        }
        // Nested steps (ADR 0025): `plain` lists `steps/v1` (`world_for`). A sub-agent step with
        // a command under it that fails, then the agent's words (`steps`); and a step that is
        // waiting when the agent asks, ended by the answer in the next run (`steps-ask`).
        "steps" => (
            chat.seed_thread("plain", "steps run the tests", None).await,
            "done",
        ),
        "steps-ask" => {
            let id = chat
                .seed_thread("plain", "steps-ask clean the build", None)
                .await;
            chat.wait_state(&id, "blocked").await;
            let event = chat.seed_message(&id, "yes").await;
            assert_eq!(event["kind"], "user_message");
            (id, "done")
        }
        // Working text and the answer (ADR 0031): `plain` lists `text-stream/v1` and `steps/v1`
        // (`world_for`). The words before a tool call are stated on a `working` status, the reply
        // on `completed`: the log marks them `working` and `answer`.
        "working" => (
            chat.seed_thread("plain", "stream-words go", None).await,
            "done",
        ),
        // What the agent's model thought before it answered (ADR 0044): `plain` lists `text-stream/v1`, and
        // the fake sends the reasoning as chunks marked `kind: "reasoning"` and then the reply. The log holds the
        // reasoning once, whole, as an `agent_reasoning`, before the reply's message.
        "reasoning" => (
            chat.seed_thread("plain", "reasoning go", None).await,
            "done",
        ),
        // The agent announces its answer (ADR 0031, `turn_output`): `plain` lists `thread-tools/v1`,
        // `text-stream/v1` and `steps/v1` (`world_for`) and the adapter mints it a grant. It says a
        // sentence before a tool call, calls the tool with the answer and finishes with a short
        // line, which the core writes as working text.
        "turn-output" => (
            chat.seed_thread("plain", "turn-output go", None).await,
            "done",
        ),
        // A person renames the thread (`patchThread`): once while it works (the title is the
        // person's from then on), once more after it is done. The log says who wrote each.
        "title" => {
            let id = chat.seed_thread("plain", "slow work", None).await;
            chat.wait_state(&id, "working").await;
            let (status, thread) = chat.rename(&id, "Fix the login").await;
            assert_eq!(status, 200, "{thread}");
            assert_eq!(thread["title"], "Fix the login");
            assert_eq!(chat.cancel(&id).await, 202);
            chat.wait_state(&id, "cancelled").await;
            let (status, thread) = chat.rename(&id, "Fix the login page").await;
            assert_eq!(status, 200, "{thread}");
            (id, "cancelled")
        }
        // A job's end gets the thread a description from the orchestrator's own model (ADR 0035,
        // `world_for`: the description task is on and the model says one sentence), and a person
        // clears it afterwards: an empty `thread_described`, which is final.
        "description" => {
            world
                .model
                .then_answer("The person wants a plan for a test.");
            let id = chat.seed_thread("plain", "talk to me", None).await;
            chat.wait_state(&id, "done").await;
            eventually("the model's description", || async {
                chat.events(&id)
                    .await
                    .iter()
                    .any(|e| e["kind"] == "thread_described")
                    .then_some(())
            })
            .await;
            let (status, thread) = chat.describe(&id, "").await;
            assert_eq!(status, 200, "{thread}");
            assert!(thread.get("description").is_none(), "{thread}");
            (id, "done")
        }
        // MCP servers attached to a thread (ADR 0024): one when the run creates it (the creation
        // commit holds the message, then the server), and once it is done another is added and that
        // one dropped in a single `PUT`, which is an attach and a detach.
        "tools-attach" => {
            let (status, created) = chat
                .try_create_thread_with_tools("plain", "echo hi", &["websearch"])
                .await;
            assert_eq!(status, 200, "{created}");
            let id = created["threadId"].as_str().unwrap().to_owned();
            chat.wait_state(&id, "done").await;
            let (status, set) = chat.put_tools(&id, &["docs", "repos"]).await;
            assert_eq!(status, 422, "`repos` is the coder's: {set}");
            let (status, set) = chat.put_tools(&id, &["websearch", "repos_"]).await;
            assert_eq!(status, 400, "{set}");
            let (status, set) = chat.put_tools(&id, &["docs"]).await;
            assert_eq!(status, 200, "{set}");
            assert_eq!(set, json!({"servers": ["docs"]}));
            (id, "done")
        }
        // An attached MCP server's tool, relayed (ADR 0024, `thread-tools/v1`): the thread is created
        // with the server attached, the agent calls its tool on the thread's endpoint, and the
        // orchestrator reports the call as one step with the server's icon, its input and output.
        "tools-relay" => {
            let (status, created) = chat
                .try_create_thread_with_tools(
                    "plain",
                    r#"tool websearch__echo {"text":"rust async"}"#,
                    &["websearch"],
                )
                .await;
            assert_eq!(status, 200, "{created}");
            (created["threadId"].as_str().unwrap().to_owned(), "done")
        }
        // Forking a thread (ADR 0029). The log is the fork's: a copy of the parent's events up to
        // the cut, `thread_forked`, then its own life. `fork`: the second message of a finished
        // thread is edited, so the fork holds the first turn and the replacing message, which
        // starts job 2 at once.
        "fork" => {
            let parent = chat.seed_thread("plain", "echo one", None).await;
            chat.wait_state(&parent, "done").await;
            let second = chat.seed_message(&parent, "echo two").await;
            chat.wait_state(&parent, "done").await;
            let (status, forked) = chat
                .post(
                    &format!("/api/threads/{parent}/fork"),
                    Some(json!({"replace": second["seq"], "text": "echo three"})),
                )
                .await;
            assert_eq!(status, 201, "{forked}");
            (forked["id"].as_str().unwrap().to_owned(), "done")
        }
        // `fork-blocked`: a thread that waits for an answer is forked as it is, with its
        // question; the fork's next message starts job 2 and the question is never answered.
        "fork-blocked" => {
            let parent = chat.seed_thread("plain", "ask about branches", None).await;
            chat.wait_state(&parent, "blocked").await;
            let (status, forked) = chat
                .post(
                    &format!("/api/threads/{parent}/fork"),
                    Some(json!({"after": 1})),
                )
                .await;
            assert_eq!(status, 201, "{forked}");
            let fork = forked["id"].as_str().unwrap().to_owned();
            assert_eq!(forked["state"], "done");
            let event = chat.seed_message(&fork, "echo thanks").await;
            assert_eq!(event["kind"], "user_message");
            (fork, "done")
        }
        // The UI's catalog (ADR 0023), through the AG-UI run route, which is the only door a
        // catalog has: the first run of the thread carries version 1; the next job, a message
        // on the finished thread, version 2; the third job version 1 again, from an older
        // screen. Each digest is recorded once, first in its commit, and the log names the
        // consumer's message and run ids, which a route that carries a catalog has.
        "catalog" => {
            let id = "00000000-0000-7000-8000-000000000301".to_owned();
            for (n, (text, version)) in [("echo hi", 1), ("echo again", 2), ("echo once more", 1)]
                .into_iter()
                .enumerate()
            {
                let n = n + 1;
                let body = Chat::agui_input(
                    &id,
                    &format!("run-{n}"),
                    &[(&format!("msg-{n}"), text)],
                    with_ui_catalog(version),
                );
                let mut response = chat.agui_run("plain", &body).await;
                assert_eq!(response.status, 200);
                let frames = response
                    .collect_frames(std::time::Duration::from_secs(20))
                    .await;
                assert_eq!(
                    frames.last().map(|f| f.event["outcome"]["type"].clone()),
                    Some(json!("success"))
                );
                chat.wait_state(&id, "done").await;
            }
            (id, "done")
        }
        // A message sent while the agent works (ADR 0036), through the AG-UI run route, which is
        // the door that says how it is delivered (`forwardedProps["vymalo.send"]`): the log holds
        // the consumer's message and run ids. `steer`: the agent lists `steer/v1`, so the message
        // goes into its running task, which reads it at its next step and says what it read; the
        // job goes on as one job and ends once.
        "steer" => {
            let id = chat
                .seed_thread("plain", "steerable refactor the parser", None)
                .await;
            chat.wait_state(&id, "working").await;
            let body = Chat::agui_input(
                &id,
                "run-2",
                &[("msg-2", "you were wrong since line 1")],
                json!({"forwardedProps": {"vymalo.send": "steer"}}),
            );
            let response = chat.agui_run("plain", &body).await;
            assert_eq!(response.status, 200);
            eventually(&format!("the task of {id} to read the message"), || async {
                let events = chat.events(&id).await;
                events
                    .iter()
                    .any(|e| {
                        e["kind"] == "agent_message"
                            && e["data"]["text"] == "steered: you were wrong since line 1"
                    })
                    .then_some(())
            })
            .await;
            world.plain.release_gate();
            (id, "done")
        }
        // `stop-and-send`: the running task is cancelled, and the message starts job 2 once it
        // has ended; the abandoned job is never judged (no `thread_state`).
        "stop-and-send" => {
            let id = chat
                .seed_thread("plain", "slow refactor the parser", None)
                .await;
            chat.wait_state(&id, "working").await;
            let body = Chat::agui_input(
                &id,
                "run-2",
                &[("msg-2", "echo do X instead")],
                json!({"forwardedProps": {"vymalo.send": "interrupt"}}),
            );
            let response = chat.agui_run("plain", &body).await;
            assert_eq!(response.status, 200);
            wait_for_job(&chat, &id, 2).await;
            (id, "done")
        }
        // Mentions (ADR 0026, `mentions/v1`), through the AG-UI run route, which is the only door a
        // mention has: a message to `plain`, whose card lists the extension, that mentions `coder`.
        // The emoji is two UTF-16 code units, so `@coder` stands at 3..9. The log holds the
        // reference as sent, with the consumer's message and run ids.
        "mentions" => {
            let id = "00000000-0000-7000-8000-000000000302".to_owned();
            let body = Chat::agui_input(
                &id,
                "run-1",
                &[("msg-1", "\u{1F604} @coder echo the build, please")],
                json!({"forwardedProps": {"vymalo.mentions": [
                    {"agentId": "coder", "label": "@coder", "start": 3, "end": 9}
                ]}}),
            );
            let mut response = chat.agui_run("plain", &body).await;
            assert_eq!(response.status, 200);
            let frames = response
                .collect_frames(std::time::Duration::from_secs(20))
                .await;
            assert_eq!(
                frames.last().map(|f| f.event["outcome"]["type"].clone()),
                Some(json!("success"))
            );
            (id, "done")
        }
        // Asked agents (ADR 0026, `ask_agent`), through the AG-UI run route: `plain` coordinates. It
        // asks `coder` to coordinate with `researcher` in its turn (an ask of an asked agent: ask 1
        // by `main`, ask 2 by `ask:1`, which ends first), and then asks `researcher` for something
        // that fails (ask 3, a failure the asker reads and goes on from). The labels are prefixes of
        // the chains the fake agent reads, so the text is the person's own.
        "ask-agent" => {
            let id = "00000000-0000-7000-8000-000000000303".to_owned();
            let text = "coordinate @coder>researcher @researcher!fail -- go";
            let body = Chat::agui_input(
                &id,
                "run-1",
                &[("msg-1", text)],
                json!({"forwardedProps": {"vymalo.mentions": [
                    {"agentId": "coder", "label": "@coder", "start": 11, "end": 17},
                    {"agentId": "researcher", "label": "@researcher", "start": 29, "end": 40}
                ]}}),
            );
            let mut response = chat.agui_run("plain", &body).await;
            assert_eq!(response.status, 200);
            let frames = response
                .collect_frames(std::time::Duration::from_secs(20))
                .await;
            assert_eq!(
                frames.last().map(|f| f.event["outcome"]["type"].clone()),
                Some(json!("success"))
            );
            (id, "done")
        }
        other => panic!("unknown scenario {other}"),
    };
    chat.wait_state(&id, last).await;
    chat.events(&id).await
}

/// Waits until job `job` of the thread has started and its task has ended: a `thread_state` that
/// is `done` after its `job_started`.
async fn wait_for_job(chat: &Chat, id: &str, job: u64) {
    eventually(&format!("job {job} of {id} to be done"), || async {
        let events = chat.events(id).await;
        let started = events
            .iter()
            .position(|e| e["kind"] == "job_started" && e["data"]["job"] == job)?;
        events[started..]
            .iter()
            .any(|e| e["kind"] == "thread_state" && e["data"]["state"] == "done")
            .then_some(())
    })
    .await;
}

const SCENARIOS: [&str; 31] = [
    "echo",
    "file",
    "ask",
    "cancel",
    "fail",
    "talk",
    "release",
    "a2ui",
    "verify-green",
    "verify-red",
    "verify-verifier-green",
    "verify-verifier-red",
    "ci",
    "followup",
    "followup-after-cancel",
    "catalog",
    "steps",
    "steps-ask",
    "working",
    "reasoning",
    "turn-output",
    "title",
    "description",
    "fork",
    "fork-blocked",
    "tools-attach",
    "tools-relay",
    "steer",
    "stop-and-send",
    "mentions",
    "ask-agent",
];

/// The world a scenario runs in: the plain agent lists the A2UI extension for `a2ui`.
async fn world_for(name: &str, tool_server: Option<&FakeToolServer>) -> World {
    match name {
        // the deployment lists a web search, which the relay calls with a bearer (ADR 0024)
        "tools-relay" => {
            let server = tool_server.expect("the scenario has a tool server");
            let endpoint = orch_ports::ToolServerEndpoint::new(
                "websearch",
                server.url(),
                std::time::Duration::from_secs(5),
            )
            .with_bearer(orch_ports::ToolSecret::new(GOLDEN_BEARER));
            World::with(
                Backend::Memory,
                Setup {
                    thread_tools: true,
                    plain: FakeAgentOptions {
                        extensions: vec![THREAD_TOOLS_EXTENSION.to_owned()],
                        ..FakeAgentOptions::default()
                    },
                    tool_servers: vec![orch_app::ToolServerInfo {
                        description: Some("Search the web.".to_owned()),
                        ..orch_app::ToolServerInfo::new("websearch", "Web search")
                    }],
                    tool_endpoints: vec![endpoint],
                    ..Setup::default()
                },
            )
            .await
        }
        // `plain` coordinates the mentioned agents with `ask_agent`: it lists `mentions/v1` and
        // `thread-tools/v1`, and so do the agents it asks, which get the grant of an asked agent
        "ask-agent" => {
            let listing = |extensions: &[&str]| FakeAgentOptions {
                extensions: extensions.iter().map(|e| (*e).to_owned()).collect(),
                ..FakeAgentOptions::default()
            };
            World::with(
                Backend::Memory,
                Setup {
                    thread_tools: true,
                    coder: FakeAgentOptions {
                        releases: None,
                        ..listing(&[THREAD_TOOLS_EXTENSION])
                    },
                    plain: listing(&[THREAD_TOOLS_EXTENSION, MENTIONS_EXTENSION]),
                    extra: vec![("researcher", listing(&[THREAD_TOOLS_EXTENSION]))],
                    ..Setup::default()
                },
            )
            .await
        }
        // `plain` lists `mentions/v1`, so the mention is told to it
        "mentions" => {
            World::with(
                Backend::Memory,
                Setup {
                    plain: FakeAgentOptions {
                        extensions: vec![MENTIONS_EXTENSION.to_owned()],
                        ..FakeAgentOptions::default()
                    },
                    ..Setup::default()
                },
            )
            .await
        }
        // `plain` lists `steer/v1`: a message sent while it works is read by its running task
        "steer" => {
            World::with(
                Backend::Memory,
                Setup {
                    plain: FakeAgentOptions {
                        extensions: vec![STEER_EXTENSION.to_owned()],
                        ..FakeAgentOptions::default()
                    },
                    ..Setup::default()
                },
            )
            .await
        }
        "a2ui" => {
            World::with(
                Backend::Memory,
                Setup {
                    plain: FakeAgentOptions {
                        ui_extensions: vec![A2UI_EXTENSION_V0_9_1.to_owned()],
                        ..FakeAgentOptions::default()
                    },
                    ..Setup::default()
                },
            )
            .await
        }
        // `plain` lists `steps/v1`, so the orchestrator asks it for steps
        "steps" | "steps-ask" => {
            World::with(
                Backend::Memory,
                Setup {
                    plain: FakeAgentOptions {
                        extensions: vec![STEPS_EXTENSION.to_owned()],
                        ..FakeAgentOptions::default()
                    },
                    ..Setup::default()
                },
            )
            .await
        }
        // `plain` lists `text-stream/v1` and `steps/v1`: it states its words and the reply as streams
        "working" | "reasoning" => {
            World::with(
                Backend::Memory,
                Setup {
                    plain: FakeAgentOptions {
                        extensions: vec![
                            TEXT_STREAM_EXTENSION.to_owned(),
                            STEPS_EXTENSION.to_owned(),
                        ],
                        ..FakeAgentOptions::default()
                    },
                    ..Setup::default()
                },
            )
            .await
        }
        // `plain` lists `thread-tools/v1` as well, and the adapter has the key to mint its grant
        "turn-output" => {
            World::with(
                Backend::Memory,
                Setup {
                    thread_tools: true,
                    plain: FakeAgentOptions {
                        extensions: vec![
                            THREAD_TOOLS_EXTENSION.to_owned(),
                            TEXT_STREAM_EXTENSION.to_owned(),
                            STEPS_EXTENSION.to_owned(),
                        ],
                        ..FakeAgentOptions::default()
                    },
                    ..Setup::default()
                },
            )
            .await
        }
        // the deployment offers servers to attach (ADR 0024)
        "tools-attach" => {
            World::with(
                Backend::Memory,
                Setup {
                    tool_servers: sample_tool_servers(),
                    ..Setup::default()
                },
            )
            .await
        }
        // the description task is on, at the one endpoint the scripted model holds
        "description" => {
            World::with(
                Backend::Memory,
                Setup {
                    descriptions: true,
                    ..Setup::default()
                },
            )
            .await
        }
        "verify-green" | "verify-red" => {
            let gate = GateLayer::from_json(&json!({"require": ["agent-checks"]}))
                .unwrap()
                .unwrap();
            World::with(
                Backend::Memory,
                Setup {
                    target_gates: BTreeMap::from([(AgentId::new("plain"), gate)]),
                    ..Setup::default()
                },
            )
            .await
        }
        "verify-verifier-green" | "verify-verifier-red" => {
            let script = if name == "verify-verifier-green" {
                VerifierScript::FindingsThenPass
            } else {
                VerifierScript::AlwaysFail
            };
            World::with(Backend::Memory, verified_by_reviewer(script)).await
        }
        "ci" => {
            let gate =
                GateLayer::from_json(&json!({"require": ["ci"], "ci": {"required": ["ci/build"]}}))
                    .unwrap()
                    .unwrap();
            World::with(
                Backend::Memory,
                Setup {
                    target_gates: BTreeMap::from([(AgentId::new("plain"), gate)]),
                    ..Setup::default()
                },
            )
            .await
        }
        _ => World::start(Backend::Memory).await,
    }
}

#[tokio::test]
async fn transcripts_match_docs_api_examples() {
    let update = std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    let dir = examples_dir();
    let mut stale = Vec::new();
    for name in SCENARIOS {
        // the tool server of the relay scenario lives as long as the scenario runs
        let tool_server = if name == "tools-relay" {
            Some(
                FakeToolServer::spawn(FakeToolServerOptions::default().bearer(GOLDEN_BEARER)).await,
            )
        } else {
            None
        };
        let world = world_for(name, tool_server.as_ref()).await;
        let got = normalise(run(&world, name).await);
        let path = dir.join(format!("{name}.events.json"));
        let mut text = serde_json::to_string_pretty(&got).unwrap();
        text.push('\n');
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &text).unwrap();
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(want) if want == text => {}
            Ok(want) => stale.push(format!(
                "{}: differs\n--- want\n{want}--- got\n{text}",
                path.display()
            )),
            Err(e) => stale.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(
        stale.is_empty(),
        "golden transcripts are out of date; run `UPDATE_GOLDEN=1 cargo test -p orch-e2e --test golden` and review the diff:\n{}",
        stale.join("\n")
    );
}

async fn an_agent_that_talks_shows_its_status_text_and_message_once(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let id = chat.create_thread("plain", "talk to me", None).await;
    chat.wait_state(&id, "done").await;
    let events = chat.events(&id).await;
    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "agent_status:working",
            "agent_message",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    assert_contiguous(&events);
    assert_eq!(events[2]["data"]["detail"], "Reading the repository");
    let message = &events[3];
    assert_eq!(message["actor"]["type"], "agent");
    assert_eq!(message["actor"]["name"], "plain");
    assert_eq!(message["data"]["text"], "Plan: add a test");
    assert_eq!(
        message["data"]["final"], true,
        "the adapter reports agent messages as final"
    );
    assert!(
        message["data"]["messageId"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert_eq!(data_of(&events, "agent_message").len(), 1);
}

backends!(an_agent_that_talks_shows_its_status_text_and_message_once);
