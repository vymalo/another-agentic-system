//! `child-sessions/v1` through the A2A adapter (ADR 0072, ADR 0008), against an in-process A2A agent
//! over real HTTP: the extension is activated (the header and the message's own `extensions`) only
//! for a call whose live card lists it well formed, on a send, a resubscribe, a `GetTask` and a
//! `CancelTask`; an activated stream says the sessions the parent lists (its first frame, its events
//! when the agent has them, a poll of the task while it runs and one read before the status that
//! ends it) and a round task says its own session; a call that did not activate it learns nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{
    AgentTaskState, AgentUpdate, CHILD_SESSIONS_EXTENSION, Classify as _, SessionEvent,
    SessionNews, SessionState,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, SendContent, SendRequest,
    TaskHandle,
};
use orch_testsupport::{FakeAgent, FakeAgentOptions};
use serde_json::{Value, json};

fn client_polling(poll: Option<Duration>) -> A2aAgentClient {
    let mut cfg = A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    };
    if let Some(poll) = poll {
        cfg.child_sessions_poll = poll;
    }
    A2aAgentClient::new(cfg).unwrap()
}

fn client() -> A2aAgentClient {
    client_polling(None)
}

fn offer() -> Value {
    json!({"agents": [{"name": "planner", "description": "Plans."}], "maxOpen": 4})
}

async fn agent(params: Option<Value>, events: bool) -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        child_sessions: params,
        child_session_events: events,
        ..FakeAgentOptions::default()
    })
    .await
}

fn request(ep: &AgentEndpoint, text: &str) -> SendRequest {
    SendRequest {
        endpoint: ep.clone(),
        message_id: format!("msg-{}", text.replace(' ', "-")),
        context_id: Some("ctx-1".to_owned()),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text(text.to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
        steer: false,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
    }
}

async fn drain(mut stream: AgentStream) -> Vec<AgentEnvelope> {
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
            Ok(Some(item)) => out.push(item.unwrap()),
            Ok(None) => return out,
            Err(_) => panic!("the stream did not end; got {out:?}"),
        }
    }
}

/// Header values as one list of URIs (a value may hold several, comma separated).
fn uris(values: &[String]) -> Vec<String> {
    values
        .iter()
        .flat_map(|v| v.split(','))
        .map(|u| u.trim().to_owned())
        .collect()
}

fn lists(values: &[String]) -> bool {
    uris(values).iter().any(|u| u == CHILD_SESSIONS_EXTENSION)
}

fn news(env: &AgentEnvelope) -> Option<&SessionNews> {
    match &env.update {
        Some(AgentUpdate::ChildSessions(news)) => Some(news),
        _ => None,
    }
}

fn is_completed(env: &AgentEnvelope) -> bool {
    matches!(
        env.update,
        Some(AgentUpdate::Status {
            state: AgentTaskState::Completed,
            ..
        })
    )
}

fn bad_cards() -> Vec<Value> {
    vec![
        json!({"agents": [], "maxOpen": 4}),
        json!({"agents": [{"name": "planner", "description": "Plans."}]}),
        json!({"agents": [{"name": "planner", "description": "Plans."}], "maxOpen": 0}),
        json!({"agents": [{"name": " ", "description": "Plans."}], "maxOpen": 4}),
        json!({"agents": "planner", "maxOpen": 4}),
        // more than a parent thread can note (MAX_CHILD_SESSIONS): no listing could pass the bound
        json!({"agents": [{"name": "planner", "description": "Plans."}], "maxOpen": 257}),
        json!(3),
    ]
}

