//! The A2A adapter against an in-process A2A 1.0 agent over real HTTP.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig, RELEASE_CHANNELS_URI};
use orch_core::AgentUpdate;
use orch_core::{
    AgentTaskState, Classify, ErrorClass, ForkHistory, HistoryEntry, HistoryRole, history_preamble,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, IdemKey, SendContent,
    SendRequest, TaskHandle,
};
use orch_testsupport::{CallKind, FakeAgent, FakeAgentOptions, FakeReleases, eventually};

fn client() -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap()
}

async fn agent() -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions::default()).await
}

/// `ep` moved to another card URL (no bearer).
fn at(ep: &AgentEndpoint, card_url: impl Into<String>) -> AgentEndpoint {
    AgentEndpoint::a2a(ep.id.clone(), card_url, None)
}

fn request(ep: &AgentEndpoint, text: &str) -> SendRequest {
    SendRequest {
        endpoint: ep.clone(),
        message_id: format!("msg-{}", text.replace(' ', "-")),
        context_id: "ctx-1".to_owned(),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text(text.to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
    }
}

async fn drain(mut stream: AgentStream) -> Vec<AgentEnvelope> {
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
            Ok(Some(item)) => out.push(item.unwrap()),
            Ok(None) => return out,
            Err(_) => panic!("stream did not end; got {out:?}"),
        }
    }
}

fn statuses(envs: &[AgentEnvelope]) -> Vec<AgentTaskState> {
    envs.iter()
        .filter_map(|e| match &e.update {
            Some(AgentUpdate::Status { state, .. }) => Some(*state),
            _ => None,
        })
        .collect()
}

fn handle(ep: &AgentEndpoint, task_id: &str) -> TaskHandle {
    TaskHandle {
        endpoint: ep.clone(),
        task_id: task_id.to_owned(),
    }
}

// ------------------------------------------------------------------- cards

#[tokio::test]
async fn card_without_the_extension_has_no_releases() {
    let fake = agent().await;
    let card = client()
        .read_card(&fake.endpoint("plain", None))
        .await
        .unwrap();
    assert_eq!(
        card.description.as_deref(),
        Some("in-process fake A2A agent")
    );
    assert_eq!(card.releases, None);
}

#[tokio::test]
async fn card_with_the_extension_exposes_releases() {
    let fake = FakeAgent::spawn(FakeAgentOptions {
        releases: Some(FakeReleases::sample()),
        ..FakeAgentOptions::default()
    })
    .await;
    let card = client()
        .read_card(&fake.endpoint("coder", None))
        .await
        .unwrap();
    let releases = card.releases.expect("releases");
    assert_eq!(releases.default_channel, "production");
    assert_eq!(releases.channels["staging"], "coder-r51");
    assert_eq!(
        releases.revisions.unwrap(),
        vec!["coder-r53", "coder-r51", "coder-r47"]
    );
}

#[tokio::test]
async fn card_is_read_live_every_time() {
    // Same client, same endpoint: a card that changes is noticed (ADR 0008: never cached).
    let with = FakeAgent::spawn(FakeAgentOptions {
        releases: Some(FakeReleases::sample()),
        ..FakeAgentOptions::default()
    })
    .await;
    let without = agent().await;
    let c = client();
    assert!(
        c.read_card(&with.endpoint("a", None))
            .await
            .unwrap()
            .releases
            .is_some()
    );
    let mut ep = without.endpoint("a", None);
    assert!(c.read_card(&ep).await.unwrap().releases.is_none());
    ep = at(&ep, with.card_url());
    assert!(c.read_card(&ep).await.unwrap().releases.is_some());
}

#[tokio::test]
async fn card_url_may_be_a_base_url() {
    let fake = agent().await;
    let mut ep = fake.endpoint("plain", None);
    ep = at(&ep, fake.base_url().to_owned());
    assert!(client().read_card(&ep).await.is_ok());
    ep = at(&ep, format!("{}/", fake.base_url()));
    assert!(client().read_card(&ep).await.is_ok());
}

