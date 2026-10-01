//! The thread-tools endpoint end to end (`thread-tools/v1`, ADR 0023), on both stores: the real
//! router, the real dispatcher, the A2A adapter and the in-process fake agent, with an rmcp client
//! over streamable HTTP as the agent's side. Any replica serves any call: a token needs no process
//! that remembers it.
//!
//! The first group mints the tokens in the test, the way the A2A adapter does. The second runs the
//! whole loop: the real dispatcher and adapter mint a grant for a message, the fake agent's
//! `thread-tools` script calls the endpoint back with it, and the answer is what the thread's
//! screen last sent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use jiff::Timestamp;
use orch_core::{AgentId, Caller, THREAD_TOOLS_EXTENSION, ThreadId};
use orch_testsupport::{Chat, FakeAgentOptions, Frame, UI_CATALOG_ID, ui_catalog, with_ui_catalog};
use orch_thread_token::{Claims, ThreadToolsKeys, mint};
use reqwest::StatusCode;
use rmcp::model::CallToolRequestParams;
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{RoleClient, ServiceExt};
use secrecy::{ExposeSecret, SecretString};
use serde_json::{Value, json};
use uuid::Uuid;

const WAIT: Duration = Duration::from_secs(20);

type Client = RunningService<RoleClient, ()>;

