//! The relay of the thread-tools endpoint (`thread-tools/v1`, ADR 0024) over the in-memory stack
//! and `MemoryToolServers`: what `tools/list` offers (names, allow-list, the agent filter, `_meta`,
//! the icon), what a call does (the upstream sees the bearer, one step per call with its input and
//! output), and every row of the contract's error table.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_app::{AppConfig, ToolServerInfo};
use orch_core::{AgentId, AgentStepData, EventBody, StepPhase, StepState, ThreadId, UserId};
use orch_ports::memory::{MemoryToolServer, MemoryToolServers, ToolScript};
use orch_ports::{Ports, ThreadStore, ToolDef, ToolSecret, ToolServerEndpoint};
use orch_surface_thread_tools::RelayTools;
use rmcp::ServiceError;
use rmcp::model::{CallToolRequestParams, MetaObject, RequestMetaObject};
use serde_json::{Map, Value, json};
use support::*;

const BEARER: &str = "ws-bearer-secret-0001";
const ICON: &str = "data:image/svg+xml;base64,PHN2Zy8+";
const WEBSEARCH: &str = "http://websearch.test/mcp";
const DOCS: &str = "http://docs.test/mcp";
const REPOS: &str = "http://repos.test/mcp";

fn object(value: Value) -> Map<String, Value> {
    let Value::Object(map) = value else {
        panic!("an object")
    };
    map
}

fn def(name: &str, schema: Value) -> ToolDef {
    ToolDef {
        name: name.to_owned(),
        title: None,
        description: Some(format!("the tool {name}")),
        input_schema: object(schema),
        output_schema: None,
        annotations: None,
    }
}

/// The deployment's list: a web search with a bearer and an icon, a documentation server with an
/// allow-list, and a repositories server only the coder is offered.
fn servers() -> Vec<(ToolServerInfo, ToolServerEndpoint)> {
    let websearch = ToolServerInfo {
        description: Some("Search the web.".to_owned()),
        icon: Some(ICON.to_owned()),
        timeout: Duration::from_secs(2),
        ..ToolServerInfo::new("websearch", "Web search")
    };
    let docs = ToolServerInfo {
        tools: Some(vec!["echo".to_owned()]),
        ..ToolServerInfo::new("docs", "Documentation")
    };
    let repos = ToolServerInfo {
        agents: Some(vec![AgentId::new("coder")]),
        ..ToolServerInfo::new("repos", "Repositories")
    };
    vec![
        (
            websearch,
            ToolServerEndpoint::new("websearch", WEBSEARCH, Duration::from_secs(2))
                .with_bearer(ToolSecret::new(BEARER)),
        ),
        (
            docs,
            ToolServerEndpoint::new("docs", DOCS, Duration::from_secs(2)),
        ),
        (
            repos,
            ToolServerEndpoint::new("repos", REPOS, Duration::from_secs(2)),
        ),
    ]
}

fn upstream() -> MemoryToolServers {
    let memory = MemoryToolServers::new();
    memory.add(
        WEBSEARCH,
        MemoryToolServer::new()
            .with_standard_tools()
            .with_bearer(BEARER)
            .with_tool(def("_hidden", json!({"type": "object"})), ToolScript::Echo)
            .with_tool(
                def(&"x".repeat(80), json!({"type": "object"})),
                ToolScript::Echo,
            )
            .with_tool(def("typeless", json!({})), ToolScript::Echo)
            .with_tool(
                def("boom", json!({"type": "object"})),
                ToolScript::Raise {
                    code: -32000,
                    message: "the upstream blew up\nsecond line".to_owned(),
                },
            ),
    );
    memory.add(DOCS, MemoryToolServer::new().with_standard_tools());
    memory.add(REPOS, MemoryToolServer::new().with_standard_tools());
    memory
}

struct Relay {
    h: Harness,
    memory: MemoryToolServers,
}

