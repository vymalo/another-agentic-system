//! Token usage through the A2A adapter (ADR 0056, ADR 0008), against an in-process A2A agent over
//! real HTTP: `usage/v1` is activated (the header and the message's own `extensions`) only for an
//! agent whose live card lists it, on a send and on a resubscribe; a call report on a `working`
//! update with no message is an `AgentUpdate::Usage` call and nothing else; the task's totals come
//! before the status that ends it, from the update when it carries them and otherwise from one read
//! of the task (`GetTask`), which happens only when the call activated the extension.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{AgentTaskState, AgentUpdate, USAGE_EXTENSION, UsageInvalid, UsageUpdate};
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

/// What the envelopes say, one word each: what the order of a turn is checked against.
fn story(envs: &[AgentEnvelope]) -> Vec<String> {
    envs.iter()
        .filter_map(|e| match &e.update {
            Some(AgentUpdate::Usage(UsageUpdate::Call(c))) => Some(format!(
                "call {} {}",
                c.call,
                c.step
                    .as_deref()
                    .map_or("-", |s| s.rsplit('/').next().unwrap_or(s))
            )),
            Some(AgentUpdate::Usage(UsageUpdate::Total(t))) => {
                Some(format!("total {}", t.totals.len()))
            }
            Some(AgentUpdate::UsageRejected(why)) => Some(format!("rejected {}", why.as_str())),
            Some(AgentUpdate::Step(s)) => Some(format!("step {:?}", s.state).to_lowercase()),
            Some(AgentUpdate::Message { .. }) => Some("message".to_owned()),
            Some(AgentUpdate::Status { state, .. }) => Some(format!("status {state:?}")),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_turn_reports_each_call_and_its_totals_before_the_end_read_from_the_task() {
    let agent = agent(&[USAGE_EXTENSION]).await;
    let ep = agent.endpoint("coder", None);
    let envs = drain(client().send_stream(request(&ep, "usage")).await.unwrap()).await;
    assert_eq!(
        story(&envs),
        [
            "status Working",
            "call c1 -",
            "step running",
            "call c2 tool:c2",
            "step completed",
            "call c3 -",
            "status Working",
            "message",
            "total 2",
            "status Completed",
        ],
        "{envs:?}"
    );
    // the call under the sub-agent's step names it as the thread knows it: the task's id first
    let task = &envs[0].task_id;
    let calls: Vec<_> = envs
        .iter()
        .filter_map(|e| match &e.update {
            Some(AgentUpdate::Usage(UsageUpdate::Call(c))) => Some((e, c)),
            _ => None,
        })
        .collect();
    assert_eq!(
        calls[1].1.step.as_deref(),
        Some(format!("{task}/tool:c2").as_str())
    );
    for (env, call) in &calls {
        assert_eq!(env.task_state, Some(AgentTaskState::Working));
        assert_eq!(
            env.key,
            orch_ports::IdemKey::Task(format!("a2a:{task}:usage:{}", call.call))
        );
    }
    // numbers an A2A server hands back as doubles are whole numbers all the same
    assert_eq!(calls[2].1.tokens.input_tokens, 2400);
    assert_eq!(calls[2].1.context_window, Some(131_072));
    // the stream said no totals: the adapter read the task once for them
    assert_eq!(agent.rpc_count("get_task"), 1);
    let call = &agent.executions()[0];
    assert!(call.activates_usage(), "{call:?}");
}

#[tokio::test]
async fn totals_on_the_update_that_ends_the_turn_need_no_read_of_the_task() {
    let agent = agent(&[USAGE_EXTENSION]).await;
    let ep = agent.endpoint("coder", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "usage-inline"))
            .await
            .unwrap(),
    )
    .await;
    let story = story(&envs);
    assert_eq!(&story[story.len() - 2..], ["total 2", "status Completed"]);
    assert_eq!(agent.rpc_count("get_task"), 0);
}

#[tokio::test]
async fn an_agent_whose_card_does_not_list_it_is_not_asked_and_the_task_is_not_read() {
    let agent = agent(&[]).await;
    let ep = agent.endpoint("coder", None);
    let envs = drain(client().send_stream(request(&ep, "usage")).await.unwrap()).await;
    let call = &agent.executions()[0];
    assert!(!call.activates(USAGE_EXTENSION), "{call:?}");
    assert!(
        !call.message_extensions.iter().any(|e| e == USAGE_EXTENSION),
        "{call:?}"
    );
    // what the response says is data all the same (as for steps), but no task is read for totals
    assert!(story(&envs).iter().any(|s| s == "call c1 -"));
    assert!(!story(&envs).iter().any(|s| s.starts_with("total")));
    assert_eq!(agent.rpc_count("get_task"), 0);
}

#[tokio::test]
async fn a_report_that_breaks_the_contract_is_rejected_and_the_turn_goes_on() {
    let agent = agent(&[USAGE_EXTENSION]).await;
    let ep = agent.endpoint("coder", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "usage-bad"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        story(&envs),
        [
            "status Working",
            "rejected total_not_sum",
            "status Completed"
        ],
        "{envs:?}"
    );
    assert!(
        envs.iter()
            .any(|e| e.update == Some(AgentUpdate::UsageRejected(UsageInvalid::TotalNotSum)))
    );
}

#[tokio::test]
async fn a_resubscribe_activates_it_when_the_card_lists_it() {
    let fake = agent(&[USAGE_EXTENSION]).await;
    let ep = fake.endpoint("coder", None);
    let client = client();
    // a task that runs until it is cancelled, so that it can be subscribed to
    let mut first = client.send_stream(request(&ep, "slow work")).await.unwrap();
    let task = first.next().await.unwrap().unwrap().task_id;
    let handle = TaskHandle {
        endpoint: ep.clone(),
        task_id: task,
    };
    let again = client.resubscribe(&handle).await.unwrap();
    drop(again);
    assert_eq!(
        fake.subscription_extensions(),
        vec![vec![USAGE_EXTENSION.to_owned()]],
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
