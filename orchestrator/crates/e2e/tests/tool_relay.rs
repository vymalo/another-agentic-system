//! The relay of the attached MCP servers end to end (`thread-tools/v1`, ADR 0024), on both stores:
//! the real router, dispatcher and A2A adapter, the in-process fake agent whose `tool <name> <json>`
//! script calls the thread's endpoint as an agent does (rmcp's client), the relay over the real MCP
//! client (`orch-tools-mcp`), and `FakeToolServer`s, real MCP servers on their own ports.
//!
//! What is proven here that the relay's own tests (`MemoryToolServers`) cannot: the upstream sees the
//! bearer and the header on the wire, the error table through the real client (a closed port, a
//! slow server, a refused bearer, an `isError`, a result over 256 KiB), a call the agent drops, and
//! that no credential reaches any table, the log, the export or a frame.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use orch_app::ToolServerInfo;
use orch_core::THREAD_TOOLS_EXTENSION;
use orch_ports::{ToolSecret, ToolServerEndpoint};
use orch_testsupport::{Chat, FakeAgentOptions, FakeToolServer, FakeToolServerOptions, Frame};
use rmcp::model::{CallToolRequestParams, MetaObject, RequestMetaObject};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);
const BEARER: &str = "relay-bearer-8f3a2c71d4e85b60-not-a-real-credential";
const API_KEY: &str = "relay-api-key-5a1c7e93b2d04f68-not-a-real-credential";
const ICON: &str = "data:image/svg+xml;base64,PHN2Zy8+";

type Client = RunningService<RoleClient, ()>;

fn info(id: &str, name: &str) -> ToolServerInfo {
    ToolServerInfo {
        icon: Some(ICON.to_owned()),
        timeout: Duration::from_secs(2),
        ..ToolServerInfo::new(id, name)
    }
}

/// A world with the relay: the servers of `servers` (info and endpoint), and a `plain` agent whose
/// card lists `thread-tools/v1`.
async fn world_with(backend: Backend, servers: Vec<(ToolServerInfo, ToolServerEndpoint)>) -> World {
    World::with(
        backend,
        Setup {
            thread_tools: true,
            plain: FakeAgentOptions {
                extensions: vec![THREAD_TOOLS_EXTENSION.to_owned()],
                ..FakeAgentOptions::default()
            },
            tool_servers: servers.iter().map(|(i, _)| i.clone()).collect(),
            tool_endpoints: servers.into_iter().map(|(_, e)| e).collect(),
            ..Setup::default()
        },
    )
    .await
}

/// The step events of a thread: `(phase, state, id)` and the data.
fn steps(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .filter(|e| e["kind"] == "agent_step")
        .map(|e| &e["data"])
        .collect()
}

fn reports(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["kind"] == "artifact" && e["data"]["name"] == "result")
        .map(|e| e["data"]["text"].as_str().unwrap().to_owned())
        .collect()
}

/// Waits until the thread is `done` in its `jobs`-th job (one `done` state per job).
async fn wait_for_jobs(chat: &Chat, thread: &str, jobs: usize) {
    orch_testsupport::eventually("the job to finish", || async {
        let events = chat.events(thread).await;
        let done = events
            .iter()
            .filter(|e| e["kind"] == "thread_state" && e["data"]["state"] == "done")
            .count();
        (done == jobs).then_some(())
    })
    .await;
}

async fn run(chat: &Chat, thread: &str, run: &str, message: &str) -> Vec<Frame> {
    let body = Chat::agui_input(thread, run, &[(&format!("m-{run}"), message)], json!({}));
    let mut sse = chat.agui_run("plain", &body).await;
    assert_eq!(sse.status, 200);
    sse.collect_frames(WAIT).await
}

async fn connect(grant: &Value) -> Client {
    let config =
        StreamableHttpClientTransportConfig::with_uri(grant["url"].as_str().unwrap().to_owned())
            .auth_header(grant["token"].as_str().unwrap());
    ().serve(StreamableHttpClientTransport::from_config(config))
        .await
        .expect("the MCP handshake")
}

