//! `ask_agent` on the thread-tools endpoint (ADR 0026) over the in-memory stack: the real router on
//! a real port, the real dispatcher sending to a scripted agent, and an rmcp client that is the
//! agent that asks. What the tool lists, what it answers, every refusal, the call key that makes a
//! repeat re-attach, the deadline, a client that goes away, and the person's cancel.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_app::{AppConfig, AskCall};
use orch_core::{
    AgentId, AskLimits, AskOutcome, Caller, EventBody, Input, StepKind, StepReport, StepState,
    ThreadId, Timer, UserId,
};
use orch_ports::memory::Call;
use orch_ports::{Ports as _, ThreadStore as _};
use orch_surface_thread_tools::AskTools;
use orch_testsupport::eventually;
use rmcp::model::{
    CallToolRequestParams, MetaObject, ProgressNotificationParam, RequestMetaObject,
};
use rmcp::service::{NotificationContext, RunningService};
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{ClientHandler, RoleClient, ServiceError, ServiceExt};
use serde_json::{Value, json};
use support::*;

const HEARTBEAT: Duration = Duration::from_millis(150);

async fn start_with(cfg: AppConfig) -> Harness {
    Harness::start_full(keys(), cfg, |app, config| {
        config.with_provider(AskTools::new(app.clone()).with_heartbeat(HEARTBEAT))
    })
    .await
}

async fn start() -> Harness {
    start_with(AppConfig::default()).await
}

/// `slow @coder @reviewer @browser` to `plain`: the agent holds its task, and the person mentioned
/// three agents.
async fn thread(h: &Harness) -> ThreadId {
    h.asking_thread("plain", "slow", &["coder", "reviewer", "browser"])
        .await
}

fn protocol_code(error: &ServiceError) -> i32 {
    match error {
        ServiceError::McpError(data) => data.code.0,
        other => panic!("not a JSON-RPC error: {other}"),
    }
}

fn alice() -> UserId {
    UserId::new(ALICE)
}

/// What a progress-recording client saw.
#[derive(Clone, Default)]
struct Progress {
    seen: Arc<Mutex<Vec<String>>>,
}

impl ClientHandler for Progress {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.seen
            .lock()
            .unwrap()
            .push(params.message.unwrap_or_default());
    }
}