/// The surface with the relay of `relayed`, on an application that offers `offered` for attaching.
async fn start(
    offered: &[(ToolServerInfo, ToolServerEndpoint)],
    relayed: Vec<(ToolServerInfo, ToolServerEndpoint)>,
) -> Relay {
    let memory = upstream();
    let cfg = AppConfig {
        tool_servers: offered.iter().map(|(info, _)| info.clone()).collect(),
        ..AppConfig::default()
    };
    let client = memory.clone();
    let h = Harness::start_full(keys(), cfg, |app, config| {
        config.with_provider(RelayTools::new(app.clone(), client, relayed).unwrap())
    })
    .await;
    Relay { h, memory }
}

async fn relay() -> Relay {
    start(&servers(), servers()).await
}

impl Relay {
    async fn attach(&self, thread: ThreadId, ids: &[&str]) {
        self.h
            .app
            .set_tools(
                &UserId::new(ALICE),
                thread,
                ids.iter().map(|s| (*s).to_owned()).collect(),
            )
            .await
            .unwrap();
    }

    async fn client(&self, thread: ThreadId, agent: &str) -> Client {
        connect(&self.h.url(thread), &grant_token(thread, agent)).await
    }

    async fn steps(&self, thread: ThreadId) -> Vec<AgentStepData> {
        steps_of(&self.h, thread).await
    }
}

async fn steps_of(h: &Harness, thread: ThreadId) -> Vec<AgentStepData> {
    h.events(thread)
        .await
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::AgentStep(data) => Some(data),
            _ => None,
        })
        .collect()
}

