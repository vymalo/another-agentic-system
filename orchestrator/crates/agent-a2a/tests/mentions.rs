//! The mentions of a message through the A2A adapter (`mentions/v1`, ADR 0026, ADR 0008), against
//! an in-process A2A agent over real HTTP: the references are in the message metadata under the
//! extension's URI and the URI is activated, only for an agent whose live card lists it and only
//! when the message mentions agents; any other agent is sent the text as it is and nothing else.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{
    AgentId, ForkHistory, HistoryEntry, HistoryRole, MENTIONS_EXTENSION, THREAD_TOOLS_EXTENSION,
    ThreadId, ToolsGrant, history_preamble, utf16_len,
};
use orch_ports::{AgentClient, AgentEndpoint, AgentStream, MentionInfo, SendContent, SendRequest};
use orch_testsupport::{Call, FakeAgent, FakeAgentOptions};
use orch_thread_token::{ThreadToolsIssuer, ThreadToolsKeys};
use secrecy::SecretString;
use serde_json::json;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

/// A person writes this; the emoji is two UTF-16 code units, so `@mock-researcher` begins at 3.
const TEXT: &str = "\u{1F604} @mock-researcher check this";

fn client(asks: bool) -> A2aAgentClient {
    let issuer = ThreadToolsIssuer::new(
        ThreadToolsKeys::new(SecretString::from(KEY.to_owned()), None).unwrap(),
        "http://orchestrator:8080",
        Duration::from_secs(7200),
    )
    .unwrap();
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        thread_tools: Some(Arc::new(issuer)),
        asks,
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

fn researcher() -> MentionInfo {
    MentionInfo {
        agent_id: AgentId::new("mock-researcher"),
        name: Some("Mock researcher".to_owned()),
        label: "@mock-researcher".to_owned(),
        start: 3,
        end: 19,
        card_url: Some("http://mock-researcher:8080/.well-known/agent-card.json".to_owned()),
    }
}

fn request(fake: &FakeAgent, mentions: Vec<MentionInfo>) -> SendRequest {
    let thread = ThreadId(uuid::Uuid::from_u128(
        0x0192_7a4e_3b00_7000_8000_0000_0000_0001,
    ));
    let ep: AgentEndpoint = fake.endpoint("chat", None);
    SendRequest {
        endpoint: ep,
        message_id: "msg-1".to_owned(),
        context_id: "ctx-1".to_owned(),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text(TEXT.to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools: Some(ToolsGrant::main(thread, 1, AgentId::new("chat"))),
        history: None,
        steer: false,
        mentions,
    }
}

async fn drain(mut stream: AgentStream) {
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
            Ok(Some(item)) => {
                item.unwrap();
            }
            Ok(None) => return,
            Err(_) => panic!("the stream did not end"),
        }
    }
}

async fn send(client: &A2aAgentClient, fake: &FakeAgent, req: SendRequest) -> Call {
    drain(client.send_stream(req).await.unwrap()).await;
    fake.executions().pop().unwrap()
}

#[tokio::test]
async fn an_agent_whose_card_lists_the_extension_is_told_the_references_and_the_uri_is_activated() {
    let fake = agent(&[MENTIONS_EXTENSION]).await;
    let call = send(&client(false), &fake, request(&fake, vec![researcher()])).await;
    assert_eq!(
        call.mentions,
        Some(json!({"mentions": [{
            "agentId": "mock-researcher",
            "name": "Mock researcher",
            "label": "@mock-researcher",
            "start": 3,
            "end": 19,
            "cardUrl": "http://mock-researcher:8080/.well-known/agent-card.json"
        }]}))
    );
    assert!(
        call.activates(MENTIONS_EXTENSION),
        "{:?}",
        call.extensions_header
    );
    assert!(
        call.message_extensions
            .iter()
            .any(|u| u == MENTIONS_EXTENSION),
        "{:?}",
        call.message_extensions
    );
    // the text is not rewritten: the label is in it, where the offsets say
    assert_eq!(call.text, TEXT);
    let units: Vec<u16> = call.text.encode_utf16().collect();
    assert_eq!(
        String::from_utf16(&units[3..19]).unwrap(),
        "@mock-researcher"
    );
    // no `coordinate`: the card does not list thread-tools/v1
    assert!(call.mentions.unwrap().get("coordinate").is_none());
}