#[tokio::test]
async fn card_errors_are_classified() {
    let fake = agent().await;
    let mut ep = fake.endpoint("plain", None);
    ep = at(&ep, format!("{}/nope.json", fake.base_url()));
    assert!(matches!(
        client().read_card(&ep).await,
        Err(AgentError::Rejected(_))
    ));

    // A closed port: nothing listens.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    ep = at(&ep, format!("http://{closed}/.well-known/agent-card.json"));
    let err = client().read_card(&ep).await.unwrap_err();
    assert!(matches!(err, AgentError::Unreachable { .. }), "{err:?}");
    assert!(err.is_retryable());
}

/// A server that answers every request with the given raw HTTP response.
async fn serve_raw(response: &'static str) -> AgentEndpoint {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0_u8; 2048];
                let _ = sock.read(&mut buf).await;
                let _ = sock.write_all(response.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    AgentEndpoint::a2a(
        orch_core::AgentId::new("raw"),
        format!("http://{addr}/.well-known/agent-card.json"),
        None,
    )
}

#[tokio::test]
async fn card_statuses_are_classified_by_what_the_caller_can_do() {
    let c = client();

    // 429 asks to slow down, and says for how long.
    let ep = serve_raw(
        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    let err = c.read_card(&ep).await.unwrap_err();
    assert!(matches!(err, AgentError::RateLimited { .. }), "{err:?}");
    assert_eq!(err.class(), ErrorClass::RateLimited);
    assert_eq!(err.retry_after(), Some(Duration::from_secs(7)));
    assert!(err.is_retryable());

    // Without the header there is no hint.
    let ep = serve_raw(
        "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    let err = c.read_card(&ep).await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::RateLimited);
    assert_eq!(err.retry_after(), None);

    // A peer cannot park the delivery for a day.
    let ep = serve_raw(
        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 86400\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    let err = c.read_card(&ep).await.unwrap_err();
    assert_eq!(err.retry_after(), Some(Duration::from_secs(3600)));

    // 401 and 403: other credentials are needed, retrying does not help.
    for status in ["401 Unauthorized", "403 Forbidden"] {
        let raw: &'static str = Box::leak(
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .into_boxed_str(),
        );
        let err = c.read_card(&serve_raw(raw).await).await.unwrap_err();
        assert!(matches!(err, AgentError::Unauthenticated { .. }), "{err:?}");
        assert_eq!(err.class(), ErrorClass::Unauthenticated);
        assert!(!err.is_retryable());
    }

    // 503 and 408: the agent (or its proxy) is in trouble, try again later.
    for status in ["503 Service Unavailable", "408 Request Timeout"] {
        let raw: &'static str = Box::leak(
            format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .into_boxed_str(),
        );
        let err = c.read_card(&serve_raw(raw).await).await.unwrap_err();
        assert!(matches!(err, AgentError::Unreachable { .. }), "{err:?}");
        assert!(err.is_retryable());
    }

    // Any other 4xx refuses the request for good.
    let ep =
        serve_raw("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
    let err = c.read_card(&ep).await.unwrap_err();
    assert!(matches!(err, AgentError::Rejected(_)), "{err:?}");
    assert!(!err.is_retryable());
}

#[tokio::test]
async fn an_unreachable_card_keeps_the_transport_error_as_source_and_out_of_the_message() {
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let ep = AgentEndpoint::a2a(
        orch_core::AgentId::new("gone"),
        format!("http://{closed}/.well-known/agent-card.json"),
        None,
    );
    let err = client().read_card(&ep).await.unwrap_err();
    let source = std::error::Error::source(&err).expect("the transport error is kept");
    assert!(
        source.downcast_ref::<reqwest::Error>().is_some(),
        "{source:?}"
    );
    // The public text and the message never carry the address; the operator's report does not
    // carry the URL either (`without_url`), only the reason.
    assert!(!err.public_detail().contains("127.0.0.1"));
    assert!(!err.to_string().contains("127.0.0.1"));
    assert!(!orch_core::report(&err).contains("127.0.0.1"));
}

// ---------------------------------------------------------------- streaming

#[tokio::test]
async fn echo_streams_working_artifact_completed_with_stable_keys() {
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let stream = client().send_stream(request(&ep, "echo hi")).await.unwrap();
    let envs = drain(stream).await;

    assert_eq!(
        statuses(&envs),
        vec![AgentTaskState::Working, AgentTaskState::Completed]
    );
    let task = envs[0].task_id.clone();
    assert!(!task.is_empty(), "the first envelope names the task");
    assert!(
        envs.iter()
            .all(|e| e.task_id == task && e.context_id == "ctx-1")
    );

    assert_eq!(envs[0].key, IdemKey::Turn(format!("{task}:status:working")));
    let artifact = envs
        .iter()
        .find(|e| matches!(e.update, Some(AgentUpdate::Artifact { .. })))
        .unwrap();
    let IdemKey::Task(key) = &artifact.key else {
        panic!("artifact keys are task scoped");
    };
    assert!(key.starts_with(&format!("a2a:{task}:artifact:")), "{key}");
    assert_eq!(
        artifact.update,
        Some(AgentUpdate::Artifact {
            name: "result".into(),
            mime_type: None,
            uri: Some(orch_testsupport::fake::PR_URL.into()),
            text: Some("echo: echo hi".into()),
        })
    );
    // The agent saw exactly what we sent.
    let calls = fake.executions();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].message_id.as_deref(), Some("msg-echo-hi"));
    assert_eq!(calls[0].context_id, "ctx-1");
    assert!(!calls[0].resuming);
    assert!(
        !calls[0].activates_release_channels(),
        "no release, no extension header"
    );
}

#[tokio::test]
async fn a_new_task_names_the_tasks_it_is_about_in_its_message() {
    // A2A `referenceTaskIds` (ADR 0021): the message carries them, and an agent sees them.
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let first = drain(
        client()
            .send_stream(request(&ep, "echo one"))
            .await
            .unwrap(),
    )
    .await;
    let task = first[0].task_id.clone();
    let mut second = request(&ep, "echo two");
    second.message_id = "msg-two".to_owned();
    second.reference_task_ids = vec![task.clone()];
    drain(client().send_stream(second).await.unwrap()).await;
    let calls = fake.executions();
    assert_eq!(calls.len(), 2);
    assert!(calls[0].reference_task_ids.is_empty());
    assert_eq!(calls[1].reference_task_ids, [task]);
    assert_ne!(calls[1].task_id, calls[0].task_id);
}

/// The conversation of a fork: what the person said and what the agent answered.
fn conversation() -> ForkHistory {
    let entry = |role, name: &str, text: &str| HistoryEntry {
        role,
        name: name.to_owned(),
        text: text.to_owned(),
    };
    ForkHistory {
        entries: vec![
            entry(HistoryRole::Person, "person", "fix the redirect loop"),
            entry(HistoryRole::Agent, "plain", "Fixed.\nSee the branch."),
        ],
        omitted: 0,
    }
}

#[tokio::test]
async fn the_first_task_of_a_fork_carries_the_conversation_in_front_of_its_message() {
    // ADR 0029: a fork is a new context, so its agent is told what was said, as plain text in
    // the same part as the message: any A2A agent reads it, and the scripts of the fake read the
    // message after it.
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let history = conversation();
    let mut first = request(&ep, "recall and go on");
    first.history = Some(history.clone());
    let envelopes = drain(client().send_stream(first).await.unwrap()).await;
    assert_eq!(
        statuses(&envelopes).last(),
        Some(&AgentTaskState::Completed)
    );
    let calls = fake.executions();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].text,
        format!("{}recall and go on", history_preamble(&history)),
        "one text part: the conversation, a blank line, the message"
    );
    assert!(
        calls[0]
            .text
            .contains("<<<conversation\nperson: fix the redirect loop\nplain: Fixed.\n  See the branch.\n>>>conversation\n")
    );
    // and the agent read the conversation it was told: its answer quotes the first line of it
    let artifact = envelopes
        .iter()
        .find_map(|e| match &e.update {
            Some(AgentUpdate::Artifact { text, .. }) => text.clone(),
            _ => None,
        })
        .expect("an artifact");
    assert_eq!(artifact, "recalled: person: fix the redirect loop");
}