/// A call with the agent's `_meta[thread-tools/v1]`.
async fn call_with(
    client: &Client,
    tool: &str,
    args: Value,
    meta: Value,
) -> Result<Outcome, ServiceError> {
    let mut params = CallToolRequestParams::new(tool.to_owned()).with_arguments(object(args));
    params.meta = Some(RequestMetaObject(MetaObject(object(
        json!({"thread-tools/v1": meta}),
    ))));
    let result = client.call_tool(params).await?;
    Ok(Outcome {
        is_error: result.is_error == Some(true),
        value: result.structured_content.unwrap_or(Value::Null),
        text: result
            .content
            .iter()
            .filter_map(|c| match c {
                rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    })
}

fn protocol_code(error: &ServiceError) -> i32 {
    match error {
        ServiceError::McpError(data) => data.code.0,
        other => panic!("not a JSON-RPC error: {other}"),
    }
}

// ---- tools/list ----------------------------------------------------------------------------

#[tokio::test]
async fn the_attached_servers_tools_are_named_server_double_underscore_tool_with_meta_and_icon() {
    let r = relay().await;
    let thread = r.h.working_thread("plain").await;
    // nothing attached: the built-in tools only
    let client = r.client(thread, "plain").await;
    assert_eq!(tool_names(&client).await, ["get_ui_catalog", "turn_output"]);

    r.attach(thread, &["websearch"]).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(
        names,
        [
            "get_ui_catalog",
            "turn_output",
            "websearch__echo",
            "websearch__fail",
            "websearch__slow",
            "websearch__big",
            // `_hidden` (a leading `_`) and the 80-byte name are left out; no schema type is added
            "websearch__typeless",
            "websearch__boom",
        ]
    );
    let echo = tools.iter().find(|t| t.name == "websearch__echo").unwrap();
    assert_eq!(echo.title.as_deref(), Some("Web search: echo"));
    assert_eq!(echo.description.as_deref(), Some("Answers its arguments."));
    assert_eq!(
        serde_json::to_value(echo.meta.as_ref().unwrap()).unwrap(),
        json!({"thread-tools/v1": {"reportsStep": true, "timeoutSecs": 7}}),
        "the server's 2 s and the margin of 5"
    );
    let icons = echo.icons.as_ref().expect("the configured icon");
    assert_eq!(icons.len(), 1);
    assert_eq!(icons[0].src, ICON);
    let typeless = tools
        .iter()
        .find(|t| t.name == "websearch__typeless")
        .unwrap();
    assert_eq!(typeless.input_schema.get("type"), Some(&json!("object")));
    // the built-in tools say nothing of steps
    assert!(tools[0].meta.is_none());
}

#[tokio::test]
async fn the_allow_list_and_the_agent_filter_decide_what_is_offered() {
    // `docs` has an allow-list of `echo`
    let r = relay().await;
    let thread = r.h.working_thread("plain").await;
    r.attach(thread, &["docs", "websearch"]).await;
    let client = r.client(thread, "plain").await;
    let names = tool_names(&client).await;
    assert!(names.contains(&"docs__echo".to_owned()));
    assert!(!names.iter().any(|n| n == "docs__fail" || n == "docs__big"));
    // a tool of the allow-list is the only one that can be called
    let err = try_call(&client, "docs__fail", json!({}))
        .await
        .unwrap_err();
    assert_eq!(protocol_code(&err), -32602);
    assert!(
        r.memory.seen(DOCS).iter().all(|s| s.method == "tools/list"),
        "a call the allow-list refuses never reaches the server"
    );

    // `repos` is offered for the coder only: attached to the coder's thread it is listed, and not
    // to a thread of `plain` (the application refuses to attach it)
    let coder = r.h.working_thread("coder").await;
    r.attach(coder, &["repos"]).await;
    let names = tool_names(&r.client(coder, "coder").await).await;
    assert!(names.contains(&"repos__echo".to_owned()));
    let refused =
        r.h.app
            .set_tools(&UserId::new(ALICE), thread, vec!["repos".to_owned()])
            .await;
    assert!(refused.is_err());
}

#[tokio::test]
async fn the_relay_filters_by_agent_even_when_the_thread_has_the_server() {
    // the application let `docs` be attached for every agent; the relay's own list says the coder
    let offered = servers();
    let mut relayed = servers();
    relayed[1].0.agents = Some(vec![AgentId::new("coder")]);
    let r = start(&offered, relayed).await;
    let thread = r.h.working_thread("plain").await;
    r.attach(thread, &["docs"]).await;
    let client = r.client(thread, "plain").await;
    assert_eq!(tool_names(&client).await, ["get_ui_catalog", "turn_output"]);
    let err = try_call(&client, "docs__echo", json!({"text": "hi"}))
        .await
        .unwrap_err();
    assert_eq!(protocol_code(&err), -32602);
    assert!(r.steps(thread).await.is_empty());
}

#[tokio::test]
async fn a_server_that_cannot_be_listed_is_left_out_and_the_rest_is_listed() {
    let r = relay().await;
    let thread = r.h.working_thread("plain").await;
    r.attach(thread, &["websearch", "docs"]).await;
    r.memory.set_down(WEBSEARCH, true);
    let names = tool_names(&r.client(thread, "plain").await).await;
    assert_eq!(names, ["get_ui_catalog", "turn_output", "docs__echo"]);
}

// ---- tools/call ----------------------------------------------------------------------------

#[tokio::test]
async fn a_call_reaches_the_server_with_the_bearer_and_is_one_step_with_its_input_and_output() {
    let r = relay().await;
    let thread = r.h.working_thread("plain").await;
    r.attach(thread, &["websearch"]).await;
    let client = r.client(thread, "plain").await;
    let out = call(&client, "websearch__echo", json!({"text": "rust"})).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.value, json!({"text": "rust"}));

    // the upstream saw the bearer, the tool's own name and the arguments, and not the agent's meta
    let seen = r.memory.seen(WEBSEARCH);
    let call_seen = seen.iter().find(|s| s.method == "tools/call").unwrap();
    assert_eq!(call_seen.bearer.as_deref(), Some(BEARER));
    assert_eq!(call_seen.name.as_deref(), Some("echo"));
    assert_eq!(call_seen.arguments, Some(object(json!({"text": "rust"}))));
    assert_eq!(call_seen.meta, None);

    // exactly one step: its start and its end, the server's icon, input and output
    let steps = r.steps(thread).await;
    assert_eq!(steps.len(), 2, "{steps:?}");
    let (start, end) = (&steps[0], &steps[1]);
    assert_eq!(start.id, end.id);
    assert!(start.id.starts_with("tool-"));
    assert_eq!(
        (start.phase, start.state, end.phase, end.state),
        (
            StepPhase::Start,
            StepState::Running,
            StepPhase::End,
            StepState::Completed
        )
    );
    assert_eq!(start.icon.as_deref(), Some("mcp-server:websearch"));
    assert_eq!(end.icon.as_deref(), Some("mcp-server:websearch"));
    assert_eq!(start.label, "Web search \u{b7} echo");
    assert_eq!(start.input, Some(object(json!({"text": "rust"}))));
    assert_eq!(end.output.as_ref().unwrap().text, "{\"text\":\"rust\"}");
    assert!(start.path.is_empty());
    let events = r.h.events(thread).await;
    let step_event = events
        .iter()
        .find(|e| matches!(e.body, EventBody::AgentStep(_)))
        .unwrap();
    assert_eq!(
        step_event.actor.name, "plain",
        "attributed to the calling agent"
    );

    // the credential is in no event of the thread
    let log = serde_json::to_string(&events).unwrap();
    assert!(!log.contains(BEARER));
    assert!(!log.contains("Bearer"));
}

#[tokio::test]
async fn a_call_id_makes_the_steps_id_and_a_parent_step_nests_it() {
    let r = relay().await;
    let thread = r.h.working_thread("plain").await;
    r.attach(thread, &["websearch"]).await;
    let client = r.client(thread, "plain").await;
    let task =
        r.h.app
            .ports()
            .store()
            .get_binding(thread)
            .await
            .unwrap()
            .and_then(|b| b.task_id)
            .expect("the agent's task");
    let meta = json!({"callId": "run-1:call-7", "parentStepId": "acp:parent"});
    for _ in 0..2 {
        // a retry of the same call: made again upstream (at least once), the same step again
        let out = call_with(
            &client,
            "websearch__echo",
            json!({"text": "a"}),
            meta.clone(),
        )
        .await
        .unwrap();
        assert!(!out.is_error);
    }
    let steps = r.steps(thread).await;
    let ids: std::collections::BTreeSet<&str> = steps.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["tool-run-1:call-7"].into(), "{steps:?}");
    for step in &steps {
        assert_eq!(step.path, [format!("{task}/acp:parent")]);
    }
    let upstream_calls = r
        .memory
        .seen(WEBSEARCH)
        .iter()
        .filter(|s| s.method == "tools/call")
        .count();
    assert_eq!(
        upstream_calls, 2,
        "the call is at least once, not deduplicated"
    );

    // another call id is another step; a callId that is not usable is as good as none
    call_with(
        &client,
        "websearch__echo",
        json!({}),
        json!({"callId": "run-1:call-8"}),
    )
    .await
    .unwrap();
    call_with(
        &client,
        "websearch__echo",
        json!({}),
        json!({"callId": "x".repeat(300)}),
    )
    .await
    .unwrap();
    let ids: std::collections::BTreeSet<String> =
        r.steps(thread).await.into_iter().map(|s| s.id).collect();
    assert_eq!(ids.len(), 3, "{ids:?}");
}