#[tokio::test]
async fn a_send_activates_it_only_when_the_card_lists_it_well_formed() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    drain(client().send_stream(request(&ep, "echo hi")).await.unwrap()).await;
    let call = fake.executions().remove(0);
    assert!(call.activates_child_sessions(), "{call:?}");
    assert!(lists(&call.extensions_header));
    assert!(
        call.message_extensions
            .iter()
            .any(|e| e == CHILD_SESSIONS_EXTENSION)
    );

    // an agent that does not list it, and agents that list it badly, are asked for nothing
    let mut cases: Vec<Option<Value>> = vec![None];
    cases.extend(bad_cards().into_iter().map(Some));
    for params in cases {
        let fake = agent(params.clone(), false).await;
        let ep = fake.endpoint("coder", None);
        drain(client().send_stream(request(&ep, "echo hi")).await.unwrap()).await;
        let call = fake.executions().remove(0);
        assert!(!lists(&call.extensions_header), "{params:?}: {call:?}");
        assert!(
            !call
                .message_extensions
                .iter()
                .any(|e| e == CHILD_SESSIONS_EXTENSION),
            "{params:?}"
        );
    }
}

#[tokio::test]
async fn the_card_of_this_very_call_decides() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client();
    drain(client.send_stream(request(&ep, "echo one")).await.unwrap()).await;
    fake.set_child_sessions(None);
    drain(client.send_stream(request(&ep, "echo two")).await.unwrap()).await;
    fake.set_child_sessions(Some(offer()));
    drain(
        client
            .send_stream(request(&ep, "echo three"))
            .await
            .unwrap(),
    )
    .await;
    let calls = fake.executions();
    let activated: Vec<bool> = calls.iter().map(|c| c.activates_child_sessions()).collect();
    assert_eq!(activated, [true, false, true]);
}

#[tokio::test]
async fn a_steer_activates_it_when_the_card_lists_it() {
    // the contract asks for activation on every request about a child context
    for (params, want) in [(Some(offer()), true), (None, false)] {
        let fake = FakeAgent::spawn(FakeAgentOptions {
            child_sessions: params,
            extensions: vec![orch_core::STEER_EXTENSION.to_owned()],
            ..FakeAgentOptions::default()
        })
        .await;
        let ep = fake.endpoint("coder", None);
        let client = client();
        let mut first = client
            .send_stream(request(&ep, "steerable work"))
            .await
            .unwrap();
        let task = first.next().await.unwrap().unwrap().task_id;
        let mut steer = request(&ep, "and this");
        steer.task_id = Some(task);
        steer.steer = true;
        steer.message_id = "msg-steer".into();
        drop(client.send_stream(steer).await.unwrap());
        let steers: Vec<_> = fake
            .calls()
            .into_iter()
            .filter(|c| c.kind == orch_testsupport::CallKind::Steer)
            .collect();
        assert_eq!(steers.len(), 1);
        assert_eq!(lists(&steers[0].extensions_header), want, "{steers:?}");
        assert_eq!(
            steers[0]
                .message_extensions
                .iter()
                .any(|e| e == CHILD_SESSIONS_EXTENSION),
            want
        );
        fake.release_gate();
    }
}

#[tokio::test]
async fn a_resubscribe_a_get_and_a_cancel_activate_it_when_the_card_lists_it() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client();
    // a task that runs until it is cancelled, so that it can be read and subscribed to
    let mut first = client.send_stream(request(&ep, "slow work")).await.unwrap();
    let task = first.next().await.unwrap().unwrap().task_id;
    let handle = TaskHandle {
        endpoint: ep.clone(),
        task_id: task,
    };
    drop(client.resubscribe(&handle).await.unwrap());
    client.get_task(&handle).await.unwrap();
    let subscribed = fake.subscription_extensions();
    assert!(lists(&subscribed[0]), "{subscribed:?}");
    let got = fake.rpc_extensions("get_task");
    assert!(lists(got.last().unwrap()), "{got:?}");

    // the card is read for each call: withdrawn, nothing is asked for
    fake.set_child_sessions(None);
    drop(client.resubscribe(&handle).await.unwrap());
    client.get_task(&handle).await.unwrap();
    assert!(!lists(&fake.subscription_extensions()[1]));
    assert!(!lists(fake.rpc_extensions("get_task").last().unwrap()));
    // and listed badly it is not listed
    for params in bad_cards() {
        fake.set_child_sessions(Some(params));
        client.get_task(&handle).await.unwrap();
        assert!(!lists(fake.rpc_extensions("get_task").last().unwrap()));
    }

    fake.set_child_sessions(Some(offer()));
    client.cancel(&handle).await.unwrap();
    let cancels = fake.rpc_extensions("cancel_task");
    assert!(lists(cancels.last().unwrap()), "{cancels:?}");
}

