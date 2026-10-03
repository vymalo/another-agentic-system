//! The thread-tools grant through the A2A adapter (`thread-tools/v1`, ADR 0023, ADR 0008), against
//! an in-process A2A agent over real HTTP: the token is minted when a message is sent, only to an
//! agent whose live card lists the extension, for the thread the request names, and it is in the
//! message and nowhere else.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use jiff::Timestamp;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{
    AgentId, Caller, THREAD_TOOLS_EXTENSION, ThreadId, ToolsGrant, UI_CATALOG_EXTENSION,
};
use orch_ports::{AgentClient, AgentEndpoint, AgentStream, SendContent, SendRequest};
use orch_testsupport::{Call, FakeAgent, FakeAgentOptions};
use orch_thread_token::{ThreadToolsIssuer, ThreadToolsKeys, verify};
use secrecy::SecretString;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const BASE_URL: &str = "http://orchestrator:8080";

fn keys() -> ThreadToolsKeys {
    ThreadToolsKeys::new(SecretString::from(KEY.to_owned()), None).unwrap()
}

fn issuer() -> Arc<ThreadToolsIssuer> {
    Arc::new(ThreadToolsIssuer::new(keys(), BASE_URL, Duration::from_secs(7200)).unwrap())
}

fn client(issuer: Option<Arc<ThreadToolsIssuer>>) -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        thread_tools: issuer,
        ..A2aConfig::default()
    })
    .unwrap()
}

/// An agent whose card lists `extensions` (ours, by URI).
async fn agent(extensions: &[&str]) -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        extensions: extensions.iter().map(|u| (*u).to_owned()).collect(),
        ..FakeAgentOptions::default()
    })
    .await
}

fn thread() -> ThreadId {
    ThreadId(uuid::Uuid::from_u128(
        0x0192_7a4e_3b00_7000_8000_0000_0000_0001,
    ))
}