impl Progress {
    fn all(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

/// A client of the endpoint that records its progress notifications.
async fn connect_recording(
    h: &Harness,
    thread: ThreadId,
    token: &str,
) -> (RunningService<RoleClient, Progress>, Progress) {
    let config = StreamableHttpClientTransportConfig::with_uri(h.url(thread)).auth_header(token);
    let progress = Progress::default();
    let client = progress
        .clone()
        .serve(StreamableHttpClientTransport::from_config(config))
        .await
        .expect("the MCP handshake");
    (client, progress)
}

fn params(agent: &str, message: &str, call_id: Option<&str>) -> CallToolRequestParams {
    let Value::Object(args) = json!({"agent": agent, "message": message}) else {
        unreachable!()
    };
    with_call_id(
        CallToolRequestParams::new("ask_agent").with_arguments(args),
        call_id,
    )
}

fn with_call_id(mut params: CallToolRequestParams, call_id: Option<&str>) -> CallToolRequestParams {
    if let Some(id) = call_id {
        let Value::Object(meta) = json!({"thread-tools/v1": {"callId": id}}) else {
            unreachable!()
        };
        params.meta = Some(RequestMetaObject(MetaObject(meta)));
    }
    params
}

/// One `ask_agent` call, to its result.
async fn ask(
    client: &RunningService<RoleClient, impl ClientHandler>,
    agent: &str,
    message: &str,
    call_id: Option<&str>,
) -> Outcome {
    try_ask(client, agent, message, call_id).await.unwrap()
}

async fn try_ask(
    client: &RunningService<RoleClient, impl ClientHandler>,
    agent: &str,
    message: &str,
    call_id: Option<&str>,
) -> Result<Outcome, ServiceError> {
    let result = client.call_tool(params(agent, message, call_id)).await?;
    Ok(outcome_of(result))
}

fn outcome_of(result: rmcp::model::CallToolResult) -> Outcome {
    let text = result
        .content
        .iter()
        .filter_map(|c| match c {
            rmcp::model::ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    Outcome {
        is_error: result.is_error == Some(true),
        value: result.structured_content.unwrap_or(Value::Null),
        text,
    }
}

fn started(events: &[orch_core::Event]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e.body, EventBody::AskStarted(_)))
        .count()
}

fn finished(events: &[orch_core::Event], ask: u32) -> Option<orch_core::AskFinishedData> {
    events.iter().find_map(|e| match &e.body {
        EventBody::AskFinished(d) if d.ask == ask => Some(d.clone()),
        _ => None,
    })
}

/// Waits for the ledger to hold `n` asks.
async fn asks_on_the_ledger(h: &Harness, thread: ThreadId, n: usize) {
    eventually(&format!("{n} asks on the ledger"), || async {
        let t = h.app.get_thread(&alice(), thread).await.unwrap();
        (t.job.asks.len() >= n).then_some(())
    })
    .await;
}

async fn ledger(h: &Harness, thread: ThreadId) -> Vec<orch_core::Ask> {
    h.app.get_thread(&alice(), thread).await.unwrap().job.asks
}

fn call_of(thread: ThreadId, caller: Caller, asker: &str, agent: &str, id: &str) -> AskCall {
    AskCall {
        thread,
        job: 1,
        caller,
        asker: AgentId::new(asker),
        agent: AgentId::new(agent),
        text: "slow work".to_owned(),
        call_id: Some(id.to_owned()),
        parent_step: None,
        timeout: None,
    }
}

// ---- what is listed ----------------------------------------------------------------------------

#[tokio::test]
async fn the_tool_is_listed_where_it_can_be_used_and_nowhere_else() {
    let h = start().await;
    let thread = thread(&h).await;

    // the addressed agent: the built-in tools and `ask_agent`, with the contract's definition
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<_> = tools.iter().map(|t| t.name.to_string()).collect();
    assert_eq!(names, ["get_ui_catalog", "turn_output", "ask_agent"]);
    let tool = tools.iter().find(|t| t.name == "ask_agent").unwrap();
    assert_eq!(tool.title.as_deref(), Some("Ask a mentioned agent"));
    assert!(
        tool.description
            .as_deref()
            .unwrap()
            .contains("Agents you can ask: browser, coder, reviewer."),
        "{:?}",
        tool.description
    );
    assert_eq!(
        tool.input_schema["required"],
        json!(["agent", "message"]),
        "{:?}",
        tool.input_schema
    );
    let meta = &tool.meta.as_ref().unwrap().0;
    assert_eq!(
        meta["thread-tools/v1"],
        json!({"reportsStep": true, "timeoutSecs": 1830})
    );
    let hints = tool.annotations.as_ref().unwrap();
    assert_eq!(
        (
            hints.read_only_hint,
            hints.destructive_hint,
            hints.idempotent_hint,
            hints.open_world_hint
        ),
        (Some(false), Some(false), Some(false), Some(true))
    );

    // an asked agent: `ask_agent` while its depth is below the limit, and none of the addressed
    // agent's own tools
    h.app
        .ask(call_of(thread, Caller::Main, "plain", "coder", "c1"))
        .await
        .unwrap();
    let coder = connect(&h.url(thread), &ask_token(thread, 1, "coder", 1)).await;
    assert_eq!(tool_names(&coder).await, ["ask_agent"]);
    h.app
        .ask(call_of(thread, Caller::Ask(1), "coder", "reviewer", "c2"))
        .await
        .unwrap();
    let reviewer = connect(&h.url(thread), &ask_token(thread, 2, "reviewer", 2)).await;
    assert!(
        tool_names(&reviewer).await.is_empty(),
        "depth 2 is the limit: it cannot ask"
    );
    // and what it is not offered is not on the endpoint
    let error = try_call(&coder, "get_ui_catalog", json!({}))
        .await
        .unwrap_err();
    assert_eq!(protocol_code(&error), -32602);
    let error = try_call(&coder, "turn_output", json!({"text": "x"}))
        .await
        .unwrap_err();
    assert_eq!(protocol_code(&error), -32602);

    // a thread whose job mentions nobody offers nothing to ask
    let alone = h.working_thread("plain").await;
    let client = connect(&h.url(alone), &grant_token(alone, "plain")).await;
    assert_eq!(tool_names(&client).await, ["get_ui_catalog", "turn_output"]);

    // and a thread that is over offers none
    h.app.cancel(&alice(), thread).await.unwrap();
    h.wait_state(thread, orch_core::ThreadState::Cancelled)
        .await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    assert!(!tool_names(&client).await.contains(&"ask_agent".to_owned()));
}

// ---- answering ---------------------------------------------------------------------------------

#[tokio::test]
async fn an_ask_waits_for_the_asked_agent_and_its_answer_is_the_result() {
    let h = start().await;
    let thread = thread(&h).await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;

    let out = ask(&client, "coder", "echo the plan", Some("call-1")).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.value["ask"], 1);
    assert_eq!(out.value["agent"], "coder");
    assert_eq!(out.value["state"], "completed");
    assert_eq!(out.value["text"], "echo: echo the plan");
    assert_eq!(
        serde_json::from_str::<Value>(&out.text).unwrap(),
        out.value,
        "the same JSON as text"
    );

    let events = h.events(thread).await;
    assert_eq!(
        (started(&events), finished(&events, 1).is_some()),
        (1, true)
    );
    let ask_started = events
        .iter()
        .find(|e| matches!(e.body, EventBody::AskStarted(_)))
        .unwrap();
    assert_eq!(ask_started.actor.name, "plain", "the asker's words");
    let EventBody::AskStarted(d) = &ask_started.body else {
        unreachable!()
    };
    assert_eq!(
        (d.agent.as_str(), d.by, d.depth, d.text.as_str()),
        ("coder", Caller::Main, 1, "echo the plan")
    );
    // the asked agent was asked once, in a context of its own, as `ask:1`
    let sends: Vec<_> = h
        .agent
        .sends()
        .into_iter()
        .filter_map(|c| match c {
            Call::Send {
                agent,
                context_id,
                thread_tools,
                ..
            } if agent.as_str() == "coder" => Some((context_id, thread_tools)),
            _ => None,
        })
        .collect();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].0, format!("{thread}-ask-coder"));
    let grant = sends[0].1.as_ref().unwrap();
    assert_eq!((grant.caller, grant.depth), (Caller::Ask(1), 1));
    // the thread's own work went on
    assert_eq!(
        h.app.get_thread(&alice(), thread).await.unwrap().state,
        orch_core::ThreadState::Working
    );
}

