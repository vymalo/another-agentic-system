//! Steps through the A2A adapter (ADR 0025, ADR 0008), against an in-process A2A agent over real
//! HTTP: the extension is activated (the header and the message's own `extensions`) only for an
//! agent whose live card lists `steps/v1`, on a send and on a resubscribe, the card is read for
//! every call and never remembered, and the response is read as data whether or not the request
//! activated the extension: a step in the stream is an `AgentUpdate::Step` with ids made unique by
//! the task id.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{
    AgentTaskState, AgentUpdate, STEPS_EXTENSION, StepKind, StepReport, StepState,
    UI_CATALOG_EXTENSION,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentStream, SendContent, SendRequest, TaskHandle,
};
use orch_testsupport::{FakeAgent, FakeAgentOptions};

fn client() -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap()
}

async fn agent(extensions: &[&str]) -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        extensions: extensions.iter().map(|u| (*u).to_owned()).collect(),
        ..FakeAgentOptions::default()
    })
    .await
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
        steer: false,
        mentions: Vec::new(),
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

fn steps(envs: &[AgentEnvelope]) -> Vec<&StepReport> {
    envs.iter()
        .filter_map(|e| match &e.update {
            Some(AgentUpdate::Step(s)) => Some(s),
            _ => None,
        })
        .collect()
}

/// One report of a turn: the id without the task, the parent, the kind, the state and the detail.
type Told = (String, Option<String>, StepKind, StepState, Option<String>);

