//! `prompt-statement/v1` (ADR 0087, the cloud path; adam-rs ADR 0077): the statement is built and
//! signed at send time, from the parts the adapter sends, and only when the live card lists the
//! exact URI. A deployment with no signer signs nothing and every message is today's.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::future::Future;
use std::sync::{Arc, Mutex};

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::PROMPT_STATEMENT_EXTENSION;
use orch_ports::{
    AgentClient, AgentEndpoint, AttestedPayload, PromptRequest, PromptSigner, PromptSource,
    SendContent, SendRequest, SignedStatement, SignerError, SignerState,
};
use orch_prompt_statement::hash_of;
use orch_testsupport::{FakeAgent, FakeAgentOptions};
use serde_json::Value;

/// A signer that records each payload it is given and answers a fixed, well-formed compact JWS.
/// The payload names the person, so it is kept only for the test to read.
struct RecordingSigner {
    payloads: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl RecordingSigner {
    fn new() -> (Arc<Self>, Arc<Mutex<Vec<Vec<u8>>>>) {
        let payloads = Arc::new(Mutex::new(Vec::new()));
        (
            Arc::new(RecordingSigner {
                payloads: Arc::clone(&payloads),
            }),
            payloads,
        )
    }
}

impl PromptSigner for RecordingSigner {
    fn sign(
        &self,
        payload: &AttestedPayload,
    ) -> impl Future<Output = Result<SignedStatement, SignerError>> + Send {
        let payload = payload.as_bytes().to_vec();
        let log = Arc::clone(&self.payloads);
        async move {
            log.lock().expect("the recording").push(payload);
            Ok(SignedStatement::new("aGVhZA.cGF5bG9hZA.c2ln".to_owned()))
        }
    }

    fn state(&self) -> SignerState {
        SignerState::Ready
    }
}

fn client() -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap()
}

fn signed(signer: Arc<RecordingSigner>, aud: &str) -> A2aAgentClient {
    client()
        .with_prompt_signer(Some(signer))
        .with_statement_aud(aud.to_owned())
}

/// A fake agent whose card lists `prompt-statement/v1` when `offered`.
async fn agent(offered: bool) -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        card_extensions: if offered {
            vec![(PROMPT_STATEMENT_EXTENSION.to_owned(), Value::Null)]
        } else {
            Vec::new()
        },
        ..FakeAgentOptions::default()
    })
    .await
}

fn prompt() -> PromptRequest {
    PromptRequest {
        thread: "thread-1".to_owned(),
        sub: "alice@example.com".to_owned(),
        azp: "another-direct".to_owned(),
        src: PromptSource::Chat,
    }
}

fn request(ep: &AgentEndpoint, envelope: Option<&str>) -> SendRequest {
    SendRequest {
        endpoint: ep.clone(),
        message_id: "msg-1".to_owned(),
        context_id: Some("ctx-1".to_owned()),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text("echo hi".to_owned()),
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
        signed_prompt: envelope.map(|e| orch_core::SignedPrompt::envelope_only(e.to_owned())),
        prompt: Some(prompt()),
    }
}

/// Sends `req` and drains the stream, so that the agent's journal is complete.
async fn send(client: &A2aAgentClient, req: SendRequest) {
    let mut stream = client.send_stream(req).await.unwrap();
    while let Some(item) = stream.next().await {
        item.unwrap();
    }
}