#[tokio::test]
async fn a_message_without_a_conversation_is_sent_as_it_is() {
    // The later tasks of a fork, a thread that is not one, and the verifier (ADR 0002; the
    // dispatcher builds its request with no history) are sent with none: their text is the
    // message and nothing before it, and an agent that is asked to `recall` has nothing to quote.
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let mut first = request(&ep, "echo one");
    first.history = Some(conversation());
    let envelopes = drain(client().send_stream(first).await.unwrap()).await;
    let task = envelopes[0].task_id.clone();
    let mut second = request(&ep, "echo two");
    second.message_id = "msg-two".to_owned();
    second.reference_task_ids = vec![task];
    drain(client().send_stream(second).await.unwrap()).await;
    let mut other = request(&ep, "recall");
    other.message_id = "msg-other".to_owned();
    other.context_id = "ctx-other".to_owned();
    let answer = drain(client().send_stream(other).await.unwrap()).await;
    let calls = fake.executions();
    assert_eq!(calls.len(), 3);
    assert!(calls[0].text.starts_with("[This chat continues"));
    assert_eq!(calls[1].text, "echo two");
    assert_eq!(calls[2].text, "recall");
    let said = answer.iter().find_map(|e| match &e.update {
        Some(AgentUpdate::Artifact { text, .. }) => text.clone(),
        _ => None,
    });
    assert_eq!(said.as_deref(), Some("recalled: nothing"));
}