#[tokio::test]
async fn a_cancel_of_a_card_that_lists_it_badly_activates_nothing() {
    let fake = agent(Some(json!({"agents": [], "maxOpen": 2})), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client();
    let mut first = client.send_stream(request(&ep, "slow work")).await.unwrap();
    let task = first.next().await.unwrap().unwrap().task_id;
    let handle = TaskHandle {
        endpoint: ep,
        task_id: task,
    };
    client.cancel(&handle).await.unwrap();
    assert!(!lists(fake.rpc_extensions("cancel_task").last().unwrap()));
}

fn session_of(env: &AgentEnvelope) -> Option<&orch_core::SessionReport> {
    match news(env)? {
        SessionNews::Listed(l) => l.first(),
        SessionNews::Event(_, r) | SessionNews::OfRound(r) => Some(r),
    }
}

#[tokio::test]
async fn a_parent_says_its_sessions_first_and_once_more_before_the_status_that_ends_it() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "child-sessions planner keep"))
            .await
            .unwrap(),
    )
    .await;
    let task = &envs[0].task_id;
    let session = format!("{task}-s1");
    let sessions: Vec<(usize, &AgentEnvelope)> = envs
        .iter()
        .enumerate()
        .filter(|(_, e)| news(e).is_some())
        .collect();
    assert!(sessions.len() >= 2, "{envs:?}");
    // the first frame lists the session working, its round the task named like the session
    let first = session_of(sessions[0].1).unwrap();
    assert_eq!(first.session, session);
    assert_eq!(first.context_id, format!("ctx-1/s/{session}"));
    assert_eq!(first.agent, "planner");
    assert_eq!(first.description, format!("planner for {task}"));
    assert!(first.keep_open);
    assert_eq!(first.state, SessionState::Working);
    let round = first.round.as_ref().unwrap();
    assert_eq!(
        (round.number, round.task_id.as_deref()),
        (1, Some(session.as_str()))
    );
    // the read before the end says it idle with the round completed, and nothing follows it
    let (at, last) = *sessions.last().unwrap();
    let last = session_of(last).unwrap();
    assert_eq!(last.state, SessionState::Idle);
    assert_eq!(
        last.round.as_ref().unwrap().state,
        Some(AgentTaskState::Completed)
    );
    let ending = envs.iter().position(is_completed).unwrap();
    assert!(
        at < ending,
        "the sessions come before the status that ends the task: {envs:?}"
    );
    assert!(
        envs[at + 1..ending].iter().all(|e| news(e).is_none()),
        "{envs:?}"
    );
    assert_eq!(ending, envs.len() - 1);
}

#[tokio::test]
async fn a_parent_that_was_not_activated_says_nothing_of_sessions() {
    for params in std::iter::once(None).chain(bad_cards().into_iter().map(Some)) {
        let fake = agent(params.clone(), true).await;
        let ep = fake.endpoint("coder", None);
        let envs = drain(
            client()
                .send_stream(request(&ep, "child-sessions planner"))
                .await
                .unwrap(),
        )
        .await;
        assert!(
            envs.iter().all(|e| news(e).is_none()),
            "{params:?}: {envs:?}"
        );
        assert!(envs.iter().any(is_completed), "{params:?}");
    }
}

#[tokio::test]
async fn the_events_of_an_agent_that_has_them_are_said_in_order() {
    let fake = agent(Some(offer()), true).await;
    let ep = fake.endpoint("coder", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "child-sessions planner"))
            .await
            .unwrap(),
    )
    .await;
    let events: Vec<(SessionEvent, SessionState)> = envs
        .iter()
        .filter_map(|e| match news(e) {
            Some(SessionNews::Event(kind, r)) => Some((*kind, r.state)),
            _ => None,
        })
        .collect();
    assert_eq!(
        events,
        [
            (SessionEvent::Opened, SessionState::Working),
            (SessionEvent::Reported, SessionState::Closed),
            (SessionEvent::Closed, SessionState::Closed),
        ]
    );
}