/// A call with `_meta["thread-tools/v1"]`; `Ok((is_error, text))` or the protocol error's code.
async fn call(
    client: &Client,
    tool: &str,
    args: Value,
    meta: Value,
) -> Result<(bool, String), i32> {
    let Value::Object(args) = args else {
        panic!("arguments are an object")
    };
    let mut params = CallToolRequestParams::new(tool.to_owned()).with_arguments(args);
    if let Value::Object(meta) = json!({"thread-tools/v1": meta}) {
        params.meta = Some(RequestMetaObject(MetaObject(meta)));
    }
    match client.call_tool(params).await {
        Ok(result) => Ok((
            result.is_error == Some(true),
            result
                .content
                .iter()
                .filter_map(|c| match c {
                    rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )),
        Err(rmcp::ServiceError::McpError(data)) => Err(data.code.0),
        Err(other) => panic!("not a JSON-RPC error: {other}"),
    }
}

/// A thread of `plain` with `servers` attached whose agent is at work (`slow work`), and the grant
/// of its message: the endpoint and token to call as the agent.
async fn working_thread(world: &World, chat: &Chat, servers: &[&str]) -> (String, Value) {
    let (status, created) = chat
        .try_create_thread_with_tools("plain", "slow work", servers)
        .await;
    assert_eq!(status, 200, "{created}");
    let thread = created["threadId"].as_str().unwrap().to_owned();
    chat.wait_state(&thread, "working").await;
    let grant = world.grants_of_plain().remove(0);
    (thread, grant)
}

// ---- the agent's own loop: FakeAgent -> endpoint -> relay -> FakeToolServer -------------------

async fn the_agents_call_is_relayed_with_the_bearer_and_is_one_step_and_no_credential_is_kept(
    backend: Backend,
) {
    let server = FakeToolServer::spawn(
        FakeToolServerOptions::default()
            .bearer(BEARER)
            .require_header("X-Api-Key", API_KEY),
    )
    .await;
    let endpoint = ToolServerEndpoint::new("websearch", server.url(), Duration::from_secs(5))
        .with_bearer(ToolSecret::new(BEARER))
        .with_header("X-Api-Key", ToolSecret::new(API_KEY));
    let world = world_with(backend, vec![(info("websearch", "Web search"), endpoint)]).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);

    // the thread is created with the server attached; the agent calls its tool
    let (status, created) = chat
        .try_create_thread_with_tools(
            "plain",
            r#"tool websearch__echo {"text":"hello"}"#,
            &["websearch"],
        )
        .await;
    assert_eq!(status, 200, "{created}");
    let thread = created["threadId"].as_str().unwrap().to_owned();
    wait_for_jobs(&chat, &thread, 1).await;
    let events = chat.events(&thread).await;
    assert_eq!(
        reports(&events),
        [r#"tool websearch__echo: {"text":"hello"}"#]
    );

    // exactly one step: a start and an end of one id, with the server's icon, input and output
    let first = steps(&events);
    assert_eq!(first.len(), 2, "{first:?}");
    assert_eq!(first[0]["id"], first[1]["id"]);
    assert!(
        first[0]["id"].as_str().unwrap().starts_with("tool-"),
        "{}",
        first[0]["id"]
    );
    assert_eq!(
        (first[0]["phase"].clone(), first[0]["state"].clone()),
        (json!("start"), json!("running"))
    );
    assert_eq!(
        (first[1]["phase"].clone(), first[1]["state"].clone()),
        (json!("end"), json!("completed"))
    );
    assert_eq!(first[0]["icon"], "mcp-server:websearch");
    assert_eq!(first[0]["kind"], "tool");
    assert_eq!(first[0]["label"], "Web search \u{b7} echo");
    assert_eq!(first[0]["input"], json!({"text": "hello"}));
    assert_eq!(first[1]["output"]["text"], r#"{"text":"hello"}"#);
    let step_event = events.iter().find(|e| e["kind"] == "agent_step").unwrap();
    assert_eq!(step_event["actor"]["name"], "plain");

    // the upstream saw the bearer and the header, and the tool's own name; the agent's ids stay here
    let seen = server.seen();
    let called = seen.iter().find(|r| r.method == "tools/call").unwrap();
    assert_eq!(called.bearer.as_deref(), Some(BEARER));
    assert!(
        called
            .headers
            .iter()
            .any(|(n, v)| n == "x-api-key" && v == API_KEY),
        "{:?}",
        called.headers.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    assert_eq!(called.name.as_deref(), Some("echo"));
    assert_eq!(called.meta, None);
    assert!(seen.iter().filter(|r| r.method == "tools/call").count() == 1);

    // a second message: the tool fails upstream, which is a failed step and an error the agent reads
    run(&chat, &thread, "run-2", "tool websearch__fail {}").await;
    wait_for_jobs(&chat, &thread, 2).await;
    let events = chat.events(&thread).await;
    assert_eq!(
        reports(&events).last().unwrap(),
        "tool websearch__fail failed: the tool failed on purpose"
    );
    let ends: Vec<_> = steps(&events)
        .into_iter()
        .filter(|s| s["phase"] == "end")
        .collect();
    assert_eq!(ends.len(), 2);
    assert_eq!(ends[1]["state"], "failed");
    assert_eq!(ends[1]["detail"], "the tool failed on purpose");

    // a detached server is not on the endpoint: the agent is not offered the tool
    let (status, set) = chat.put_tools(&thread, &[]).await;
    assert_eq!(status, 200, "{set}");
    run(
        &chat,
        &thread,
        "run-3",
        r#"tool websearch__echo {"text":"again"}"#,
    )
    .await;
    wait_for_jobs(&chat, &thread, 3).await;
    let events = chat.events(&thread).await;
    assert_eq!(
        reports(&events).last().unwrap(),
        "tool websearch__echo not offered; offered=get_ui_catalog,turn_output"
    );
    assert_eq!(steps(&events).len(), 4, "the detached call made no step");
    // and a call the agent makes anyway, with the token of its message, is an unknown tool
    let grant = world.grants_of_plain().pop().unwrap();
    let client = connect(&grant).await;
    assert_eq!(
        call(&client, "websearch__echo", json!({}), json!({})).await,
        Err(-32602)
    );

    // no credential reaches anything the orchestrator keeps or says
    let viewer = chat
        .agui_connect(&thread, None, true)
        .await
        .collect_frames(WAIT)
        .await;
    let (_, export) = chat.get(&format!("/api/threads/{thread}/export")).await;
    let saw = format!(
        "{} {} {} {}",
        serde_json::to_string(&events).unwrap(),
        chat.thread(&thread).await,
        export,
        serde_json::to_string(&viewer.iter().map(|f| &f.event).collect::<Vec<_>>()).unwrap(),
    );
    let tokens: Vec<String> = world
        .grants_of_plain()
        .iter()
        .map(|g| g["token"].as_str().unwrap().to_owned())
        .collect();
    for secret in [BEARER, API_KEY, "Bearer "] {
        assert!(
            !saw.contains(secret),
            "{secret} reached what the chat reads"
        );
    }
    for needle in [BEARER, API_KEY]
        .into_iter()
        .map(str::to_owned)
        .chain(tokens)
        .chain([THREAD_TOOLS_KEY.to_owned()])
    {
        assert!(
            world.tables_mentioning(&needle).await.is_empty(),
            "a credential reached the database: {:?}",
            world.tables_mentioning(&needle).await
        );
    }
    if backend == Backend::Postgres {
        // the scan reads what it is meant to: the thread's id is in the tables
        assert!(!world.tables_mentioning(&thread).await.is_empty());
    }
}

backends!(the_agents_call_is_relayed_with_the_bearer_and_is_one_step_and_no_credential_is_kept);

// ---- the error table through the real client ---------------------------------------------------

async fn every_row_of_the_error_table_happens_through_the_real_client(backend: Backend) {
    let good = FakeToolServer::spawn(FakeToolServerOptions::default().bearer(BEARER)).await;
    let locked = FakeToolServer::spawn(FakeToolServerOptions::default().bearer(BEARER)).await;
    let closed = FakeToolServer::closed_url().await;
    let endpoint = |id: &str, url: String, secs: u64, bearer: &str| {
        ToolServerEndpoint::new(id, url, Duration::from_secs(secs))
            .with_bearer(ToolSecret::new(bearer))
    };
    let world = world_with(
        backend,
        vec![
            (
                info("good", "Good"),
                endpoint("good", good.url(), 5, BEARER),
            ),
            (
                info("quick", "Quick"),
                endpoint("quick", good.url(), 1, BEARER),
            ),
            (info("down", "Down"), endpoint("down", closed, 5, BEARER)),
            (
                info("locked", "Locked"),
                endpoint("locked", locked.url(), 5, "a-rotated-away-bearer-0000"),
            ),
        ],
    )
    .await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let (thread, grant) = working_thread(&world, &chat, &["good", "quick", "down", "locked"]).await;
    let client = connect(&grant).await;

    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    // `down` cannot be listed and is left out; the others are listed, in the deployment's order
    assert!(names.contains(&"good__echo".to_owned()), "{names:?}");
    assert!(names.contains(&"quick__slow".to_owned()), "{names:?}");
    assert!(!names.iter().any(|n| n.starts_with("down__")), "{names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with("locked__")),
        "{names:?}"
    );

    // unreachable
    let (is_error, text) = call(&client, "down__echo", json!({}), json!({}))
        .await
        .unwrap();
    assert!(is_error);
    assert_eq!(
        text,
        "the MCP server 'Down' could not be reached; the call did not run"
    );
    // a refused bearer
    let (is_error, text) = call(&client, "locked__echo", json!({}), json!({}))
        .await
        .unwrap();
    assert!(is_error);
    assert_eq!(
        text,
        "the MCP server refused the orchestrator's credentials"
    );
    // no answer in time
    let started = std::time::Instant::now();
    let (is_error, text) = call(&client, "quick__slow", json!({}), json!({}))
        .await
        .unwrap();
    assert!(is_error);
    assert_eq!(
        text,
        "no answer within 1 s; it may still be running on the server"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    // the tool says it failed
    let (is_error, text) = call(&client, "good__fail", json!({}), json!({}))
        .await
        .unwrap();
    assert!(is_error);
    assert_eq!(text, "the tool failed on purpose");
    // a result over 256 KiB is cut, with a note, and is not an error
    let (is_error, text) = call(
        &client,
        "good__big",
        json!({"bytes": 300 * 1024}),
        json!({}),
    )
    .await
    .unwrap();
    assert!(!is_error);
    let (body, note) = text.split_once('\n').unwrap();
    assert_eq!(body.len(), 256 * 1024);
    assert_eq!(note, "[the result is longer than 256 KiB and was cut here]");

    let events = chat.events(&thread).await;
    let ends: Vec<(String, String)> = steps(&events)
        .into_iter()
        .filter(|s| s["phase"] == "end")
        .map(|s| {
            (
                s["icon"].as_str().unwrap().to_owned(),
                s["state"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let want = [
        ("mcp-server:down", "failed"),
        ("mcp-server:locked", "failed"),
        ("mcp-server:quick", "failed"),
        ("mcp-server:good", "failed"),
        ("mcp-server:good", "completed"),
    ];
    assert_eq!(
        ends,
        want.map(|(a, b)| (a.to_owned(), b.to_owned())),
        "one step per call, each ended"
    );
    // the big result's step keeps 8 KiB and says it is not all of it
    let big = steps(&events)
        .into_iter()
        .rfind(|s| s["phase"] == "end")
        .unwrap();
    assert_eq!(big["output"]["truncated"], true);
    assert!(big["output"]["text"].as_str().unwrap().len() <= 8192);
    // the refused credentials are named nowhere
    let log = serde_json::to_string(&events).unwrap();
    assert!(!log.contains("a-rotated-away-bearer-0000") && !log.contains(BEARER));

    // the call that ends the thread's task: over, with no step
    assert_eq!(chat.cancel(&thread).await, 202);
    chat.wait_state(&thread, "cancelled").await;
    let before = steps(&chat.events(&thread).await).len();
    let (is_error, text) = call(&client, "good__echo", json!({}), json!({}))
        .await
        .unwrap();
    assert!(is_error);
    assert_eq!(text, "this task is over");
    assert_eq!(steps(&chat.events(&thread).await).len(), before);
    for needle in ["a-rotated-away-bearer-0000", BEARER] {
        assert!(world.tables_mentioning(needle).await.is_empty());
    }
}

backends!(every_row_of_the_error_table_happens_through_the_real_client);

// ---- a retried call, and a call the agent drops ----------------------------------------------

async fn a_retried_call_id_is_the_same_step_and_a_dropped_call_ends_canceled(backend: Backend) {
    let server = FakeToolServer::spawn(FakeToolServerOptions::default().bearer(BEARER)).await;
    let endpoint = ToolServerEndpoint::new("websearch", server.url(), Duration::from_secs(30))
        .with_bearer(ToolSecret::new(BEARER));
    let world = world_with(backend, vec![(info("websearch", "Web search"), endpoint)]).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let (thread, grant) = working_thread(&world, &chat, &["websearch"]).await;
    let client = connect(&grant).await;

    // the same `callId` twice (a retry after a lease expired): the same step, a call upstream each
    let meta = json!({"callId": "run-1:call-1"});
    for _ in 0..2 {
        let (is_error, _) = call(
            &client,
            "websearch__echo",
            json!({"text": "a"}),
            meta.clone(),
        )
        .await
        .unwrap();
        assert!(!is_error);
    }
    let events = chat.events(&thread).await;
    let ids: std::collections::BTreeSet<&str> = steps(&events)
        .iter()
        .map(|s| s["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["tool-run-1:call-1"].into());
    assert_eq!(
        server
            .seen()
            .iter()
            .filter(|r| r.method == "tools/call")
            .count(),
        2
    );

    // the agent gives up on a call that does not answer: its step ends `canceled`
    let pending = tokio::spawn({
        let grant = grant.clone();
        async move {
            let client = connect(&grant).await;
            let _ = call(
                &client,
                "websearch__slow",
                json!({}),
                json!({"callId": "run-1:call-2"}),
            )
            .await;
        }
    });
    orch_testsupport::eventually("the slow call's step is running", || async {
        steps(&chat.events(&thread).await)
            .iter()
            .any(|s| s["id"] == "tool-run-1:call-2")
            .then_some(())
    })
    .await;
    pending.abort();
    let _ = pending.await;
    orch_testsupport::eventually("the slow call's step is canceled", || async {
        steps(&chat.events(&thread).await)
            .iter()
            .any(|s| s["id"] == "tool-run-1:call-2" && s["phase"] == "end")
            .then_some(())
    })
    .await;
    let events = chat.events(&thread).await;
    let end = steps(&events)
        .into_iter()
        .rfind(|s| s["id"] == "tool-run-1:call-2")
        .unwrap();
    assert_eq!(end["state"], "canceled");
    assert_eq!(chat.cancel(&thread).await, 202);
    chat.wait_state(&thread, "cancelled").await;
}

backends!(a_retried_call_id_is_the_same_step_and_a_dropped_call_ends_canceled);