#[tokio::test]
async fn an_action_is_never_given_a_conversation_and_an_empty_one_changes_nothing() {
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    // a conversation with nothing in it is no preamble at all
    let mut empty = request(&ep, "echo hi");
    empty.history = Some(ForkHistory::default());
    drain(client().send_stream(empty).await.unwrap()).await;
    // an action is a data part: there is no text to put the conversation in front of
    let mut action = request(&ep, "echo hi");
    action.message_id = "msg-action".to_owned();
    action.content = SendContent::UiAction {
        action: orch_core::UiActionData {
            surface_id: "s1".to_owned(),
            name: "go".to_owned(),
            source_component_id: "btn".to_owned(),
            context: serde_json::Map::new(),
            version: orch_core::UiVersion::V0_9_1,
            run_id: None,
        },
        at: jiff::Timestamp::UNIX_EPOCH,
    };
    action.history = Some(conversation());
    drain(client().send_stream(action).await.unwrap()).await;
    let calls = fake.executions();
    assert_eq!(calls[0].text, "echo hi");
    assert!(
        !calls[1].text.contains("<<<conversation"),
        "{}",
        calls[1].text
    );
    assert_eq!(calls[1].actions.len(), 1);
}

#[tokio::test]
async fn chunked_artifacts_arrive_as_one_artifact() {
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let envs = drain(client().send_stream(request(&ep, "chunks")).await.unwrap()).await;
    let artifacts: Vec<_> = envs
        .iter()
        .filter_map(|e| match &e.update {
            Some(AgentUpdate::Artifact { name, text, .. }) => Some((name.clone(), text.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        artifacts,
        vec![("log".to_owned(), Some("one\ntwo\nthree".to_owned()))]
    );
}

#[tokio::test]
async fn failed_task_carries_its_message() {
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "fail now"))
            .await
            .unwrap(),
    )
    .await;
    let last = envs.last().unwrap();
    assert_eq!(
        last.update,
        Some(AgentUpdate::Status {
            state: AgentTaskState::Failed,
            detail: Some("scripted failure".into())
        })
    );
}

#[tokio::test]
async fn ask_blocks_and_the_follow_up_continues_the_same_task() {
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let c = client();
    let first = drain(c.send_stream(request(&ep, "ask what")).await.unwrap()).await;
    assert_eq!(
        first.last().unwrap().update,
        Some(AgentUpdate::Status {
            state: AgentTaskState::InputRequired,
            detail: Some("Which branch?".into())
        })
    );
    let IdemKey::Task(key) = &first.last().unwrap().key else {
        panic!("a status with a message has a task scoped key");
    };
    assert!(key.starts_with("a2a:"), "{key}");
    let task = first[0].task_id.clone();

    let mut follow_up = request(&ep, "main");
    follow_up.task_id = Some(task.clone());
    let second = drain(c.send_stream(follow_up).await.unwrap()).await;
    assert_eq!(
        statuses(&second),
        vec![AgentTaskState::Working, AgentTaskState::Completed]
    );
    assert!(second.iter().all(|e| e.task_id == task), "same A2A task");

    let calls = fake.executions();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].task_id, calls[1].task_id);
    assert!(calls[1].resuming);
    assert_eq!(calls[1].text, "main");
}