/// One run of `message` under `run`, to its end: the frames.
async fn run(chat: &Chat, thread: &str, run: &str, message: &str, extra: Value) -> Vec<Frame> {
    let body = Chat::agui_input(thread, run, &[(&format!("m-{run}"), message)], extra);
    let mut sse = chat.agui_run("plain", &body).await;
    assert_eq!(sse.status, 200);
    sse.collect_frames(WAIT).await
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

fn claims(thread: &str, agent: &str) -> Claims {
    let issued_at = Timestamp::from_second(Timestamp::now().as_second()).unwrap();
    Claims {
        thread: thread.parse().unwrap(),
        job: 1,
        agent: AgentId::new(agent),
        caller: Caller::Main,
        depth: 0,
        message_id: "message-1".to_owned(),
        issued_at,
        expires_at: Timestamp::from_second(issued_at.as_second() + 7200).unwrap(),
    }
}

fn token_with(keys: &ThreadToolsKeys, claims: &Claims) -> String {
    mint(keys, claims).unwrap().expose_secret().to_owned()
}

/// The token of the thread's agent, as the adapter mints it.
fn token_for(thread: &str, agent: &str) -> String {
    token_with(&thread_tools_keys(), &claims(thread, agent))
}

fn url(instance: &orch_testsupport::TestInstance, thread: &str) -> String {
    format!("{}/thread-tools/{thread}/mcp", instance.base_url)
}

async fn connect(url: &str, token: &str) -> Client {
    let config = StreamableHttpClientTransportConfig::with_uri(url.to_owned()).auth_header(token);
    ().serve(StreamableHttpClientTransport::from_config(config))
        .await
        .expect("the MCP handshake")
}

/// `get_ui_catalog`; `(is_error, structured result)`.
async fn get_ui_catalog(client: &Client, args: Value) -> (bool, Value) {
    let Value::Object(args) = args else {
        panic!("arguments must be an object")
    };
    let result = client
        .call_tool(CallToolRequestParams::new("get_ui_catalog").with_arguments(args))
        .await
        .unwrap();
    (
        result.is_error == Some(true),
        result.structured_content.unwrap_or(Value::Null),
    )
}

async fn raw_list(url: &str, token: Option<&str>) -> reqwest::Response {
    let mut req = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    req.send().await.unwrap()
}

async fn assert_refused(url: &str, token: &str, what: &str) {
    let resp = raw_list(url, Some(token)).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{what}");
    assert_eq!(
        resp.headers()["www-authenticate"],
        r#"Bearer error="invalid_token""#,
        "{what}"
    );
}

async fn the_endpoint_gives_the_newest_catalog_and_any_replica_serves_it(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    // a second process that only serves the API: it shares nothing with the first but the database
    let replica = world.instance_with_thread_tools("orch-2", false).await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();

    // a thread whose screen sent no catalog: the tool says so, as an error to read
    run(&chat, &thread, "run-0", "echo none", json!({})).await;
    chat.wait_state(&thread, "done").await;
    let token = token_for(&thread, "plain");
    let client = connect(&url(&replica, &thread), &token).await;
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(
        tools.iter().map(|t| t.name.to_string()).collect::<Vec<_>>(),
        ["get_ui_catalog"]
    );
    let (is_error, _) = get_ui_catalog(&client, json!({})).await;
    assert!(is_error);

    // version 1 reaches the log through a run: the other process gives it, with the same token
    run(&chat, &thread, "run-1", "echo one", with_ui_catalog(1)).await;
    wait_for_jobs(&chat, &thread, 2).await;
    let (is_error, got) = get_ui_catalog(&client, json!({})).await;
    assert!(!is_error, "{got}");
    let v1 = ui_catalog(1);
    assert_eq!(got["version"], 1);
    assert_eq!(got["digest"], v1["digest"]);
    assert_eq!(got["catalog"], v1["catalog"]);
    assert_eq!(got["unchanged"], false);

    // version 2: the newest, from either process, and a known digest leaves the catalog out
    run(&chat, &thread, "run-2", "echo two", with_ui_catalog(2)).await;
    wait_for_jobs(&chat, &thread, 3).await;
    let v2 = ui_catalog(2);
    for instance in [&orch, &replica] {
        let client = connect(&url(instance, &thread), &token).await;
        let (_, got) = get_ui_catalog(&client, json!({})).await;
        assert_eq!(got["digest"], v2["digest"]);
        assert_eq!(got["catalog"], v2["catalog"]);
        let (_, got) = get_ui_catalog(&client, json!({"knownDigest": v2["digest"]})).await;
        assert_eq!(
            got,
            json!({
                "catalogId": v2["catalogId"], "version": 2, "digest": v2["digest"],
                "unchanged": true,
            })
        );
        let (_, got) = get_ui_catalog(&client, json!({"knownDigest": v1["digest"]})).await;
        assert_eq!(got["unchanged"], false);
        assert_eq!(got["version"], 2);
    }

    // the older screen comes back: the thread keeps the newest
    run(&chat, &thread, "run-3", "echo three", with_ui_catalog(1)).await;
    wait_for_jobs(&chat, &thread, 4).await;
    let (_, got) = get_ui_catalog(&client, json!({})).await;
    assert_eq!(got["version"], 2);
    assert_eq!(chat.thread(&thread).await["state"], "done");
}

async fn a_token_that_is_not_the_threads_is_refused_and_nothing_is_written(backend: Backend) {
    let world = World::start(backend).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let (a, b) = (Uuid::now_v7().to_string(), Uuid::now_v7().to_string());
    run(&chat, &a, "run-a", "echo a", with_ui_catalog(1)).await;
    run(&chat, &b, "run-b", "echo b", with_ui_catalog(2)).await;
    chat.wait_state(&a, "done").await;
    chat.wait_state(&b, "done").await;
    let (log_a, log_b) = (chat.events(&a).await, chat.events(&b).await);
    let url_a = url(&orch, &a);

    // the good token opens its own thread, and only that
    let client = connect(&url_a, &token_for(&a, "plain")).await;
    assert_eq!(get_ui_catalog(&client, json!({})).await.1["version"], 1);

    let keys = thread_tools_keys();
    let good = claims(&a, "plain");
    let now = Timestamp::now().as_second();
    let other_keys = ThreadToolsKeys::new(
        SecretString::from("an-unknown-key-0123456789abcdef0123456789abcdef".to_owned()),
        None,
    )
    .unwrap();
    for (what, token) in [
        // thread B's token on thread A's URL
        ("another thread's token", token_for(&b, "plain")),
        // the right thread, minted for another agent than the thread's
        ("another agent", token_for(&a, "coder")),
        // a bad signature: a key nobody holds
        ("an unknown key", token_with(&other_keys, &good)),
        ("garbage", "a.b.c".to_owned()),
        (
            "expired",
            token_with(
                &keys,
                &Claims {
                    issued_at: Timestamp::from_second(now - 7300).unwrap(),
                    expires_at: Timestamp::from_second(now - 100).unwrap(),
                    ..good.clone()
                },
            ),
        ),
        (
            "an ask nobody started",
            token_with(
                &keys,
                &Claims {
                    caller: Caller::Ask(1),
                    depth: 1,
                    ..good.clone()
                },
            ),
        ),
        (
            "a thread nobody created",
            token_with(
                &keys,
                &Claims {
                    thread: ThreadId(Uuid::now_v7()),
                    ..good.clone()
                },
            ),
        ),
    ] {
        assert_refused(&url_a, &token, what).await;
    }
    // no token at all
    let resp = raw_list(&url_a, None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(resp.headers()["www-authenticate"], "Bearer");

    // rotation: the previous key still opens it (a token minted before the key moved), and a
    // replica that has dropped the previous key would refuse it (tested in the surface's tests)
    let old =
        ThreadToolsKeys::new(SecretString::from(THREAD_TOOLS_OLD_KEY.to_owned()), None).unwrap();
    let client = connect(&url_a, &token_with(&old, &good)).await;
    assert_eq!(get_ui_catalog(&client, json!({})).await.1["version"], 1);

    // nothing was written by any of it
    assert_eq!(chat.events(&a).await, log_a);
    assert_eq!(chat.events(&b).await, log_b);
}

// ---- the whole loop: the adapter mints, the agent calls back ---------------------------------

/// A world whose adapter mints grants and whose `plain` agent lists `thread-tools/v1`.
async fn world_with_tools(backend: Backend, plain_lists_it: bool) -> World {
    World::with(
        backend,
        Setup {
            thread_tools: true,
            plain: FakeAgentOptions {
                extensions: if plain_lists_it {
                    vec![THREAD_TOOLS_EXTENSION.to_owned()]
                } else {
                    Vec::new()
                },
                ..FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await
}

/// What the agent said it got back from the endpoint in its `jobs`-th job (the `result` artifacts,
/// in order).
fn reports(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e["kind"] == "artifact" && e["data"]["name"] == "result")
        .map(|e| e["data"]["text"].as_str().unwrap().to_owned())
        .collect()
}

fn mentions(haystack: &str, needle: &str) -> bool {
    haystack.contains(needle)
}

async fn the_agent_calls_the_endpoint_back_with_the_grant_of_its_message_and_gets_the_current_catalog(
    backend: Backend,
) {
    let world = world_with_tools(backend, true).await;
    // the first instance that serves the thread tools is on the address the grants name
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let replica = world.instance_with_thread_tools("orch-2", false).await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    let (v1, v2) = (ui_catalog(1), ui_catalog(2));
    let said = |version: &Value| {
        format!(
            "thread-tools: tools=get_ui_catalog; catalog={UI_CATALOG_ID} v{} {}; again unchanged=true",
            version["version"],
            version["digest"].as_str().unwrap()
        )
    };

    // job 1: the screen sends version 1; the agent lists the tools and gets it
    let mut frames = run(
        &chat,
        &thread,
        "run-1",
        "thread-tools one",
        with_ui_catalog(1),
    )
    .await;
    wait_for_jobs(&chat, &thread, 1).await;
    // job 2: version 2 (a newer screen)
    frames.extend(
        run(
            &chat,
            &thread,
            "run-2",
            "thread-tools two",
            with_ui_catalog(2),
        )
        .await,
    );
    wait_for_jobs(&chat, &thread, 2).await;
    // job 3: the older screen is back: the thread keeps, and the agent gets, the newest
    frames.extend(
        run(
            &chat,
            &thread,
            "run-3",
            "thread-tools three",
            with_ui_catalog(1),
        )
        .await,
    );
    wait_for_jobs(&chat, &thread, 3).await;
    let events = chat.events(&thread).await;
    assert_eq!(
        reports(&events),
        [said(&v1), said(&v2), said(&v2)],
        "the agent saw the thread's current catalog each time"
    );

    // the agent was given one grant per message, each naming the thread's endpoint
    let grants = world.grants_of_plain();
    assert_eq!(grants.len(), 3, "{grants:?}");
    for grant in &grants {
        assert_eq!(
            grant["url"],
            format!("{}/thread-tools/{thread}/mcp", orch.base_url)
        );
    }
    let tokens: Vec<&str> = grants
        .iter()
        .map(|g| g["token"].as_str().unwrap())
        .collect();
    assert_eq!(
        tokens
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3,
        "one token per message"
    );

    // and the token is nowhere the orchestrator keeps or says anything: not in the log, the thread,
    // its export, any frame the chat saw, any row of the database
    let viewer = chat
        .agui_connect(&thread, None, true)
        .await
        .collect_frames(WAIT)
        .await;
    let (_, export) = chat.get(&format!("/api/threads/{thread}/export")).await;
    let saw = format!(
        "{} {} {} {} {}",
        serde_json::to_string(&events).unwrap(),
        chat.thread(&thread).await,
        export,
        serde_json::to_string(&frames.iter().map(|f| &f.event).collect::<Vec<_>>()).unwrap(),
        serde_json::to_string(&viewer.iter().map(|f| &f.event).collect::<Vec<_>>()).unwrap(),
    );
    for token in &tokens {
        assert!(
            !mentions(&saw, token),
            "a token reached what the chat reads"
        );
        assert!(
            world.tables_mentioning(token).await.is_empty(),
            "a token reached the database: {:?}",
            world.tables_mentioning(token).await
        );
    }
    assert!(world.tables_mentioning(THREAD_TOOLS_KEY).await.is_empty());
    if backend == Backend::Postgres {
        // the scan reads what it is meant to: the thread's id is in the tables
        assert!(!world.tables_mentioning(&thread).await.is_empty());
    }

    // The grant of the first message still opens the thread, on either process, and no other
    let url = grants[0]["url"].as_str().unwrap().to_owned();
    let token = tokens[0].to_owned();
    for base in [&orch.base_url, &replica.base_url] {
        let client = connect(&format!("{base}/thread-tools/{thread}/mcp"), &token).await;
        let (_, got) = get_ui_catalog(&client, json!({})).await;
        assert_eq!(got["version"], 2);
    }
    // another thread's URL: refused (the token is for this thread)
    let other = Uuid::now_v7().to_string();
    run(&chat, &other, "run-o", "echo other", json!({})).await;
    chat.wait_state(&other, "done").await;
    assert_refused(
        &url.replace(&thread, &other),
        &token,
        "another thread's URL",
    )
    .await;
    // a bad signature
    let mut forged = token.clone();
    let last = forged.pop().unwrap();
    forged.push(if last == 'A' { 'B' } else { 'A' });
    assert_refused(&url, &forged, "a changed signature").await;
    // an expired token (the lifetime is two hours: minted in the past, with the key)
    let now = Timestamp::now().as_second();
    let expired = Claims {
        issued_at: Timestamp::from_second(now - 7300).unwrap(),
        expires_at: Timestamp::from_second(now - 100).unwrap(),
        ..claims(&thread, "plain")
    };
    assert_refused(&url, &token_with(&thread_tools_keys(), &expired), "expired").await;
    // a token signed with the previous key of a rotation still opens it
    let old =
        ThreadToolsKeys::new(SecretString::from(THREAD_TOOLS_OLD_KEY.to_owned()), None).unwrap();
    let client = connect(&url, &token_with(&old, &claims(&thread, "plain"))).await;
    assert_eq!(get_ui_catalog(&client, json!({})).await.1["version"], 2);
}

async fn an_agent_that_does_not_list_the_extension_or_an_adapter_without_keys_gives_no_grant(
    backend: Backend,
) {
    // the card lists nothing: the agent is told nothing, and its script says it has no grant
    let world = world_with_tools(backend, false).await;
    let orch = world.instance_with_thread_tools("orch-1", true).await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    run(
        &chat,
        &thread,
        "run-1",
        "thread-tools one",
        with_ui_catalog(1),
    )
    .await;
    wait_for_jobs(&chat, &thread, 1).await;
    assert_eq!(
        reports(&chat.events(&thread).await),
        ["thread-tools: no grant"]
    );
    assert!(world.grants_of_plain().is_empty());
    assert!(
        world
            .plain
            .executions()
            .iter()
            .all(|call| !call.activates(THREAD_TOOLS_EXTENSION))
    );

    // the card lists it, but this deployment has no keys: nothing is minted either
    let world = World::with(
        backend,
        Setup {
            plain: FakeAgentOptions {
                extensions: vec![THREAD_TOOLS_EXTENSION.to_owned()],
                ..FakeAgentOptions::default()
            },
            ..Setup::default()
        },
    )
    .await;
    let orch = world.instance("orch-1").await;
    let chat = world.chat(&orch);
    let thread = Uuid::now_v7().to_string();
    run(&chat, &thread, "run-1", "thread-tools one", json!({})).await;
    wait_for_jobs(&chat, &thread, 1).await;
    assert_eq!(
        reports(&chat.events(&thread).await),
        ["thread-tools: no grant"]
    );
    assert!(world.grants_of_plain().is_empty());
}

backends!(
    the_endpoint_gives_the_newest_catalog_and_any_replica_serves_it,
    a_token_that_is_not_the_threads_is_refused_and_nothing_is_written,
    the_agent_calls_the_endpoint_back_with_the_grant_of_its_message_and_gets_the_current_catalog,
    an_agent_that_does_not_list_the_extension_or_an_adapter_without_keys_gives_no_grant,
);