#[tokio::test]
async fn a_malformed_listing_is_rejected_and_the_task_goes_on() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "child-sessions-bad"))
            .await
            .unwrap(),
    )
    .await;
    let rejected = envs
        .iter()
        .filter(|e| matches!(e.update, Some(AgentUpdate::ChildSessionsRejected(_))))
        .count();
    assert!(rejected >= 1, "{envs:?}");
    assert!(envs.iter().all(|e| news(e).is_none()));
    assert!(envs.iter().any(is_completed));
}

async fn wait_for_gets(fake: &FakeAgent, at_least: usize) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while fake.rpc_count("get_task") < at_least {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the task was not polled");
}

type Seen = std::sync::Arc<std::sync::Mutex<Vec<AgentEnvelope>>>;

/// Reads a stream on a task of its own, as the dispatcher does, keeping what it says: a stream is
/// read only as fast as it is asked.
fn read_on_a_task(mut stream: AgentStream) -> (Seen, tokio::task::JoinHandle<()>) {
    let seen: Seen = std::sync::Arc::default();
    let into = std::sync::Arc::clone(&seen);
    let handle = tokio::spawn(async move {
        while let Some(item) = stream.next().await {
            into.lock().unwrap().push(item.unwrap());
        }
    });
    (seen, handle)
}

#[tokio::test]
async fn the_task_is_polled_while_it_runs_and_a_repeat_is_not_passed_on() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client_polling(Some(Duration::from_millis(40)));
    let stream = client
        .send_stream(request(&ep, "child-sessions planner keep gate hold"))
        .await
        .unwrap();
    let (seen, reading) = read_on_a_task(stream);
    // the task is read again and again while the round works, and none of it is news
    wait_for_gets(&fake, 3).await;
    {
        let seen = seen.lock().unwrap();
        let said: Vec<_> = seen
            .iter()
            .filter_map(session_of)
            .map(|r| r.state)
            .collect();
        assert_eq!(
            said,
            [SessionState::Working],
            "the first frame only: {said:?}"
        );
    }
    fake.release_gate();
    tokio::time::timeout(Duration::from_secs(10), reading)
        .await
        .unwrap()
        .unwrap();
    let seen = seen.lock().unwrap();
    let said: Vec<SessionState> = seen
        .iter()
        .filter_map(session_of)
        .map(|r| r.state)
        .collect();
    // what a poll learned (the session idle, while the agent finishes its answer) is passed on
    // once, and the read before the end repeats it under the same key
    assert_eq!(said.first(), Some(&SessionState::Working));
    assert!(
        said[1..].iter().all(|s| *s == SessionState::Idle),
        "{said:?}"
    );
    let idle_keys: Vec<_> = seen
        .iter()
        .filter(|e| session_of(e).is_some_and(|r| r.state == SessionState::Idle))
        .map(|e| e.key.clone())
        .collect();
    assert!(
        !idle_keys.is_empty() && idle_keys.windows(2).all(|w| w[0] == w[1]),
        "{idle_keys:?}"
    );
    assert!(is_completed(seen.last().unwrap()));
}

#[tokio::test]
async fn a_stream_that_has_events_is_not_polled_but_is_read_once_before_its_end() {
    let fake = agent(Some(offer()), true).await;
    let ep = fake.endpoint("coder", None);
    let client = client_polling(Some(Duration::from_millis(40)));
    let stream = client
        .send_stream(request(&ep, "child-sessions planner keep gate hold"))
        .await
        .unwrap();
    let (seen, reading) = read_on_a_task(stream);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !seen
            .lock()
            .unwrap()
            .iter()
            .any(|e| matches!(news(e), Some(SessionNews::Event(SessionEvent::Opened, _))))
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let before = fake.rpc_count("get_task");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fake.rpc_count("get_task"),
        before,
        "the agent sends events: no periodic read"
    );
    fake.release_gate();
    tokio::time::timeout(Duration::from_secs(10), reading)
        .await
        .unwrap()
        .unwrap();
    assert!(is_completed(seen.lock().unwrap().last().unwrap()));
    assert_eq!(
        fake.rpc_count("get_task"),
        before + 1,
        "the one read before the end"
    );
}