fn grant() -> ToolsGrant {
    ToolsGrant::main(thread(), 3, AgentId::new("coder"))
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

fn request(fake: &FakeAgent, message_id: &str, thread_tools: Option<ToolsGrant>) -> SendRequest {
    let ep: AgentEndpoint = fake.endpoint("coder", None);
    SendRequest {
        endpoint: ep,
        message_id: message_id.to_owned(),
        context_id: format!("ctx-{message_id}"),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text("echo hi".to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools,
        history: None,
        steer: false,
        mentions: Vec::new(),
    }
}

/// Sends the request and returns what the agent saw of it.
async fn send(client: &A2aAgentClient, fake: &FakeAgent, req: SendRequest) -> Call {
    drain(client.send_stream(req).await.unwrap()).await;
    fake.executions().pop().unwrap()
}

/// The fields of the grant the agent saw, read the way an agent reads them.
fn grant_of(call: &Call) -> (String, String, String) {
    let grant = call.thread_tools.clone().expect("a grant");
    let text = |name: &str| grant[name].as_str().unwrap().to_owned();
    assert_eq!(
        grant.as_object().unwrap().len(),
        3,
        "exactly url, token and expiresAt: {grant}"
    );
    (text("url"), text("token"), text("expiresAt"))
}

#[tokio::test]
async fn a_card_that_lists_the_extension_gets_the_endpoint_and_a_token_for_this_message() {
    let fake = agent(&[THREAD_TOOLS_EXTENSION]).await;
    let client = client(Some(issuer()));
    let before = Timestamp::now().as_second();
    let call = send(&client, &fake, request(&fake, "msg-1", Some(grant()))).await;
    let after = Timestamp::now().as_second();

    let (url, token, expires_at) = grant_of(&call);
    assert_eq!(
        url,
        format!("{BASE_URL}/thread-tools/{}/mcp", thread()),
        "the endpoint of the request's thread"
    );
    // the token is the contract's: the thread, the job, the agent, who calls, this message
    let claims = verify(&keys(), &token, Timestamp::now()).unwrap();
    assert_eq!(claims.thread, thread());
    assert_eq!(claims.job, 3);
    assert_eq!(claims.agent.as_str(), "coder");
    assert_eq!((claims.caller, claims.depth), (Caller::Main, 0));
    assert_eq!(claims.message_id, "msg-1");
    assert!((before..=after).contains(&claims.issued_at.as_second()));
    assert_eq!(
        claims.expires_at.as_second() - claims.issued_at.as_second(),
        7200
    );
    assert_eq!(expires_at, claims.expires_at.to_string());
    // the extension is activated by the header, as the others are
    assert!(
        call.activates(THREAD_TOOLS_EXTENSION),
        "{:?}",
        call.extensions_header
    );
}

fn servers() -> Vec<orch_core::AttachedServer> {
    vec![
        orch_core::AttachedServer {
            id: "docs".to_owned(),
            name: "Documentation".to_owned(),
            description: None,
        },
        orch_core::AttachedServer {
            id: "websearch".to_owned(),
            name: "Web search".to_owned(),
            description: Some("Search the web.".to_owned()),
        },
    ]
}

/// The servers attached to the thread (ADR 0024) ride in the same metadata as the endpoint, by
/// `server`, `name` and `description` when there is one, and never as anything that says where a
/// server is or how to reach it; with none attached the member is not there.
#[tokio::test]
async fn the_attached_servers_are_named_in_the_metadata_beside_the_endpoint_and_only_then() {
    let fake = agent(&[THREAD_TOOLS_EXTENSION]).await;
    let client = client(Some(issuer()));
    let call = send(
        &client,
        &fake,
        request(&fake, "msg-1", Some(grant().with_attached(servers()))),
    )
    .await;
    let metadata = call.thread_tools.clone().expect("a grant");
    assert_eq!(
        metadata["attached"],
        serde_json::json!([
            {"server": "docs", "name": "Documentation"},
            {"server": "websearch", "name": "Web search", "description": "Search the web."},
        ])
    );
    assert_eq!(
        metadata.as_object().unwrap().len(),
        4,
        "url, token, expiresAt and attached: {metadata}"
    );
    assert_eq!(call.attached().len(), 2);
    // the only URL in it is the endpoint's
    let text = metadata.to_string();
    assert_eq!(text.matches("http").count(), 1, "{text}");

    // none attached: no member, so an agent that never heard of it reads what it always read
    let none = send(&client, &fake, request(&fake, "msg-2", Some(grant()))).await;
    assert!(
        none.thread_tools
            .as_ref()
            .unwrap()
            .get("attached")
            .is_none()
    );
    assert!(none.attached().is_empty());
    let (_, _, _) = grant_of(&none);
}

/// A card without the extension is told nothing, servers attached or not: the message is the one
/// it got before the extension existed (ADR 0008, fail closed).
#[tokio::test]
async fn a_card_without_the_extension_is_not_told_the_servers_either() {
    let keyed = client(Some(issuer()));
    for listed in [vec![], vec![UI_CATALOG_EXTENSION]] {
        let fake = agent(&listed).await;
        let call = send(
            &keyed,
            &fake,
            request(&fake, "msg-1", Some(grant().with_attached(servers()))),
        )
        .await;
        assert_eq!(call.thread_tools, None, "{listed:?}");
        assert!(call.attached().is_empty());
        assert!(call.extensions_header.is_empty(), "{listed:?}");
    }
    // and an adapter with no keys gives no grant, so nothing is told
    let fake = agent(&[THREAD_TOOLS_EXTENSION]).await;
    let call = send(
        &client(None),
        &fake,
        request(&fake, "msg-2", Some(grant().with_attached(servers()))),
    )
    .await;
    assert_eq!(call.thread_tools, None);
}

#[tokio::test]
async fn every_message_gets_a_token_of_its_own() {
    let fake = agent(&[THREAD_TOOLS_EXTENSION]).await;
    let client = client(Some(issuer()));
    let first = send(&client, &fake, request(&fake, "msg-1", Some(grant()))).await;
    let second = send(&client, &fake, request(&fake, "msg-2", Some(grant()))).await;
    let (_, token_1, _) = grant_of(&first);
    let (_, token_2, _) = grant_of(&second);
    assert_ne!(token_1, token_2);
    let now = Timestamp::now();
    assert_eq!(verify(&keys(), &token_1, now).unwrap().message_id, "msg-1");
    assert_eq!(verify(&keys(), &token_2, now).unwrap().message_id, "msg-2");
}

#[tokio::test]
async fn a_card_without_the_exact_uri_gets_the_message_it_got_before_the_extension() {
    let near_misses = [
        "https://agents.vymalo.com/a2a/extensions/thread-tools/v2",
        "https://agents.vymalo.com/a2a/extensions/thread-tools/v1/",
        "http://agents.vymalo.com/a2a/extensions/thread-tools/v1",
        "https://agents.vymalo.com/a2a/extensions/Thread-Tools/v1",
        "https://agents.vymalo.com/a2a/extensions/thread-tools",
    ];
    let client = client(Some(issuer()));
    let mut cards: Vec<Vec<&str>> = vec![vec![], vec![UI_CATALOG_EXTENSION]];
    cards.extend(near_misses.iter().map(|uri| vec![*uri]));
    for listed in cards {
        let fake = agent(&listed).await;
        let call = send(&client, &fake, request(&fake, "msg-1", Some(grant()))).await;
        assert_eq!(call.thread_tools, None, "{listed:?}");
        assert!(!call.activates(THREAD_TOOLS_EXTENSION), "{listed:?}");
        assert!(call.extensions_header.is_empty(), "{listed:?}");
    }
}

#[tokio::test]
async fn no_issuer_and_no_grant_mean_no_token_whatever_the_card_says() {
    let fake = agent(&[THREAD_TOOLS_EXTENSION]).await;
    // an adapter that has no keys (the default) gives no grant
    let without_issuer = client(None);
    let call = send(
        &without_issuer,
        &fake,
        request(&fake, "msg-1", Some(grant())),
    )
    .await;
    assert_eq!(call.thread_tools, None);
    assert!(!call.activates(THREAD_TOOLS_EXTENSION));
    // a request that names no thread (the verifier's) gets none either
    let with_issuer = client(Some(issuer()));
    let call = send(&with_issuer, &fake, request(&fake, "msg-2", None)).await;
    assert_eq!(call.thread_tools, None);
    assert!(!call.activates(THREAD_TOOLS_EXTENSION));
    // and the same request with a grant does get one
    let call = send(&with_issuer, &fake, request(&fake, "msg-3", Some(grant()))).await;
    assert!(call.thread_tools.is_some());
}

#[tokio::test]
async fn the_card_is_read_for_every_message_and_never_remembered() {
    let fake = agent(&[THREAD_TOOLS_EXTENSION]).await;
    let client = client(Some(issuer()));
    assert!(
        send(&client, &fake, request(&fake, "msg-1", Some(grant())))
            .await
            .thread_tools
            .is_some()
    );
    // the agent drops the extension: the very next message gets nothing
    fake.set_extensions(&[]);
    let call = send(&client, &fake, request(&fake, "msg-2", Some(grant()))).await;
    assert_eq!(call.thread_tools, None);
    assert!(!call.activates(THREAD_TOOLS_EXTENSION));
    // and takes it up again
    fake.set_extensions(&[THREAD_TOOLS_EXTENSION]);
    assert!(
        send(&client, &fake, request(&fake, "msg-3", Some(grant())))
            .await
            .thread_tools
            .is_some()
    );
}

#[tokio::test]
async fn an_asked_agent_and_its_depth_are_in_the_claims() {
    let fake = agent(&[THREAD_TOOLS_EXTENSION]).await;
    let client = client(Some(issuer()));
    let ask = ToolsGrant {
        caller: Caller::Ask(2),
        depth: 1,
        agent: AgentId::new("researcher"),
        ..grant()
    };
    let call = send(&client, &fake, request(&fake, "msg-1", Some(ask))).await;
    let (_, token, _) = grant_of(&call);
    let claims = verify(&keys(), &token, Timestamp::now()).unwrap();
    assert_eq!((claims.caller, claims.depth), (Caller::Ask(2), 1));
    assert_eq!(claims.agent.as_str(), "researcher");
}