#[tokio::test]
async fn a_question_back_is_an_answer_and_the_next_ask_continues_the_task() {
    let h = start().await;
    let thread = thread(&h).await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    // the script `ask` says "Which branch?" and waits for the answer
    let first = ask(&client, "coder", "ask which branch", Some("c1")).await;
    assert!(!first.is_error, "{first:?}");
    assert_eq!(first.value["state"], "input_required");
    assert_eq!(first.value["question"], "Which branch?");
    let second = ask(&client, "coder", "main", Some("c2")).await;
    assert_eq!(second.value["ask"], 2);
    assert_eq!(second.value["state"], "completed", "{second:?}");
    let sends: Vec<_> = h
        .agent
        .sends()
        .into_iter()
        .filter_map(|c| match c {
            Call::Send { agent, task_id, .. } if agent.as_str() == "coder" => Some(task_id),
            _ => None,
        })
        .collect();
    assert_eq!(sends.len(), 2);
    assert!(
        sends[0].is_none() && sends[1].is_some(),
        "the same task: {sends:?}"
    );
}

#[tokio::test]
async fn a_failed_task_is_an_error_result_that_says_how_it_failed() {
    let h = start().await;
    let thread = thread(&h).await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    let out = ask(&client, "coder", "failed now", Some("c1")).await;
    assert!(out.is_error, "{out:?}");
    assert_eq!(out.value["state"], "failed");
    assert_eq!(out.value["error"], "scripted failure");
}