#[tokio::test]
async fn a_card_withdrawn_while_the_stream_runs_stops_the_poll() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client_polling(Some(Duration::from_millis(40)));
    let stream = client
        .send_stream(request(&ep, "child-sessions planner keep gate hold"))
        .await
        .unwrap();
    let (seen, reading) = read_on_a_task(stream);
    wait_for_gets(&fake, 2).await;
    fake.set_child_sessions(None);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let settled = fake.rpc_count("get_task");
    let said_before = seen
        .lock()
        .unwrap()
        .iter()
        .filter(|e| news(e).is_some())
        .count();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fake.rpc_count("get_task"),
        settled,
        "the poll went on after the card dropped it"
    );
    fake.release_gate();
    tokio::time::timeout(Duration::from_secs(10), reading)
        .await
        .unwrap()
        .unwrap();
    let seen = seen.lock().unwrap();
    let said_after = seen.iter().filter(|e| news(e).is_some()).count();
    assert_eq!(
        said_after, said_before,
        "nothing is said of sessions once the card withdrew them"
    );
    assert_eq!(
        fake.rpc_count("get_task"),
        settled,
        "not even the read before the end"
    );
    assert!(is_completed(seen.last().unwrap()));
}

#[tokio::test]
async fn a_card_that_comes_back_does_not_start_the_poll_again() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client_polling(Some(Duration::from_millis(40)));
    let stream = client
        .send_stream(request(&ep, "child-sessions planner keep gate hold"))
        .await
        .unwrap();
    let (seen, reading) = read_on_a_task(stream);
    wait_for_gets(&fake, 2).await;
    fake.set_child_sessions(None);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let settled = fake.rpc_count("get_task");
    fake.set_child_sessions(Some(offer()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fake.rpc_count("get_task"),
        settled,
        "a poll that stopped for a withdrawn card stays stopped"
    );
    fake.release_gate();
    tokio::time::timeout(Duration::from_secs(10), reading)
        .await
        .unwrap()
        .unwrap();
    assert!(is_completed(seen.lock().unwrap().last().unwrap()));
}