#[tokio::test]
async fn a_signing_client_gets_a_statement_of_the_requests_parts() {
    let (signer, payloads) = RecordingSigner::new();
    let fake = agent(true).await;
    let ep = fake.endpoint("coder", None);
    send(
        &signed(signer, "https://models.test.invalid"),
        request(&ep, Some("eyJhbGciOiJFZERTQSJ9..c2ln")),
    )
    .await;

    // `sign` was called exactly once, with the statement's canonical payload.
    let payloads = payloads.lock().unwrap();
    assert_eq!(payloads.len(), 1, "one message, one statement");
    let statement: Value = serde_json::from_slice(&payloads[0]).unwrap();

    let calls = fake.calls();
    let [call] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    // The statement names the person, the token's client, the run's thread and agent, and what
    // started the run; `aud` is the deployment's gateway origin.
    assert_eq!(statement["typ"], "prompt-statement/v1");
    assert_eq!(statement["aud"], "https://models.test.invalid");
    assert_eq!(statement["sub"], "alice@example.com");
    assert_eq!(statement["azp"], "another-direct");
    assert_eq!(statement["thread"], "thread-1");
    assert_eq!(statement["agent"], "coder");
    assert_eq!(statement["src"], "chat");
    // The A2A ids of the message it was sent with.
    assert_eq!(statement["a2a"]["contextId"], "ctx-1");
    assert_eq!(statement["a2a"]["messageId"], "msg-1");
    // `content_sha256` is the hash of the parts exactly as the agent received them.
    assert_eq!(
        statement["content_sha256"].as_str(),
        Some(hash_of(call.message_parts.as_ref().expect("the parts")).as_str()),
    );
    // The person's claims are not carried to the send path yet: the statement does not claim them.
    assert_eq!(statement["user"], Value::Null);
    // It lives at most a day.
    let iat = statement["iat"].as_u64().unwrap();
    let exp = statement["exp"].as_u64().unwrap();
    assert!(exp > iat && exp - iat <= 86_400, "{iat}..{exp}");

    // The metadata holds the statement and the person's envelope, byte for byte, and the URI is
    // activated on the request and in the message's own extensions.
    let metadata = call.message_metadata.as_ref().expect("metadata");
    let entry = &metadata[PROMPT_STATEMENT_EXTENSION];
    assert_eq!(entry["statement"], "aGVhZA.cGF5bG9hZA.c2ln");
    assert_eq!(entry["user"], "eyJhbGciOiJFZERTQSJ9..c2ln");
    assert!(call.activates(PROMPT_STATEMENT_EXTENSION));
    assert!(
        call.message_extensions
            .iter()
            .any(|e| e == PROMPT_STATEMENT_EXTENSION)
    );
}

#[tokio::test]
async fn without_a_signer_no_statement_and_no_extension() {
    let fake = agent(true).await;
    let ep = fake.endpoint("coder", None);
    send(&client(), request(&ep, None)).await;

    let calls = fake.calls();
    let [call] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    let metadata = call.message_metadata.as_ref().unwrap_or(&Value::Null);
    assert!(
        metadata.get(PROMPT_STATEMENT_EXTENSION).is_none(),
        "{metadata}"
    );
    assert!(!call.activates(PROMPT_STATEMENT_EXTENSION));
    assert!(
        !call
            .message_extensions
            .iter()
            .any(|e| e == PROMPT_STATEMENT_EXTENSION)
    );
}

#[tokio::test]
async fn a_card_without_the_extension_gets_no_statement() {
    let (signer, payloads) = RecordingSigner::new();
    let fake = agent(false).await;
    let ep = fake.endpoint("coder", None);
    send(
        &signed(signer, "https://models.test.invalid"),
        request(&ep, Some("eyJhbGciOiJFZERTQSJ9..c2ln")),
    )
    .await;

    assert!(payloads.lock().unwrap().is_empty(), "nothing is signed");
    let calls = fake.calls();
    let [call] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    let metadata = call.message_metadata.as_ref().unwrap_or(&Value::Null);
    assert!(
        metadata.get(PROMPT_STATEMENT_EXTENSION).is_none(),
        "{metadata}"
    );
    assert!(!call.activates(PROMPT_STATEMENT_EXTENSION));
}

#[tokio::test]
async fn a_message_with_no_client_is_signed_for_nobody() {
    let (signer, payloads) = RecordingSigner::new();
    let fake = agent(true).await;
    let ep = fake.endpoint("coder", None);
    let mut req = request(&ep, None);
    // no token named a client (the proxy header, an MCP static token): there is no statement
    req.prompt = None;
    send(&signed(signer, "https://models.test.invalid"), req).await;

    assert!(payloads.lock().unwrap().is_empty(), "nothing is signed");
    let calls = fake.calls();
    let [call] = calls.as_slice() else {
        panic!("{calls:?}")
    };
    let metadata = call.message_metadata.as_ref().unwrap_or(&Value::Null);
    assert!(
        metadata.get(PROMPT_STATEMENT_EXTENSION).is_none(),
        "{metadata}"
    );
    assert!(!call.activates(PROMPT_STATEMENT_EXTENSION));
}