// ---- refusals ----------------------------------------------------------------------------------

#[tokio::test]
async fn every_refusal_is_a_result_that_says_why_and_writes_nothing() {
    let h = start_with(AppConfig {
        asks: AskLimits {
            per_job: 3,
            running: 2,
            ..AskLimits::default()
        },
        ..AppConfig::default()
    })
    .await;
    let thread = thread(&h).await;
    let main = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    let refused = |out: &Outcome, says: &str| {
        assert!(out.is_error, "{out:?}");
        assert!(
            out.value.is_null(),
            "a refusal has no structured result: {out:?}"
        );
        assert_eq!(out.text, says);
    };

    // not mentioned in this job
    refused(
        &ask(&main, "plain", "x", Some("n1")).await,
        "you can ask only the agents the person mentioned: browser, coder, reviewer",
    );
    refused(
        &ask(&main, "nobody", "x", Some("n2")).await,
        "you can ask only the agents the person mentioned: browser, coder, reviewer",
    );
    // a call key used for another ask
    let first = tokio::spawn({
        let peer = main.peer().clone();
        async move {
            peer.call_tool(params("coder", "slow one", Some("k1")))
                .await
        }
    });
    asks_on_the_ledger(&h, thread, 1).await;
    refused(
        &ask(&main, "reviewer", "slow one", Some("k1")).await,
        "this callId was used for another ask",
    );
    refused(
        &ask(&main, "coder", "slow other", Some("k1")).await,
        "this callId was used for another ask",
    );
    // depth: ask 1 (coder, depth 1) asks reviewer (depth 2); reviewer may not ask browser
    let coder = connect(&h.url(thread), &ask_token(thread, 1, "coder", 1)).await;
    let second = tokio::spawn({
        let peer = coder.peer().clone();
        async move {
            peer.call_tool(params("reviewer", "slow two", Some("k2")))
                .await
        }
    });
    asks_on_the_ledger(&h, thread, 2).await;
    let reviewer = connect(&h.url(thread), &ask_token(thread, 2, "reviewer", 2)).await;
    refused(
        &ask(&reviewer, "browser", "x", Some("k3")).await,
        "asks are nested at most 2 deep",
    );
    // a cycle: the asked agent asks the agent that asked it
    refused(
        &ask(&coder, "coder", "x", Some("k4")).await,
        "an agent cannot ask itself or an agent already in its chain",
    );
    // two asks run and the limit is two running
    refused(
        &ask(&main, "browser", "slow three", Some("k5")).await,
        "2 asks are already running; wait for one to finish",
    );
    // nothing refused was written
    let events = h.events(thread).await;
    assert_eq!(started(&events), 2, "{:?}", ledger(&h, thread).await);
    // a thread that is over: the task is over
    h.app.cancel(&alice(), thread).await.unwrap();
    h.wait_state(thread, orch_core::ThreadState::Cancelled)
        .await;
    for call in [first, second] {
        let out = outcome_of(call.await.unwrap().unwrap());
        assert_eq!(out.value["state"], "canceled", "{out:?}");
    }
    refused(
        &ask(&main, "coder", "late", Some("late-1")).await,
        "this task is over",
    );
}