#[tokio::test]
async fn the_read_before_the_end_says_the_listing_again_even_when_a_poll_just_said_it() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client_polling(Some(Duration::from_millis(40)));
    let stream = client
        .send_stream(request(&ep, "child-sessions planner keep hold"))
        .await
        .unwrap();
    let (seen, reading) = read_on_a_task(stream);
    tokio::time::timeout(Duration::from_secs(10), async {
        while !seen
            .lock()
            .unwrap()
            .iter()
            .any(|e| session_of(e).is_some_and(|r| r.state == SessionState::Idle))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a poll learned the session idle");
    tokio::time::timeout(Duration::from_secs(10), reading)
        .await
        .unwrap()
        .unwrap();
    let seen = seen.lock().unwrap();
    let idle: Vec<usize> = seen
        .iter()
        .enumerate()
        .filter(|(_, e)| session_of(e).is_some_and(|r| r.state == SessionState::Idle))
        .map(|(i, _)| i)
        .collect();
    assert!(
        idle.len() >= 2,
        "the poll's and the forced read's: {seen:?}"
    );
    let last = *idle.last().unwrap();
    assert_eq!(seen[idle[0]].key, seen[last].key);
    assert!(is_completed(&seen[last + 1]), "{seen:?}");
}

#[tokio::test]
async fn news_nobody_asked_for_is_dropped_on_every_road() {
    // an agent that tells its sessions to a call that did not activate the extension
    for params in [None, Some(json!({"agents": [], "maxOpen": 2}))] {
        let fake = FakeAgent::spawn(FakeAgentOptions {
            child_sessions: params,
            child_session_events: true,
            child_sessions_leak: true,
            ..FakeAgentOptions::default()
        })
        .await;
        let ep = fake.endpoint("coder", None);
        let client = client();
        let envs = drain(
            client
                .send_stream(request(&ep, "child-sessions planner keep"))
                .await
                .unwrap(),
        )
        .await;
        assert!(
            envs.iter().all(|e| {
                !matches!(
                    e.update,
                    Some(AgentUpdate::ChildSessions(_) | AgentUpdate::ChildSessionsRejected(_))
                )
            }),
            "the stream: {envs:?}"
        );
        assert!(envs.iter().any(is_completed));
        // a read of the task, which the agent still tells the sessions on
        let handle = TaskHandle {
            endpoint: ep.clone(),
            task_id: envs[0].task_id.clone(),
        };
        let polled = client.get_task(&handle).await.unwrap();
        assert!(
            polled.envelopes.iter().all(|e| news(e).is_none()
                && !matches!(e.update, Some(AgentUpdate::ChildSessionsRejected(_)))),
            "get_task: {polled:?}"
        );
        // and a cancel
        let mut slow = client.send_stream(request(&ep, "slow work")).await.unwrap();
        let slow = slow.next().await.unwrap().unwrap().task_id;
        let canceled = client
            .cancel(&TaskHandle {
                endpoint: ep.clone(),
                task_id: slow,
            })
            .await
            .unwrap();
        assert!(
            canceled.envelopes.iter().all(|e| news(e).is_none()),
            "cancel: {canceled:?}"
        );
        // a malformed listing, told to a call that did not ask, is not even counted
        let bad = drain(
            client
                .send_stream(request(&ep, "child-sessions-bad"))
                .await
                .unwrap(),
        )
        .await;
        assert!(
            bad.iter()
                .all(|e| !matches!(e.update, Some(AgentUpdate::ChildSessionsRejected(_)))),
            "{bad:?}"
        );
    }
}

#[tokio::test]
async fn a_list_of_a_child_context_is_activated_like_a_get() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client();
    let parent = drain(
        client
            .send_stream(request(&ep, "child-sessions planner keep"))
            .await
            .unwrap(),
    )
    .await;
    let session = format!("{}-s1", parent[0].task_id);
    let child = format!("ctx-1/s/{session}");
    assert_eq!(
        client
            .find_task_by_message(&ep, Some(&child), "m-x")
            .await
            .unwrap(),
        None
    );
    let lists_seen = fake.rpc_extensions("list_tasks");
    assert!(lists(lists_seen.last().unwrap()), "{lists_seen:?}");
    // the card withdrew it: the agent refuses a child context, and so the adapter says it
    fake.set_child_sessions(None);
    let refused = client
        .find_task_by_message(&ep, Some(&child), "m-x")
        .await
        .unwrap_err();
    assert!(matches!(refused, AgentError::Rejected(_)), "{refused:?}");
    assert!(!lists(fake.rpc_extensions("list_tasks").last().unwrap()));
    // a parent context is listed with or without it
    assert_eq!(
        client
            .find_task_by_message(&ep, Some("ctx-1"), "m-x")
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn a_cancel_of_a_round_needs_the_activation_and_stops_the_round() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client();
    let parent = drain(
        client
            .send_stream(request(&ep, "child-sessions planner keep gate"))
            .await
            .unwrap(),
    )
    .await;
    let handle = TaskHandle {
        endpoint: ep.clone(),
        task_id: format!("{}-s1", parent[0].task_id),
    };
    // withdrawn: the agent refuses, the round is untouched
    fake.set_child_sessions(None);
    let refused = client.cancel(&handle).await.unwrap_err();
    assert!(matches!(refused, AgentError::Rejected(_)), "{refused:?}");
    fake.set_child_sessions(Some(offer()));
    let still = client.get_task(&handle).await.unwrap();
    assert_eq!(still.state, AgentTaskState::Working);
    // activated: the round is canceled and its session says so, idle because it is kept open
    let canceled = client.cancel(&handle).await.unwrap();
    assert_eq!(canceled.state, AgentTaskState::Canceled);
    let Some(SessionNews::OfRound(report)) = canceled.envelopes.first().and_then(news) else {
        panic!("{canceled:?}")
    };
    assert_eq!(report.state, SessionState::Idle);
    assert_eq!(
        report.round.as_ref().unwrap().state,
        Some(AgentTaskState::Canceled)
    );
}

#[tokio::test]
async fn a_close_that_waits_is_said_by_the_round_and_a_session_that_is_not_kept_closes() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client();
    let parent = drain(
        client
            .send_stream(request(&ep, "child-sessions planner keep gate close"))
            .await
            .unwrap(),
    )
    .await;
    let handle = TaskHandle {
        endpoint: ep.clone(),
        task_id: format!("{}-s1", parent[0].task_id),
    };
    let round = client.get_task(&handle).await.unwrap();
    let Some(SessionNews::OfRound(report)) = round.envelopes.first().and_then(news) else {
        panic!("{round:?}")
    };
    assert!(report.closing, "a close waits for the round");
    fake.release_gate();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let polled = client.get_task(&handle).await.unwrap();
            if let Some(SessionNews::OfRound(r)) = polled.envelopes.first().and_then(news)
                && r.state == SessionState::Closed
            {
                assert!(!r.closing);
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the session closed with its round");
}

#[tokio::test]
async fn an_offer_above_what_a_parent_can_note_is_no_offer() {
    let with = |max: u64| json!({"agents": [{"name": "planner", "description": "Plans."}], "maxOpen": max});
    for (max, listed) in [(256, true), (257, false), (4_294_967_295, false)] {
        let fake = agent(Some(with(max)), false).await;
        let ep = fake.endpoint("coder", None);
        let card = client().read_card(&ep).await.unwrap();
        assert_eq!(card.child_sessions.is_some(), listed, "maxOpen {max}");
        drain(client().send_stream(request(&ep, "echo hi")).await.unwrap()).await;
        assert_eq!(
            fake.executions()[0].activates_child_sessions(),
            listed,
            "maxOpen {max}"
        );
    }
}

#[tokio::test]
async fn a_round_says_its_own_session_and_is_followed_to_its_end() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client();
    let parent = drain(
        client
            .send_stream(request(&ep, "child-sessions planner keep gate"))
            .await
            .unwrap(),
    )
    .await;
    let session = format!("{}-s1", parent[0].task_id);
    let handle = TaskHandle {
        endpoint: ep.clone(),
        task_id: session.clone(),
    };
    let stream = client.resubscribe(&handle).await.unwrap();
    fake.release_gate();
    let envs = drain(stream).await;
    let first = &envs[0];
    let SessionNews::OfRound(report) = news(first).unwrap() else {
        panic!("{first:?}")
    };
    assert_eq!(report.session, session);
    assert_eq!(report.context_id, format!("ctx-1/s/{session}"));
    let round = report.round.as_ref().unwrap();
    assert_eq!(
        (round.number, round.task_id.as_deref()),
        (1, Some(session.as_str()))
    );
    assert_eq!(round.state, Some(AgentTaskState::Working));
    assert!(
        envs.iter().any(
            |e| matches!(&e.update, Some(AgentUpdate::Step(s)) if s.id.ends_with("helper:work"))
        )
    );
    assert!(envs.iter().any(|e| matches!(
        &e.update,
        Some(AgentUpdate::Message { text, .. }) if text == "planner: done"
    )));
    // the read before the end says the round completed, and the status ends it
    let ending = envs.iter().position(is_completed).unwrap();
    let last = envs[..ending]
        .iter()
        .rev()
        .find(|e| news(e).is_some())
        .unwrap();
    let SessionNews::OfRound(report) = news(last).unwrap() else {
        panic!("{last:?}")
    };
    assert_eq!(
        report.round.as_ref().unwrap().state,
        Some(AgentTaskState::Completed)
    );
    assert_eq!(report.state, SessionState::Idle);
    // a poll of the round says the same
    let polled = client.get_task(&handle).await.unwrap();
    assert!(matches!(
        polled.envelopes.first().and_then(news),
        Some(SessionNews::OfRound(_))
    ));
}