// -------------------------------------------------- resubscribe, poll, cancel

#[tokio::test]
async fn resubscribe_to_a_finished_task_is_task_not_found() {
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let c = client();
    let envs = drain(c.send_stream(request(&ep, "echo x")).await.unwrap()).await;
    let task = envs[0].task_id.clone();
    let err = c
        .resubscribe(&handle(&ep, &task))
        .await
        .err()
        .expect("must fail");
    assert!(matches!(err, AgentError::TaskNotFound(_)), "{err:?}");
    assert!(!err.is_retryable());
}

#[tokio::test]
async fn resubscribe_to_a_running_task_yields_a_snapshot_then_the_rest_with_the_same_keys() {
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let c = client();
    let mut live = c.send_stream(request(&ep, "gate go")).await.unwrap();
    let working = live.next().await.unwrap().unwrap();
    assert_eq!(working.task_state, Some(AgentTaskState::Working));
    let task = working.task_id.clone();

    // A second observer attaches while the task runs, then the gate opens.
    let resumed = c.resubscribe(&handle(&ep, &task)).await.unwrap();
    fake.release_gate();
    let envs = drain(resumed).await;
    assert_eq!(
        envs[0].task_state,
        Some(AgentTaskState::Working),
        "snapshot first"
    );
    assert_eq!(
        envs[0].key, working.key,
        "the snapshot's status has the live stream's key, so the dispatcher deduplicates it"
    );
    assert_eq!(statuses(&envs).last(), Some(&AgentTaskState::Completed));
    let live_rest = drain(live).await;
    let live_artifact = live_rest
        .iter()
        .find(|e| matches!(e.update, Some(AgentUpdate::Artifact { .. })))
        .unwrap();
    let resub_artifact = envs
        .iter()
        .find(|e| matches!(e.update, Some(AgentUpdate::Artifact { .. })))
        .unwrap();
    assert_eq!(live_artifact.key, resub_artifact.key);
}

#[tokio::test]
async fn resubscribe_can_be_unsupported() {
    let fake = FakeAgent::spawn(FakeAgentOptions {
        resubscribe: false,
        ..FakeAgentOptions::default()
    })
    .await;
    let ep = fake.endpoint("plain", None);
    let c = client();
    let err = c
        .resubscribe(&handle(&ep, "any"))
        .await
        .err()
        .expect("must fail");
    assert!(matches!(err, AgentError::Unsupported(_)), "{err:?}");
}

#[tokio::test]
async fn get_task_snapshot_matches_the_live_stream() {
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let c = client();
    let live = drain(c.send_stream(request(&ep, "echo poll")).await.unwrap()).await;
    let task = live[0].task_id.clone();

    let snap = c.get_task(&handle(&ep, &task)).await.unwrap();
    assert_eq!(snap.task_id, task);
    assert_eq!(snap.context_id, "ctx-1");
    assert_eq!(snap.state, AgentTaskState::Completed);
    assert_eq!(snap.envelopes.len(), 2, "artifact, then status");
    let live_artifact = live
        .iter()
        .find(|e| matches!(e.update, Some(AgentUpdate::Artifact { .. })))
        .unwrap();
    assert_eq!(snap.envelopes[0].key, live_artifact.key);
    assert_eq!(snap.envelopes[0].update, live_artifact.update);
    let live_done = live.last().unwrap();
    assert_eq!(snap.envelopes[1].key, live_done.key);
}

#[tokio::test]
async fn get_task_of_an_unknown_task_is_task_not_found() {
    let fake = agent().await;
    let err = client()
        .get_task(&handle(&fake.endpoint("plain", None), "nope"))
        .await
        .unwrap_err();
    assert!(matches!(err, AgentError::TaskNotFound(_)), "{err:?}");
}

