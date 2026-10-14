//! The large files of a message through the A2A adapter (`uploads/v1`, ADR 0058, ADR 0008), against
//! an in-process A2A agent over real HTTP: the extension is activated, with the deployment's limits
//! in the message, only when the card read for this very message lists it and the thread tools it
//! rides on, a grant was minted for the message and the request's grant offers uploads; any other
//! message is the message it was.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{
    AgentId, Caller, THREAD_TOOLS_EXTENSION, ThreadId, ToolsGrant, UPLOADS_EXTENSION, UploadOffer,
};
use orch_ports::{AgentClient, AgentEndpoint, AgentStream, SendContent, SendRequest};
use orch_testsupport::{Call, FakeAgent, FakeAgentOptions};
use orch_thread_token::{ThreadToolsIssuer, ThreadToolsKeys};
use secrecy::SecretString;
use serde_json::json;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OFFER: UploadOffer = UploadOffer {
    max_bytes: 1_073_741_824,
    ttl_secs: 300,
};

fn issuer() -> Arc<ThreadToolsIssuer> {
    let keys = ThreadToolsKeys::new(SecretString::from(KEY.to_owned()), None).unwrap();
    Arc::new(
        ThreadToolsIssuer::new(keys, "http://orchestrator:8080", Duration::from_secs(7200))
            .unwrap(),
    )
}

fn client(issuer: Option<Arc<ThreadToolsIssuer>>) -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        thread_tools: issuer,
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

fn grant(offer: Option<UploadOffer>) -> ToolsGrant {
    ToolsGrant::main(
        ThreadId(uuid::Uuid::from_u128(
            0x0192_7a4e_3b00_7000_8000_0000_0000_0001,
        )),
        3,
        AgentId::new("coder"),
    )
    .with_uploads(offer)
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
        context_id: Some(format!("ctx-{message_id}")),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text("echo hi".to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools,
        history: None,
        steer: false,
        mentions: Vec::new(),
        tier: None,
    }
}

async fn send(client: &A2aAgentClient, fake: &FakeAgent, req: SendRequest) -> Call {
    drain(client.send_stream(req).await.unwrap()).await;
    fake.executions().pop().unwrap()
}

#[tokio::test]
async fn a_card_that_lists_the_extension_and_the_thread_tools_is_told_the_limits() {
    let fake = agent(&[UPLOADS_EXTENSION, THREAD_TOOLS_EXTENSION]).await;
    let client = client(Some(issuer()));
    let call = send(
        &client,
        &fake,
        request(&fake, "m-1", Some(grant(Some(OFFER)))),
    )
    .await;

    assert_eq!(
        call.uploads,
        Some(json!({"maxBytes": 1_073_741_824u64, "ttlSecs": 300}))
    );
    // activated by the header and by the message's own list, as the others are
    assert!(
        call.activates(UPLOADS_EXTENSION),
        "{:?}",
        call.extensions_header
    );
    assert!(
        call.message_extensions
            .contains(&UPLOADS_EXTENSION.to_owned())
    );
    // and the thread tools it rides on are there too
    assert!(call.thread_tools.is_some());
    assert!(call.activates(THREAD_TOOLS_EXTENSION));
}

#[tokio::test]
async fn a_card_that_lists_only_one_of_the_two_is_told_nothing_of_uploads() {
    for listed in [UPLOADS_EXTENSION, THREAD_TOOLS_EXTENSION] {
        let fake = agent(&[listed]).await;
        let call = send(
            &client(Some(issuer())),
            &fake,
            request(&fake, "m-1", Some(grant(Some(OFFER)))),
        )
        .await;
        assert!(call.uploads.is_none(), "{listed}");
        assert!(!call.activates(UPLOADS_EXTENSION), "{listed}");
        assert!(
            !call
                .message_extensions
                .contains(&UPLOADS_EXTENSION.to_owned())
        );
    }
    // a card that lists neither: the message is what it was
    let fake = agent(&[]).await;
    let call = send(
        &client(Some(issuer())),
        &fake,
        request(&fake, "m-1", Some(grant(Some(OFFER)))),
    )
    .await;
    assert!(call.uploads.is_none() && call.thread_tools.is_none());
    assert!(
        call.extensions_header.is_empty(),
        "{:?}",
        call.extensions_header
    );
}

#[tokio::test]
async fn without_an_offer_a_grant_or_an_issuer_nothing_is_said() {
    let fake = agent(&[UPLOADS_EXTENSION, THREAD_TOOLS_EXTENSION]).await;
    // the deployment offers none: the store cannot presign, or no limit is configured
    let call = send(
        &client(Some(issuer())),
        &fake,
        request(&fake, "m-1", Some(grant(None))),
    )
    .await;
    assert!(call.uploads.is_none() && !call.activates(UPLOADS_EXTENSION));
    assert!(
        call.thread_tools.is_some(),
        "the thread tools are given as before"
    );
    // no grant on the request (the verifier's)
    let call = send(&client(Some(issuer())), &fake, request(&fake, "m-2", None)).await;
    assert!(call.uploads.is_none() && call.thread_tools.is_none());
    // no issuer, so no token: the agent has no endpoint to ask for a slot on
    let call = send(
        &client(None),
        &fake,
        request(&fake, "m-3", Some(grant(Some(OFFER)))),
    )
    .await;
    assert!(call.uploads.is_none() && call.thread_tools.is_none());
    // an asked agent is offered nothing
    let ask = ToolsGrant {
        caller: Caller::Ask(1),
        depth: 1,
        ..grant(None)
    };
    let call = send(
        &client(Some(issuer())),
        &fake,
        request(&fake, "m-4", Some(ask)),
    )
    .await;
    assert!(call.uploads.is_none());
}

#[tokio::test]
async fn the_card_is_read_for_every_message_so_an_agent_that_drops_the_extension_is_told_nothing() {
    let fake = agent(&[UPLOADS_EXTENSION, THREAD_TOOLS_EXTENSION]).await;
    let client = client(Some(issuer()));
    let first = send(
        &client,
        &fake,
        request(&fake, "m-1", Some(grant(Some(OFFER)))),
    )
    .await;
    assert!(first.uploads.is_some());

    fake.set_extensions(&[THREAD_TOOLS_EXTENSION]);
    let second = send(
        &client,
        &fake,
        request(&fake, "m-2", Some(grant(Some(OFFER)))),
    )
    .await;
    assert!(second.uploads.is_none() && !second.activates(UPLOADS_EXTENSION));
    assert!(second.thread_tools.is_some());

    fake.set_extensions(&[UPLOADS_EXTENSION, THREAD_TOOLS_EXTENSION]);
    let third = send(
        &client,
        &fake,
        request(&fake, "m-3", Some(grant(Some(OFFER)))),
    )
    .await;
    assert!(third.uploads.is_some());
}

#[tokio::test]
async fn a_near_miss_of_the_uri_is_not_the_extension() {
    for near in [
        "https://agents.vymalo.com/a2a/extensions/uploads/v2",
        "https://agents.vymalo.com/a2a/extensions/uploads/v1/",
        "https://agents.vymalo.com/a2a/extensions/Uploads/v1",
        "http://agents.vymalo.com/a2a/extensions/uploads/v1",
    ] {
        let fake = agent(&[near, THREAD_TOOLS_EXTENSION]).await;
        let call = send(
            &client(Some(issuer())),
            &fake,
            request(&fake, "m-1", Some(grant(Some(OFFER)))),
        )
        .await;
        assert!(call.uploads.is_none(), "{near}");
    }
}