#[tokio::test]
async fn an_unreachable_server_is_a_failed_step_and_an_error_to_read() {
    let r = relay().await;
    let thread = r.h.working_thread("plain").await;
    r.attach(thread, &["websearch"]).await;
    r.memory.set_down(WEBSEARCH, true);
    let out = call(
        &r.client(thread, "plain").await,
        "websearch__echo",
        json!({"text": "x"}),
    )
    .await;
    assert!(out.is_error);
    assert_eq!(
        out.text,
        "the MCP server 'Web search' could not be reached; the call did not run"
    );
    let steps = r.steps(thread).await;
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1].state, StepState::Failed);
    assert_eq!(
        steps[1].detail.as_deref(),
        Some("the MCP server 'Web search' could not be reached; the call did not run")
    );
}

#[tokio::test]
async fn a_server_that_does_not_answer_in_time_is_a_failed_step() {
    let r = start(&servers(), {
        let mut s = servers();
        s[0].1 = s[0].1.clone().with_timeout(Duration::from_secs(1));
        s
    })
    .await;
    let thread = r.h.working_thread("plain").await;
    r.attach(thread, &["websearch"]).await;
    let started = std::time::Instant::now();
    let out = call(
        &r.client(thread, "plain").await,
        "websearch__slow",
        json!({}),
    )
    .await;
    assert!(out.is_error);
    assert_eq!(
        out.text,
        "no answer within 1 s; it may still be running on the server"
    );
    assert!(started.elapsed() < Duration::from_secs(4));
    let steps = r.steps(thread).await;
    assert_eq!(steps.last().unwrap().state, StepState::Failed);
}