#[tokio::test]
async fn cancel_stops_a_running_task_and_a_second_cancel_is_refused_after_completion() {
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let c = client();
    let mut live = c.send_stream(request(&ep, "slow job")).await.unwrap();
    let task = live.next().await.unwrap().unwrap().task_id;

    let snap = c.cancel(&handle(&ep, &task)).await.unwrap();
    assert_eq!(snap.state, AgentTaskState::Canceled);
    let cancels = fake.cancels();
    assert_eq!(cancels.len(), 1);
    assert_eq!(cancels[0].task_id, task);
    assert_eq!(cancels[0].kind, CallKind::Cancel);
    // The live stream ends with the canceled status too.
    let rest = drain(live).await;
    assert_eq!(statuses(&rest).last(), Some(&AgentTaskState::Canceled));

    // Completed tasks cannot be canceled.
    let done = drain(c.send_stream(request(&ep, "echo finished")).await.unwrap()).await;
    let err = c.cancel(&handle(&ep, &done[0].task_id)).await.unwrap_err();
    assert!(matches!(err, AgentError::NotCancelable(_)), "{err:?}");
}

#[tokio::test]
async fn find_task_by_message_uses_list_tasks() {
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let c = client();
    let envs = drain(c.send_stream(request(&ep, "echo find")).await.unwrap()).await;
    let task = envs[0].task_id.clone();

    assert_eq!(
        c.find_task_by_message(&ep, "ctx-1", "msg-echo-find")
            .await
            .unwrap(),
        Some(task)
    );
    assert_eq!(
        c.find_task_by_message(&ep, "ctx-1", "unknown")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        c.find_task_by_message(&ep, "other-ctx", "msg-echo-find")
            .await
            .unwrap(),
        None
    );
}

// ----------------------------------------------------------------- releases

#[tokio::test]
async fn selected_release_reaches_the_agent_as_header_and_metadata() {
    let fake = FakeAgent::spawn(FakeAgentOptions {
        releases: Some(FakeReleases::sample()),
        ..FakeAgentOptions::default()
    })
    .await;
    let ep = fake.endpoint("coder", None);
    let mut req = request(&ep, "echo ship");
    req.release = Some("staging".to_owned());
    let envs = drain(client().send_stream(req).await.unwrap()).await;

    let calls = fake.executions();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].activates_release_channels(),
        "{:?}",
        calls[0].extensions_header
    );
    assert_eq!(
        calls[0].extensions_header,
        vec![RELEASE_CHANNELS_URI.to_owned()]
    );
    assert_eq!(calls[0].release.as_deref(), Some("staging"));
    // The agent echoes the revision it resolved; the adapter surfaces it.
    assert!(
        envs.iter()
            .all(|e| e.revision.as_deref() == Some("coder-r51")),
        "{:?}",
        envs.iter().map(|e| &e.revision).collect::<Vec<_>>()
    );
    assert_eq!(
        envs[0].task_state,
        Some(AgentTaskState::Submitted),
        "the Task frame names the task"
    );
    assert_eq!(envs[0].update, None);
}

#[tokio::test]
async fn no_release_selected_sends_no_extension_header_and_gets_the_default_revision() {
    let fake = FakeAgent::spawn(FakeAgentOptions {
        releases: Some(FakeReleases::sample()),
        ..FakeAgentOptions::default()
    })
    .await;
    let ep = fake.endpoint("coder", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "echo dflt"))
            .await
            .unwrap(),
    )
    .await;
    let calls = fake.executions();
    assert!(!calls[0].activates_release_channels());
    assert_eq!(calls[0].release, None);
    assert_eq!(envs.last().unwrap().revision.as_deref(), Some("coder-r47"));
}