#[tokio::test]
async fn a_round_of_a_card_that_no_longer_lists_it_is_refused() {
    let fake = agent(Some(offer()), false).await;
    let ep = fake.endpoint("coder", None);
    let client = client();
    let parent = drain(
        client
            .send_stream(request(&ep, "child-sessions planner keep"))
            .await
            .unwrap(),
    )
    .await;
    let handle = TaskHandle {
        endpoint: ep,
        task_id: format!("{}-s1", parent[0].task_id),
    };
    fake.set_child_sessions(None);
    let Err(refused) = client.resubscribe(&handle).await else {
        panic!("a round was served to a request that did not activate the extension")
    };
    assert!(matches!(refused, AgentError::Rejected(_)), "{refused:?}");
    assert!(!refused.is_retryable());
    let Err(refused) = client.get_task(&handle).await else {
        panic!("a round was read by a request that did not activate the extension")
    };
    assert!(matches!(refused, AgentError::Rejected(_)), "{refused:?}");
}

#[tokio::test]
async fn a_later_message_starts_the_next_round_of_a_kept_open_session() {
    let fake = agent(Some(offer()), true).await;
    let ep = fake.endpoint("coder", None);
    let client = client();
    let parent = drain(
        client
            .send_stream(request(&ep, "child-sessions planner keep"))
            .await
            .unwrap(),
    )
    .await;
    let session = format!("{}-s1", parent[0].task_id);
    let again = drain(
        client
            .send_stream(request(&ep, &format!("child-sessions-send {session}")))
            .await
            .unwrap(),
    )
    .await;
    let listed = again.iter().find_map(|e| match news(e) {
        Some(SessionNews::Listed(l)) => Some(l.clone()),
        _ => None,
    });
    let round = listed.unwrap()[0].round.clone().unwrap();
    assert_eq!(
        (round.number, round.task_id),
        (2, Some(format!("{session}-r2")))
    );
    assert!(again.iter().any(|e| matches!(
        news(e),
        Some(SessionNews::Event(SessionEvent::Round, r)) if r.round.as_ref().is_some_and(|r| r.number == 2)
    )));
    // a session that does not exist is a failed task
    let failed = drain(
        client
            .send_stream(request(&ep, "child-sessions-send nope"))
            .await
            .unwrap(),
    )
    .await;
    assert!(failed.iter().any(|e| matches!(
        e.update,
        Some(AgentUpdate::Status {
            state: AgentTaskState::Failed,
            ..
        })
    )));
}

#[tokio::test]
async fn the_task_is_read_once_for_its_totals_and_its_sessions_before_its_status() {
    let fake = FakeAgent::spawn(FakeAgentOptions {
        child_sessions: Some(offer()),
        extensions: vec![
            orch_core::USAGE_EXTENSION.to_owned(),
            orch_core::STEPS_EXTENSION.to_owned(),
        ],
        ..FakeAgentOptions::default()
    })
    .await;
    let ep = fake.endpoint("coder", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "child-sessions planner keep"))
            .await
            .unwrap(),
    )
    .await;
    let total = envs
        .iter()
        .position(|e| {
            matches!(
                e.update,
                Some(AgentUpdate::Usage(orch_core::UsageUpdate::Total(_)))
            )
        })
        .expect("the totals");
    let last_news = envs.iter().rposition(|e| news(e).is_some()).unwrap();
    let ending = envs.iter().position(is_completed).unwrap();
    assert!(total < last_news && last_news < ending, "{envs:?}");
    // one read of the task serves the totals and the sessions
    assert_eq!(fake.rpc_count("get_task"), 1);
}