#[tokio::test]
async fn a_server_that_refuses_the_credentials_is_a_failed_step_that_names_no_credential() {
    let mut wrong = servers();
    wrong[0].1 = ToolServerEndpoint::new("websearch", WEBSEARCH, Duration::from_secs(2))
        .with_bearer(ToolSecret::new("a-rotated-away-bearer"));
    let r = start(&servers(), wrong).await;
    let thread = r.h.working_thread("plain").await;
    r.attach(thread, &["websearch"]).await;
    let out = call(
        &r.client(thread, "plain").await,
        "websearch__echo",
        json!({}),
    )
    .await;
    assert!(out.is_error);
    assert_eq!(
        out.text,
        "the MCP server refused the orchestrator's credentials"
    );
    let steps = r.steps(thread).await;
    assert_eq!(steps.last().unwrap().state, StepState::Failed);
    let log = serde_json::to_string(&r.h.events(thread).await).unwrap();
    assert!(!log.contains("a-rotated-away-bearer") && !log.contains(BEARER));
}

#[tokio::test]
async fn an_error_of_the_server_and_a_result_that_says_is_error_are_failed_steps() {
    let r = relay().await;
    let thread = r.h.working_thread("plain").await;
    r.attach(thread, &["websearch"]).await;
    let client = r.client(thread, "plain").await;

    // a JSON-RPC error: its message, to the agent and in the step
    let out = call(&client, "websearch__boom", json!({})).await;
    assert!(out.is_error);
    assert_eq!(out.text, "the upstream blew up\nsecond line");
    // the tool ran and failed: the result is passed through as it is
    let out = call(&client, "websearch__fail", json!({})).await;
    assert!(out.is_error);
    assert_eq!(out.text, "the tool failed on purpose");

    let ends: Vec<_> = r
        .steps(thread)
        .await
        .into_iter()
        .filter(|s| s.phase == StepPhase::End)
        .collect();
    assert_eq!(ends.len(), 2);
    assert!(ends.iter().all(|s| s.state == StepState::Failed));
    assert_eq!(ends[0].detail.as_deref(), Some("the upstream blew up"));
    assert_eq!(
        ends[1].detail.as_deref(),
        Some("the tool failed on purpose")
    );
    let output = ends[1].output.as_ref().unwrap();
    assert!(output.error);
    assert_eq!(output.text, "the tool failed on purpose");
}