#[tokio::test]
async fn unknown_release_fails_the_task_and_never_falls_back() {
    let fake = FakeAgent::spawn(FakeAgentOptions {
        releases: Some(FakeReleases::sample()),
        ..FakeAgentOptions::default()
    })
    .await;
    let ep = fake.endpoint("coder", None);
    let mut req = request(&ep, "echo nope");
    req.release = Some("does-not-exist".to_owned());
    let envs = drain(client().send_stream(req).await.unwrap()).await;
    assert_eq!(statuses(&envs), vec![AgentTaskState::Failed]);
    let Some(AgentUpdate::Status { detail, .. }) = &envs[0].update else {
        panic!("status expected");
    };
    assert!(detail.as_deref().unwrap().contains("does-not-exist"));
}

#[tokio::test]
async fn a_release_is_refused_when_the_live_card_no_longer_offers_releases() {
    let fake = agent().await;
    let ep = fake.endpoint("plain", None);
    let mut req = request(&ep, "echo x");
    req.release = Some("staging".to_owned());
    let err = client().send_stream(req).await.err().expect("must fail");
    assert!(matches!(err, AgentError::Rejected(_)), "{err:?}");
    assert!(!err.is_retryable());
    assert!(fake.executions().is_empty(), "nothing may reach the agent");
}

// --------------------------------------------------------------------- auth

#[tokio::test]
async fn bearer_token_reaches_the_agent() {
    let fake = FakeAgent::spawn(FakeAgentOptions {
        bearer: Some("s3cret".to_owned()),
        ..FakeAgentOptions::default()
    })
    .await;
    let ep = fake.endpoint("plain", Some("s3cret"));
    let c = client();
    let envs = drain(c.send_stream(request(&ep, "echo auth")).await.unwrap()).await;
    assert_eq!(statuses(&envs).last(), Some(&AgentTaskState::Completed));
    assert_eq!(
        fake.executions()[0].authorization.as_deref(),
        Some("Bearer s3cret")
    );
    // Every other operation authenticates too.
    let snap = c.get_task(&handle(&ep, &envs[0].task_id)).await.unwrap();
    assert_eq!(snap.state, AgentTaskState::Completed);
    assert_eq!(fake.unauthorized_requests(), 0);
}

#[tokio::test]
async fn wrong_or_missing_token_is_an_error_not_a_hang() {
    let fake = FakeAgent::spawn(FakeAgentOptions {
        bearer: Some("s3cret".to_owned()),
        ..FakeAgentOptions::default()
    })
    .await;
    let c = client();
    for token in [Some("wrong"), None] {
        let ep = fake.endpoint("plain", token);
        let outcome = tokio::time::timeout(Duration::from_secs(10), async {
            // The SDK loses the 401 and treats the empty body as an empty stream; the adapter
            // turns that into an error, which the dispatcher retries a bounded number of times.
            let mut stream = c.send_stream(request(&ep, "echo nope")).await?;
            stream.next().await.expect("an item")
        })
        .await
        .expect("must not hang");
        let err = outcome.unwrap_err();
        assert!(matches!(err, AgentError::Protocol { .. }), "{err:?}");
        assert!(err.is_retryable());
    }
    assert!(fake.executions().is_empty());
    eventually("both refusals counted", || async {
        (fake.unauthorized_requests() >= 2).then_some(())
    })
    .await;
}

#[tokio::test]
async fn a_local_endpoint_is_unsupported_on_every_operation() {
    let client = client();
    let ep = AgentEndpoint::local(orch_core::AgentId::new("helper"), "echo");
    let task = TaskHandle {
        endpoint: ep.clone(),
        task_id: "t1".to_owned(),
    };
    let is_unsupported = |e: AgentError| {
        assert!(matches!(e, AgentError::Unsupported(_)), "{e:?}");
        assert_eq!(e.class(), ErrorClass::Unsupported);
    };
    is_unsupported(client.read_card(&ep).await.unwrap_err());
    is_unsupported(client.send_stream(request(&ep, "hi")).await.err().unwrap());
    is_unsupported(client.resubscribe(&task).await.err().unwrap());
    is_unsupported(client.get_task(&task).await.unwrap_err());
    is_unsupported(client.cancel(&task).await.unwrap_err());
    is_unsupported(
        client
            .find_task_by_message(&ep, "ctx-1", "m1")
            .await
            .unwrap_err(),
    );
}