#[tokio::test]
async fn the_jobs_ask_limit_and_a_finished_job_and_a_stale_token_are_refused() {
    let h = start_with(AppConfig {
        asks: AskLimits {
            per_job: 2,
            ..AskLimits::default()
        },
        ..AppConfig::default()
    })
    .await;
    let thread = thread(&h).await;
    let main = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    for n in 1..=2 {
        let out = ask(&main, "coder", "echo ok", Some(&format!("c{n}"))).await;
        assert!(!out.is_error, "{out:?}");
    }
    let out = ask(&main, "coder", "echo ok", Some("c3")).await;
    assert!(out.is_error);
    assert_eq!(out.text, "this job has used its 2 asks");
    // a token of another job, on a thread whose job is the first
    let other = token(
        &keys(),
        &orch_thread_token::Claims {
            job: 2,
            ..claims(thread, "plain")
        },
    );
    let stale = connect(&h.url(thread), &other).await;
    let out = ask(&stale, "coder", "echo ok", Some("c9")).await;
    assert!(out.is_error);
    assert_eq!(out.text, "this task is over");
    // and the finished job: the thread done
    let done = h.thread("plain").await;
    let client = connect(&h.url(done), &grant_token(done, "plain")).await;
    let out = ask(&client, "coder", "echo ok", Some("d1")).await;
    assert!(out.is_error);
    assert_eq!(out.text, "this task is over");
}

#[tokio::test]
async fn the_arguments_are_checked_as_the_schema_says() {
    let h = start().await;
    let thread = thread(&h).await;
    let main = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    for args in [
        json!({}),
        json!({"agent": "coder"}),
        json!({"agent": "coder", "message": ""}),
        json!({"agent": "coder", "message": "x", "timeout_secs": 5}),
        json!({"agent": "coder", "message": "x", "extra": 1}),
    ] {
        let error = try_call(&main, "ask_agent", args.clone())
            .await
            .unwrap_err();
        assert_eq!(protocol_code(&error), -32602, "{args}");
    }
    assert_eq!(started(&h.events(thread).await), 0);
}

// ---- calls that repeat, that are dropped, that run out ------------------------------------------

#[tokio::test]
async fn a_repeat_of_a_call_re_attaches_to_the_same_ask() {
    let h = start().await;
    let thread = thread(&h).await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    let first = tokio::spawn({
        let peer = client.peer().clone();
        async move {
            peer.call_tool(params("coder", "gate the plan", Some("dup")))
                .await
        }
    });
    asks_on_the_ledger(&h, thread, 1).await;
    // the same call again, on another connection, while the ask runs
    let again = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    let second = tokio::spawn({
        let peer = again.peer().clone();
        async move {
            peer.call_tool(params("coder", "gate the plan", Some("dup")))
                .await
        }
    });
    // both wait: the asked agent is held
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!first.is_finished() && !second.is_finished());
    assert_eq!(started(&h.events(thread).await), 1, "one ask, not two");
    h.agent.release_gate();
    let first = outcome_of(first.await.unwrap().unwrap());
    let second = outcome_of(second.await.unwrap().unwrap());
    for out in [&first, &second] {
        assert!(!out.is_error, "{out:?}");
        assert_eq!(out.value["ask"], 1);
        assert_eq!(out.value["state"], "completed");
    }
    assert_eq!(first.value, second.value);
    // and a call after the end is answered at once with the recorded result
    let late = tokio::time::timeout(
        Duration::from_secs(5),
        ask(&client, "coder", "gate the plan", Some("dup")),
    )
    .await
    .unwrap();
    assert_eq!(late.value, first.value);
    let events = h.events(thread).await;
    assert_eq!(started(&events), 1);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e.body, EventBody::AskFinished(_)))
            .count(),
        1
    );
    // the asked agent was asked once
    let asked = h
        .agent
        .sends()
        .into_iter()
        .filter(|c| matches!(c, Call::Send { agent, .. } if agent.as_str() == "coder"))
        .count();
    assert_eq!(asked, 1);
}