#[tokio::test]
async fn an_agent_whose_card_does_not_list_it_is_sent_the_text_as_it_is() {
    for listed in [
        vec![],
        vec![THREAD_TOOLS_EXTENSION],
        // a near miss is not the extension
        vec!["https://agents.vymalo.com/a2a/extensions/mentions/v2"],
        vec!["https://agents.vymalo.com/a2a/extensions/mentions/v1/"],
    ] {
        let fake = agent(&listed).await;
        let call = send(&client(true), &fake, request(&fake, vec![researcher()])).await;
        assert_eq!(call.mentions, None, "{listed:?}");
        assert!(!call.activates(MENTIONS_EXTENSION), "{listed:?}");
        assert!(
            !call
                .message_extensions
                .iter()
                .any(|u| u == MENTIONS_EXTENSION),
            "{listed:?}"
        );
        assert_eq!(call.text, TEXT, "the labels stay in the text");
    }
}

#[tokio::test]
async fn a_message_that_mentions_nobody_carries_nothing_even_to_an_agent_that_lists_it() {
    let fake = agent(&[MENTIONS_EXTENSION]).await;
    let call = send(&client(false), &fake, request(&fake, vec![])).await;
    assert_eq!(call.mentions, None);
    assert!(!call.activates(MENTIONS_EXTENSION));
}

#[tokio::test]
async fn a_mention_that_could_not_be_resolved_goes_with_its_id_label_and_offsets_only() {
    let fake = agent(&[MENTIONS_EXTENSION]).await;
    let gone = MentionInfo {
        name: None,
        card_url: None,
        ..researcher()
    };
    let call = send(&client(false), &fake, request(&fake, vec![gone])).await;
    assert_eq!(
        call.mentions.unwrap()["mentions"][0],
        json!({"agentId": "mock-researcher", "label": "@mock-researcher", "start": 3, "end": 19})
    );
}

#[tokio::test]
async fn coordinate_is_added_only_with_thread_tools_a_grant_and_a_tool_to_call() {
    // the card lists both, the grant is minted, the endpoint offers ask_agent
    let fake = agent(&[MENTIONS_EXTENSION, THREAD_TOOLS_EXTENSION]).await;
    let call = send(&client(true), &fake, request(&fake, vec![researcher()])).await;
    assert_eq!(
        call.mentions.unwrap()["coordinate"],
        json!({"tool": "ask_agent"})
    );
    assert!(call.thread_tools.is_some());

    // ask_agent is not mounted: the references, and no way to ask
    let fake = agent(&[MENTIONS_EXTENSION, THREAD_TOOLS_EXTENSION]).await;
    let call = send(&client(false), &fake, request(&fake, vec![researcher()])).await;
    assert!(call.mentions.unwrap().get("coordinate").is_none());

    // the card lists mentions only: no thread tools, no coordinate
    let fake = agent(&[MENTIONS_EXTENSION]).await;
    let call = send(&client(true), &fake, request(&fake, vec![researcher()])).await;
    assert!(call.mentions.unwrap().get("coordinate").is_none());

    // the card lists both but no grant was made for the request (the verifier's)
    let fake = agent(&[MENTIONS_EXTENSION, THREAD_TOOLS_EXTENSION]).await;
    let mut req = request(&fake, vec![researcher()]);
    req.thread_tools = None;
    let call = send(&client(true), &fake, req).await;
    assert!(call.mentions.unwrap().get("coordinate").is_none());
}

#[tokio::test]
async fn the_offsets_of_the_first_task_of_a_fork_move_past_the_history_in_front_of_the_text() {
    let fake = agent(&[MENTIONS_EXTENSION]).await;
    let history = ForkHistory {
        entries: vec![HistoryEntry {
            role: HistoryRole::Person,
            name: "person".to_owned(),
            text: "hello \u{1F680}".to_owned(),
        }],
        omitted: 0,
    };
    let in_front = utf16_len(&history_preamble(&history));
    let mut req = request(&fake, vec![researcher()]);
    req.history = Some(history);
    let call = send(&client(false), &fake, req).await;
    // what the agent receives is the history and then the text, in one part
    assert!(call.text.ends_with(TEXT));
    let told = &call.mentions.unwrap()["mentions"][0];
    let (start, end) = (
        usize::try_from(told["start"].as_u64().unwrap()).unwrap(),
        usize::try_from(told["end"].as_u64().unwrap()).unwrap(),
    );
    assert_eq!((start, end), (3 + in_front, 19 + in_front));
    let units: Vec<u16> = call.text.encode_utf16().collect();
    assert_eq!(
        String::from_utf16(&units[start..end]).unwrap(),
        "@mock-researcher"
    );
}