#[tokio::test]
async fn a_result_over_256_kib_is_cut_with_a_note_and_the_step_is_done() {
    let r = relay().await;
    let thread = r.h.working_thread("plain").await;
    r.attach(thread, &["websearch"]).await;
    let out = call(
        &r.client(thread, "plain").await,
        "websearch__big",
        json!({"bytes": 300 * 1024}),
    )
    .await;
    assert!(!out.is_error);
    let (body, note) = out
        .text
        .split_once('\n')
        .expect("the note is a block of its own");
    assert_eq!(
        body.len(),
        256 * 1024,
        "the agent gets what the bound allows"
    );
    assert_eq!(note, "[the result is longer than 256 KiB and was cut here]");
    let end = r.steps(thread).await.pop().unwrap();
    assert_eq!(end.state, StepState::Completed);
    let output = end.output.unwrap();
    assert!(output.truncated, "the step says the text is not all of it");
    assert!(
        output.text.len() <= orch_core::STEP_OUTPUT_MAX_BYTES,
        "and keeps 8 KiB"
    );
}

#[tokio::test]
async fn a_call_the_agent_drops_ends_its_step_as_canceled() {
    let r = relay().await;
    let thread = r.h.working_thread("plain").await;
    r.attach(thread, &["websearch"]).await;
    let client = r.client(thread, "plain").await;
    let pending = tokio::spawn(async move {
        let _ = try_call(&client, "websearch__slow", json!({})).await;
    });
    orch_testsupport::eventually("the step is running", || async {
        (!steps_of(&r.h, thread).await.is_empty()).then_some(())
    })
    .await;
    pending.abort();
    let _ = pending.await;
    orch_testsupport::eventually("the step is canceled", || async {
        steps_of(&r.h, thread)
            .await
            .iter()
            .find(|s| s.phase == StepPhase::End)
            .map(|s| s.state)
    })
    .await;
    let steps = r.steps(thread).await;
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[1].state, StepState::Canceled);
    assert_eq!(steps[0].id, steps[1].id);
}

// ---- what is not on the endpoint, and a task that is over -----------------------------------

#[tokio::test]
async fn a_detached_or_unknown_name_is_invalid_params_and_writes_nothing() {
    let r = relay().await;
    let thread = r.h.working_thread("plain").await;
    let client = r.client(thread, "plain").await;
    // not attached yet
    r.attach(thread, &["websearch"]).await;
    assert!(
        !call(&client, "websearch__echo", json!({"text": "a"}))
            .await
            .is_error
    );
    let before = r.steps(thread).await.len();
    // detached: the same call is an unknown tool
    r.attach(thread, &[]).await;
    for name in [
        "websearch__echo",
        "websearch___hidden",
        "websearch__",
        "nobody__echo",
        "websearch",
        "docs__echo",
    ] {
        let err = try_call(&client, name, json!({})).await.unwrap_err();
        assert_eq!(protocol_code(&err), -32602, "{name}");
    }
    assert_eq!(r.steps(thread).await.len(), before);
    let calls = r
        .memory
        .seen(WEBSEARCH)
        .iter()
        .filter(|s| s.method == "tools/call")
        .count();
    assert_eq!(calls, 1, "nothing reached a server after the detach");
}

#[tokio::test]
async fn a_task_that_is_over_gets_an_error_and_no_step() {
    let r = relay().await;
    // a finished thread
    let thread = r.h.thread("plain").await;
    r.attach(thread, &["websearch"]).await;
    let out = call(
        &r.client(thread, "plain").await,
        "websearch__echo",
        json!({}),
    )
    .await;
    assert!(out.is_error);
    assert_eq!(out.text, "this task is over");
    assert!(r.steps(thread).await.is_empty());

    // a token of another job than the thread's current one
    let working = r.h.working_thread("plain").await;
    r.attach(working, &["websearch"]).await;
    let mut claims = claims(working, "plain");
    claims.job = 2;
    let client = connect(&r.h.url(working), &token(&keys(), &claims)).await;
    let out = call(&client, "websearch__echo", json!({})).await;
    assert!(out.is_error);
    assert_eq!(out.text, "this task is over");
    assert!(r.steps(working).await.is_empty());
    assert!(
        r.memory
            .seen(WEBSEARCH)
            .iter()
            .all(|s| s.method != "tools/call"),
        "no call reached a server"
    );
}