#[tokio::test]
async fn a_dropped_client_leaves_the_ask_running_and_a_second_call_re_attaches() {
    let h = start().await;
    let thread = thread(&h).await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    let waiting = tokio::spawn({
        let peer = client.peer().clone();
        async move {
            peer.call_tool(params("coder", "gate the plan", Some("keep")))
                .await
        }
    });
    asks_on_the_ledger(&h, thread, 1).await;
    // the client goes away: the call is dropped
    waiting.abort();
    client.cancel().await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    let asks = ledger(&h, thread).await;
    assert_eq!(asks.len(), 1);
    assert!(asks[0].is_running(), "the ask runs on: {asks:?}");
    assert!(finished(&h.events(thread).await, 1).is_none());
    // the agent calls again with the same callId, and waits for the same ask
    let again = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    let second = tokio::spawn({
        let peer = again.peer().clone();
        async move {
            peer.call_tool(params("coder", "gate the plan", Some("keep")))
                .await
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!second.is_finished());
    h.agent.release_gate();
    let out = outcome_of(second.await.unwrap().unwrap());
    assert!(!out.is_error, "{out:?}");
    assert_eq!(
        (out.value["ask"].clone(), out.value["state"].clone()),
        (json!(1), json!("completed"))
    );
    assert_eq!(started(&h.events(thread).await), 1);
}

#[tokio::test]
async fn the_deadline_ends_the_ask_timed_out_and_the_call_says_so() {
    let h = start().await;
    let thread = thread(&h).await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    let waiting = tokio::spawn({
        let peer = client.peer().clone();
        async move {
            peer.call_tool(params("coder", "slow work", Some("late")))
                .await
        }
    });
    asks_on_the_ledger(&h, thread, 1).await;
    eventually("the asked agent runs the task", || async {
        h.agent
            .sends()
            .iter()
            .any(|c| matches!(c, Call::Send { agent, .. } if agent.as_str() == "coder"))
            .then_some(())
    })
    .await;
    // the timer is the inbox worker's: it fires here
    h.app
        .apply(
            thread,
            Input::TimerFired(Timer::AskDeadline { job: 1, ask: 1 }),
            None,
            None,
            None,
        )
        .await
        .unwrap();
    let out = outcome_of(waiting.await.unwrap().unwrap());
    assert!(out.is_error, "{out:?}");
    assert_eq!(out.value["state"], "timed_out");
    assert_eq!(out.value["error"], "the asked agent did not answer in time");
    // the asked agent is told to stop
    eventually("the asked agent is cancelled", || async {
        h.agent
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Cancel { .. }))
            .then_some(())
    })
    .await;
    // the job is not the ask's to end
    assert_eq!(
        h.app.get_thread(&alice(), thread).await.unwrap().state,
        orch_core::ThreadState::Working
    );
    // a second call is answered with the same end
    let again = ask(&client, "coder", "slow work", Some("late")).await;
    assert_eq!(again.value["state"], "timed_out");
}

#[tokio::test]
async fn a_call_may_lower_the_ask_deadline() {
    let h = start().await;
    let thread = thread(&h).await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    let Value::Object(args) = json!({"agent": "coder", "message": "slow work", "timeout_secs": 60})
    else {
        unreachable!()
    };
    let waiting = tokio::spawn({
        let peer = client.peer().clone();
        async move {
            peer.call_tool(CallToolRequestParams::new("ask_agent").with_arguments(args))
                .await
        }
    });
    asks_on_the_ledger(&h, thread, 1).await;
    // the deadline is a timer of the inbox: one row, due in a minute and not in half an hour
    let key = orch_ports::NewTimer {
        id: orch_ports::InboxId(uuid::Uuid::nil()),
        after: jiff::SignedDuration::ZERO,
        timer: Timer::AskDeadline { job: 1, ask: 1 },
    }
    .idempotency_key(thread);
    let row = h
        .app
        .ports()
        .store()
        .find_inbox(orch_ports::TIMER_SOURCE, &key)
        .await
        .unwrap()
        .expect("the deadline is an inbox row");
    let due = row.available_at.as_second() - jiff::Timestamp::now().as_second();
    assert!((50..=61).contains(&due), "due in {due} s");
    h.app.cancel(&alice(), thread).await.unwrap();
    let out = outcome_of(waiting.await.unwrap().unwrap());
    assert_eq!(out.value["state"], "canceled");
}

