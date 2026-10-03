//! Streamed text through the A2A adapter (ADR 0027, ADR 0008), against an in-process A2A agent
//! over real HTTP: `text-stream/v1` is activated (the header and the message's own `extensions`)
//! only for an agent whose live card lists it, on a send and on a resubscribe, the card is read for
//! every call and never remembered, and the response is read as data whether or not the request
//! activated the extension: the chunks of a reply are live pieces that are never applied, and the
//! text the agent states is one message under the stream's id.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{
    AgentTaskState, AgentUpdate, LiveChunk, LiveEnd, STEPS_EXTENSION, TEXT_STREAM_EXTENSION,
    UI_CATALOG_EXTENSION,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentStream, SendContent, SendRequest, TaskHandle,
};
use orch_testsupport::{FakeAgent, FakeAgentOptions, STREAM_PIECES, stream_id, stream_text};

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

fn pieces(envs: &[AgentEnvelope]) -> Vec<&LiveChunk> {
    envs.iter().filter_map(|e| e.live.as_ref()).collect()
}

fn messages(envs: &[AgentEnvelope]) -> Vec<(&str, &str)> {
    envs.iter()
        .filter_map(|e| match &e.update {
            Some(AgentUpdate::Message {
                message_id, text, ..
            }) => Some((message_id.as_str(), text.as_str())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn an_agent_that_lists_the_extension_is_asked_and_its_reply_arrives_as_pieces_then_one_message()
 {
    let fake = agent(&[TEXT_STREAM_EXTENSION]).await;
    let ep = fake.endpoint("stream", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "stream tell me"))
            .await
            .unwrap(),
    )
    .await;

    let call = fake.executions().pop().unwrap();
    assert!(
        call.activates(TEXT_STREAM_EXTENSION),
        "the header: {:?}",
        call.extensions_header
    );
    assert!(
        call.message_extensions
            .iter()
            .any(|e| e == TEXT_STREAM_EXTENSION),
        "the message's own extensions: {:?}",
        call.message_extensions
    );
    assert!(call.activates_text_stream());

    let task = envs[0].task_id.clone();
    let id = stream_id(&task);
    let got = pieces(&envs);
    assert_eq!(got.len(), STREAM_PIECES.len(), "{got:?}");
    let mut offset = 0u64;
    for (n, (piece, want)) in got.iter().zip(STREAM_PIECES).enumerate() {
        assert_eq!(piece.message_id, id);
        assert_eq!(piece.offset, offset, "chunk {n} is placed by bytes");
        assert_eq!(piece.text, want);
        let end = if n + 1 == STREAM_PIECES.len() {
            LiveEnd::Last
        } else {
            LiveEnd::Open
        };
        assert_eq!(piece.end, end);
        offset += want.len() as u64;
    }
    // A piece is never applied: no update, no task state.
    for env in envs.iter().filter(|e| e.live.is_some()) {
        assert_eq!(env.update, None);
        assert_eq!(env.task_state, None);
    }
    // The text is stated once, under the stream's id, then the turn ends with it as its words.
    assert_eq!(messages(&envs), [(id.as_str(), stream_text().as_str())]);
    let last = envs.last().unwrap();
    assert_eq!(last.task_state, Some(AgentTaskState::Completed));
    assert_eq!(
        last.update,
        Some(AgentUpdate::Status {
            state: AgentTaskState::Completed,
            detail: Some(stream_text())
        })
    );
    // No artifact was made of the chunks.
    assert!(
        envs.iter()
            .all(|e| !matches!(e.update, Some(AgentUpdate::Artifact { .. })))
    );
}

#[tokio::test]
async fn an_agent_that_does_not_list_the_extension_is_not_asked_and_what_it_sends_is_read_all_the_same()
 {
    // a card without it, and one that lists only other extensions of ours
    for extensions in [&[][..], &[UI_CATALOG_EXTENSION, STEPS_EXTENSION][..]] {
        let fake = agent(extensions).await;
        let ep = fake.endpoint("stream", None);
        let envs = drain(
            client()
                .send_stream(request(&ep, "stream tell me"))
                .await
                .unwrap(),
        )
        .await;
        let call = fake.executions().pop().unwrap();
        assert!(!call.activates(TEXT_STREAM_EXTENSION), "{extensions:?}");
        assert!(
            !call
                .message_extensions
                .iter()
                .any(|e| e == TEXT_STREAM_EXTENSION)
        );
        // the response is data: whatever the agent sent is read, asked for or not
        assert_eq!(pieces(&envs).len(), STREAM_PIECES.len(), "{extensions:?}");
        assert_eq!(messages(&envs).len(), 1);
    }
}

#[tokio::test]
async fn the_extensions_are_asked_for_together_and_each_alone() {
    let both = agent(&[STEPS_EXTENSION, TEXT_STREAM_EXTENSION]).await;
    let ep = both.endpoint("stream", None);
    drain(client().send_stream(request(&ep, "echo hi")).await.unwrap()).await;
    let call = both.executions().pop().unwrap();
    assert!(call.activates_steps() && call.activates_text_stream());

    let only_steps = agent(&[STEPS_EXTENSION]).await;
    let ep = only_steps.endpoint("stream", None);
    drain(client().send_stream(request(&ep, "echo hi")).await.unwrap()).await;
    let call = only_steps.executions().pop().unwrap();
    assert!(call.activates_steps() && !call.activates_text_stream());
}

#[tokio::test]
async fn a_near_miss_uri_is_not_the_extension() {
    for near in [
        "https://agents.vymalo.com/a2a/extensions/text-stream/v2",
        "https://agents.vymalo.com/a2a/extensions/text-stream/v1/",
        "http://agents.vymalo.com/a2a/extensions/text-stream/v1",
        "https://agents.vymalo.com/a2a/extensions/Text-Stream/v1",
    ] {
        let fake = agent(&[near]).await;
        let ep = fake.endpoint("stream", None);
        drain(client().send_stream(request(&ep, "echo hi")).await.unwrap()).await;
        let call = fake.executions().pop().unwrap();
        assert!(
            !call.activates(near) && !call.activates(TEXT_STREAM_EXTENSION),
            "{near}"
        );
        assert!(call.message_extensions.is_empty(), "{near}");
    }
}

#[tokio::test]
async fn the_card_is_read_for_every_message_and_never_remembered() {
    let fake = agent(&[TEXT_STREAM_EXTENSION]).await;
    let ep = fake.endpoint("stream", None);
    let client = client();
    drain(client.send_stream(request(&ep, "echo one")).await.unwrap()).await;
    assert!(fake.executions().pop().unwrap().activates_text_stream());

    // the agent drops the extension: the next message does not ask for it
    fake.set_extensions(&[]);
    drain(client.send_stream(request(&ep, "echo two")).await.unwrap()).await;
    let call = fake.executions().pop().unwrap();
    assert!(!call.activates(TEXT_STREAM_EXTENSION));
    assert!(call.message_extensions.is_empty());

    // and takes it back
    fake.set_extensions(&[TEXT_STREAM_EXTENSION]);
    drain(
        client
            .send_stream(request(&ep, "echo three"))
            .await
            .unwrap(),
    )
    .await;
    assert!(fake.executions().pop().unwrap().activates_text_stream());
}

#[tokio::test]
async fn a_resubscribe_asks_again_when_the_card_lists_the_extension() {
    let fake = agent(&[TEXT_STREAM_EXTENSION, STEPS_EXTENSION]).await;
    let ep = fake.endpoint("stream", None);
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
        vec![vec![
            STEPS_EXTENSION.to_owned(),
            TEXT_STREAM_EXTENSION.to_owned()
        ]],
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
async fn a_reply_that_is_given_up_ends_the_stream_and_states_no_text() {
    let fake = agent(&[TEXT_STREAM_EXTENSION]).await;
    let ep = fake.endpoint("stream-abandon", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "stream-abandon go"))
            .await
            .unwrap(),
    )
    .await;
    let got = pieces(&envs);
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].end, LiveEnd::Open);
    assert_eq!(got[2].end, LiveEnd::Abandoned);
    assert_eq!(got[2].text, "");
    assert!(
        messages(&envs).is_empty(),
        "nothing states the text of a reply that failed"
    );
    assert_eq!(
        envs.last().unwrap().task_state,
        Some(AgentTaskState::Failed)
    );
}