#[tokio::test]
async fn an_asked_agents_call_is_a_step_of_its_ask_and_its_tools_end_with_the_ask() {
    let r = relay().await;
    let thread = r.h.asking_thread("plain", "slow", &["coder"]).await;
    r.attach(thread, &["websearch"]).await;
    r.h.app
        .ask(orch_app::AskCall {
            thread,
            job: 1,
            caller: orch_core::Caller::Main,
            asker: AgentId::new("plain"),
            agent: AgentId::new("coder"),
            text: "slow work".to_owned(),
            call_id: Some("c1".to_owned()),
            parent_step: None,
            timeout: None,
        })
        .await
        .unwrap();
    let coder = connect(&r.h.url(thread), &ask_token(thread, 1, "coder", 1)).await;

    // the attached servers it may use, and none of the addressed agent's own tools
    let names = tool_names(&coder).await;
    assert!(names.contains(&"websearch__echo".to_owned()), "{names:?}");
    assert!(
        !names.contains(&"get_ui_catalog".to_owned()) && !names.contains(&"turn_output".to_owned()),
        "{names:?}"
    );

    // its call is a step of the ask, whatever step it says it runs under, and its own
    let out = call_with(
        &coder,
        "websearch__echo",
        json!({"text": "rust"}),
        json!({"callId": "ask-call-1", "parentStepId": "acp:somewhere"}),
    )
    .await
    .unwrap();
    assert!(!out.is_error, "{out:?}");
    let steps = r.steps(thread).await;
    assert_eq!(steps.len(), 2, "{steps:?}");
    assert_eq!(steps[0].id, "tool-ask-call-1");
    assert_eq!(steps[0].path, ["ask-1"], "{steps:?}");
    assert_eq!(steps[1].state, StepState::Completed);
    let events = r.h.events(thread).await;
    let step_event = events
        .iter()
        .find(|e| matches!(e.body, EventBody::AgentStep(_)))
        .unwrap();
    assert_eq!(step_event.actor.name, "coder", "the asked agent's call");
    assert_eq!(step_event.actor.revision, None);

    // the ask ends: the asked agent's task is over
    r.h.app
        .apply(
            thread,
            orch_core::Input::AskFinished {
                job: 1,
                ask: 1,
                revision: None,
                result: orch_core::AskResult::of(orch_core::AskOutcome::Completed),
            },
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let out = call(&coder, "websearch__echo", json!({"text": "again"})).await;
    assert!(out.is_error);
    assert_eq!(out.text, "this task is over");
    assert_eq!(
        r.steps(thread).await.len(),
        2,
        "no step for a task that is over"
    );
}

#[tokio::test]
async fn a_relay_is_refused_for_a_bad_id_a_duplicate_or_an_endpoint_of_another_id() {
    let memory = upstream();
    let h = Harness::start().await;
    let build = |servers: Vec<(ToolServerInfo, ToolServerEndpoint)>| {
        RelayTools::new(h.app.clone(), memory.clone(), servers).map(|_| ())
    };
    let endpoint = |id: &str| ToolServerEndpoint::new(id, DOCS, Duration::from_secs(1));
    let info = |id: &str| ToolServerInfo::new(id, "A server");
    assert_eq!(
        build(vec![(info("a_b"), endpoint("a_b"))]),
        Err(orch_surface_thread_tools::RelayError::BadServerId(
            "a_b".into()
        ))
    );
    assert_eq!(
        build(vec![(info("a"), endpoint("a")), (info("a"), endpoint("a"))]),
        Err(orch_surface_thread_tools::RelayError::DuplicateServer(
            "a".into()
        ))
    );
    assert_eq!(
        build(vec![(info("a"), endpoint("b"))]),
        Err(orch_surface_thread_tools::RelayError::EndpointMismatch(
            "a".into()
        ))
    );
    assert!(build(vec![(info("a"), endpoint("a"))]).is_ok());
}