/// What a `steps` turn reports.
fn story(task: &str, steps: &[&StepReport]) -> Vec<Told> {
    let bare = |id: &str| id.strip_prefix(&format!("{task}/")).unwrap().to_owned();
    steps
        .iter()
        .map(|s| {
            (
                bare(&s.id),
                s.parent.as_deref().map(bare),
                s.kind,
                s.state,
                s.detail.clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn an_agent_that_lists_the_extension_is_asked_for_steps_and_its_steps_arrive_nested() {
    let fake = agent(&[STEPS_EXTENSION]).await;
    let ep = fake.endpoint("steps", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "steps run the tests"))
            .await
            .unwrap(),
    )
    .await;

    let call = fake.executions().pop().unwrap();
    assert!(
        call.activates(STEPS_EXTENSION),
        "the header: {:?}",
        call.extensions_header
    );
    assert!(
        call.message_extensions.iter().any(|e| e == STEPS_EXTENSION),
        "the message's own extensions: {:?}",
        call.message_extensions
    );
    assert!(call.activates_steps());

    let task = envs[0].task_id.clone();
    let got = steps(&envs);
    assert_eq!(
        story(&task, &got),
        [
            (
                "tool:c2".to_owned(),
                None,
                StepKind::Subagent,
                StepState::Running,
                None
            ),
            (
                "acp:c2:1".to_owned(),
                Some("tool:c2".to_owned()),
                StepKind::Command,
                StepState::Running,
                None
            ),
            (
                "acp:c2:1".to_owned(),
                Some("tool:c2".to_owned()),
                StepKind::Command,
                StepState::Failed,
                Some("1 failed".to_owned())
            ),
            (
                "tool:c2".to_owned(),
                None,
                StepKind::Subagent,
                StepState::Completed,
                None
            ),
        ]
    );
    assert_eq!(got[0].label, "OpenCode");
    assert_eq!(got[0].icon.as_deref(), Some("agent"));
    // every step envelope is a `working` task; the keys name the step and its status message
    for env in envs
        .iter()
        .filter(|e| matches!(e.update, Some(AgentUpdate::Step(_))))
    {
        assert_eq!(env.task_state, Some(AgentTaskState::Working));
    }
    // the turn goes on after them: the agent's words, then the end
    let last = envs.last().unwrap();
    assert_eq!(last.task_state, Some(AgentTaskState::Completed));
    assert_eq!(
        last.update,
        Some(AgentUpdate::Status {
            state: AgentTaskState::Completed,
            detail: Some("Done.".to_owned())
        })
    );
    // each key is its own: no two reports of a step collapse into one
    let keys: std::collections::BTreeSet<_> = envs.iter().map(|e| format!("{:?}", e.key)).collect();
    assert_eq!(keys.len(), envs.len());
}

#[tokio::test]
async fn an_agent_that_does_not_list_the_extension_is_not_asked_and_its_steps_are_read_all_the_same()
 {
    // a card without it, and one that lists only other extensions of ours
    for extensions in [&[][..], &[UI_CATALOG_EXTENSION][..]] {
        let fake = agent(extensions).await;
        let ep = fake.endpoint("steps", None);
        let envs = drain(
            client()
                .send_stream(request(&ep, "steps run the tests"))
                .await
                .unwrap(),
        )
        .await;
        let call = fake.executions().pop().unwrap();
        assert!(!call.activates(STEPS_EXTENSION), "{extensions:?}");
        assert!(!call.message_extensions.iter().any(|e| e == STEPS_EXTENSION));
        // the response is data: whatever the agent reported is read, asked for or not
        assert_eq!(steps(&envs).len(), 4, "{extensions:?}");
    }
}

#[tokio::test]
async fn a_near_miss_uri_is_not_the_extension() {
    for near in [
        "https://agents.vymalo.com/a2a/extensions/steps/v2",
        "https://agents.vymalo.com/a2a/extensions/steps/v1/",
        "http://agents.vymalo.com/a2a/extensions/steps/v1",
        "https://agents.vymalo.com/a2a/extensions/Steps/v1",
    ] {
        let fake = agent(&[near]).await;
        let ep = fake.endpoint("steps", None);
        drain(client().send_stream(request(&ep, "echo hi")).await.unwrap()).await;
        let call = fake.executions().pop().unwrap();
        assert!(
            !call.activates(near) && !call.activates(STEPS_EXTENSION),
            "{near}"
        );
        assert!(call.message_extensions.is_empty(), "{near}");
    }
}

#[tokio::test]
async fn the_card_is_read_for_every_message_and_never_remembered() {
    let fake = agent(&[STEPS_EXTENSION]).await;
    let ep = fake.endpoint("steps", None);
    let client = client();
    drain(client.send_stream(request(&ep, "echo one")).await.unwrap()).await;
    assert!(fake.executions().pop().unwrap().activates_steps());

    // the agent drops the extension: the next message does not ask for it
    fake.set_extensions(&[]);
    drain(client.send_stream(request(&ep, "echo two")).await.unwrap()).await;
    let call = fake.executions().pop().unwrap();
    assert!(!call.activates(STEPS_EXTENSION));
    assert!(call.message_extensions.is_empty());

    // and takes it back
    fake.set_extensions(&[STEPS_EXTENSION]);
    drain(
        client
            .send_stream(request(&ep, "echo three"))
            .await
            .unwrap(),
    )
    .await;
    assert!(fake.executions().pop().unwrap().activates_steps());
}

#[tokio::test]
async fn a_resubscribe_asks_for_steps_again_when_the_card_lists_the_extension() {
    let fake = agent(&[STEPS_EXTENSION]).await;
    let ep = fake.endpoint("steps", None);
    let client = client();
    // a task that runs until it is cancelled, so that it can be subscribed to
    let mut first = client.send_stream(request(&ep, "slow work")).await.unwrap();
    let task = first.next().await.unwrap().unwrap().task_id;
    let handle = TaskHandle {
        endpoint: ep.clone(),
        task_id: task.clone(),
    };
    let again = client.resubscribe(&handle).await.unwrap();
    drop(again);
    assert_eq!(
        fake.subscription_extensions(),
        vec![vec![STEPS_EXTENSION.to_owned()]],
        "the header of the SubscribeToTask"
    );

    // the card is read for this call too: without the extension, nothing is asked for
    fake.set_extensions(&[]);
    let again = client.resubscribe(&handle).await.unwrap();
    drop(again);
    let subscriptions = fake.subscription_extensions();
    assert_eq!(subscriptions.len(), 2);
    assert!(subscriptions[1].is_empty(), "{subscriptions:?}");

    client.cancel(&handle).await.unwrap();
}

#[tokio::test]
async fn a_polled_task_whose_status_is_a_question_has_no_step() {
    // the task's current status is a step: a poll gives the same envelope (and key) as the stream
    let fake = agent(&[STEPS_EXTENSION]).await;
    let ep = fake.endpoint("steps", None);
    let client = client();
    let envs = drain(
        client
            .send_stream(request(&ep, "steps-ask clean the build"))
            .await
            .unwrap(),
    )
    .await;
    let task = envs[0].task_id.clone();
    let snap = client
        .get_task(&TaskHandle {
            endpoint: ep,
            task_id: task,
        })
        .await
        .unwrap();
    assert_eq!(snap.state, AgentTaskState::InputRequired);
    // the status of a waiting task is the question, not a step
    assert!(steps(&snap.envelopes).is_empty());
}

#[tokio::test]
async fn a_chatty_agent_reports_as_often_as_it_likes_and_every_report_arrives() {
    // coalescing is the core's: the adapter passes every report on, each under a key of its own
    let fake = agent(&[STEPS_EXTENSION]).await;
    let ep = fake.endpoint("steps", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "steps-chatty go"))
            .await
            .unwrap(),
    )
    .await;
    let reports = steps(&envs);
    assert_eq!(reports.len(), 22);
    assert_eq!(reports.last().map(|s| s.state), Some(StepState::Completed));
    let keys: std::collections::BTreeSet<_> = envs.iter().map(|e| format!("{:?}", e.key)).collect();
    assert_eq!(keys.len(), envs.len(), "no two reports share a key");
}