#[tokio::test]
async fn words_before_a_tool_call_are_a_message_of_their_own() {
    let fake = agent(&[TEXT_STREAM_EXTENSION, STEPS_EXTENSION]).await;
    let ep = fake.endpoint("stream-words", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "stream-words go"))
            .await
            .unwrap(),
    )
    .await;
    let task = envs[0].task_id.clone();
    let all = messages(&envs);
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(
        all[0],
        (
            format!("{task}-words").as_str(),
            "Let me run the tests first."
        )
    );
    assert_eq!(all[1].0, stream_id(&task));
    // The first is stated on a `working` status that says nothing more; the step comes after it.
    let working = envs
        .iter()
        .find(|e| {
            matches!(&e.update, Some(AgentUpdate::Status { state, .. }) if *state == AgentTaskState::Working)
        })
        .unwrap();
    assert_eq!(
        working.update,
        Some(AgentUpdate::Status {
            state: AgentTaskState::Working,
            detail: None
        })
    );
    assert!(
        envs.iter()
            .any(|e| matches!(e.update, Some(AgentUpdate::Step(_))))
    );
}

#[tokio::test]
async fn a_poll_gets_the_stated_text_under_the_key_of_the_stream_and_no_chunk() {
    let fake = agent(&[TEXT_STREAM_EXTENSION]).await;
    let ep = fake.endpoint("stream", None);
    let client = client();
    let envs = drain(
        client
            .send_stream(request(&ep, "stream tell me"))
            .await
            .unwrap(),
    )
    .await;
    let task = envs[0].task_id.clone();
    let snap = client
        .get_task(&TaskHandle {
            endpoint: ep,
            task_id: task.clone(),
        })
        .await
        .unwrap();
    assert_eq!(snap.state, AgentTaskState::Completed);
    // chunks are transient: none is in a task
    assert!(pieces(&snap.envelopes).is_empty());
    assert!(
        snap.envelopes
            .iter()
            .all(|e| !matches!(e.update, Some(AgentUpdate::Artifact { .. })))
    );
    // the stated text is, under the same key the stream gave it
    let polled: Vec<_> = snap
        .envelopes
        .iter()
        .filter(|e| matches!(e.update, Some(AgentUpdate::Message { .. })))
        .collect();
    let streamed: Vec<_> = envs
        .iter()
        .filter(|e| matches!(e.update, Some(AgentUpdate::Message { .. })))
        .collect();
    assert_eq!(polled.len(), 1);
    assert_eq!(polled[0].key, streamed[0].key);
    assert_eq!(polled[0].update, streamed[0].update);
}

#[tokio::test]
async fn an_agent_that_never_streamed_can_still_state_its_text() {
    let fake = agent(&[TEXT_STREAM_EXTENSION]).await;
    let ep = fake.endpoint("stream-marker", None);
    let envs = drain(
        client()
            .send_stream(request(&ep, "stream-marker go"))
            .await
            .unwrap(),
    )
    .await;
    assert!(pieces(&envs).is_empty());
    let task = envs[0].task_id.clone();
    assert_eq!(
        messages(&envs),
        [(stream_id(&task).as_str(), stream_text().as_str())]
    );
}