// ---- the person's cancel -----------------------------------------------------------------------

#[tokio::test]
async fn the_persons_cancel_ends_every_ask_and_every_call_that_waits_for_one() {
    let h = start().await;
    let thread = thread(&h).await;
    let main = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    let first = tokio::spawn({
        let peer = main.peer().clone();
        async move { peer.call_tool(params("coder", "slow one", Some("a"))).await }
    });
    asks_on_the_ledger(&h, thread, 1).await;
    let coder = connect(&h.url(thread), &ask_token(thread, 1, "coder", 1)).await;
    let child = tokio::spawn({
        let peer = coder.peer().clone();
        async move {
            peer.call_tool(params("reviewer", "slow two", Some("b")))
                .await
        }
    });
    asks_on_the_ledger(&h, thread, 2).await;
    eventually("both asked agents run", || async {
        (h.agent
            .sends()
            .iter()
            .filter(|c| matches!(c, Call::Send { agent, .. } if agent.as_str() != "plain"))
            .count()
            == 2)
            .then_some(())
    })
    .await;

    h.app.cancel(&alice(), thread).await.unwrap();

    for (what, call) in [("the ask", first), ("the child", child)] {
        let out = outcome_of(call.await.unwrap().unwrap());
        assert!(out.is_error, "{what}: {out:?}");
        assert_eq!(out.value["state"], "canceled", "{what}: {out:?}");
    }
    let events = h.events(thread).await;
    for n in 1..=2 {
        assert_eq!(
            finished(&events, n).unwrap().state,
            AskOutcome::Canceled,
            "ask {n}"
        );
    }
    // both asked agents are told to stop
    eventually("both asked agents are cancelled", || async {
        (h.agent
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Cancel { .. }))
            .count()
            >= 3)
            .then_some(())
    })
    .await;
}

// ---- progress ----------------------------------------------------------------------------------

#[tokio::test]
async fn a_waiting_call_tells_a_client_that_wants_progress_what_goes_on_under_the_ask() {
    let h = start().await;
    let thread = thread(&h).await;
    let (client, progress) = connect_recording(&h, thread, &grant_token(thread, "plain")).await;
    let waiting = tokio::spawn({
        let peer = client.peer().clone();
        async move {
            peer.call_tool(params("coder", "gate the plan", Some("p1")))
                .await
        }
    });
    asks_on_the_ledger(&h, thread, 1).await;
    // a step under the ask: what an asked agent's relayed tool call is
    h.app
        .record_step(
            thread,
            orch_core::Actor::agent(&AgentId::new("coder"), None),
            StepReport {
                id: "tool-s1".to_owned(),
                parent: Some("ask-1".to_owned()),
                kind: StepKind::Tool,
                label: "Web search \u{b7} search".to_owned(),
                state: StepState::Running,
                icon: None,
                detail: None,
                input: None,
                output: None,
            },
            None,
        )
        .await
        .unwrap();
    eventually("the step and a heartbeat are reported", || async {
        let all = progress.all();
        (all.iter().any(|m| m == "coder: Web search \u{b7} search")
            && all.iter().any(|m| m == "waiting for coder"))
        .then_some(())
    })
    .await;
    h.agent.release_gate();
    let out = outcome_of(waiting.await.unwrap().unwrap());
    assert_eq!(out.value["state"], "completed");
}
