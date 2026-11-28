//! The files of a message through the A2A adapter, over real HTTP (`docs/api/attachments.md`, "The
//! message side"): one URL part per file with its `mediaType` and `filename`, no bytes, and a
//! message whose files cannot all be sent that way reaches no agent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::future::BoxFuture;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{Classify, ErrorClass, ThreadId, UiActionData, UiVersion};
use orch_ports::testkit::agent_client::{LINK_SECRET, TestLinks, test_link};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentError, ArtifactError, ArtifactKey, ArtifactMeta,
    AttachmentLinks, PresignedGet, SendAttachment, SendContent, SendRequest,
};
use orch_testsupport::{FakeAgent, FakeAgentOptions};
use serde_json::json;

/// A store that signs nothing, failing for now or for good.
#[derive(Debug, Clone, Copy)]
struct NoLink {
    for_now: bool,
}

impl AttachmentLinks for NoLink {
    fn link<'a>(
        &'a self,
        _key: &'a ArtifactKey,
    ) -> BoxFuture<'a, Result<PresignedGet, ArtifactError>> {
        let for_now = self.for_now;
        Box::pin(async move {
            Err(if for_now {
                ArtifactError::unavailable("signing")
            } else {
                ArtifactError::Unsupported
            })
        })
    }
}

fn client(links: Option<Arc<dyn AttachmentLinks>>) -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        attachment_links: links,
        ..A2aConfig::default()
    })
    .unwrap()
}

fn file(media_type: &str, filename: Option<&str>, bytes: &[u8]) -> SendAttachment {
    SendAttachment {
        key: ArtifactMeta::of(media_type, None, bytes).key(ThreadId(uuid::Uuid::from_u128(1))),
        media_type: media_type.to_owned(),
        filename: filename.map(str::to_owned),
    }
}

fn request(ep: &AgentEndpoint, attachments: Vec<SendAttachment>) -> SendRequest {
    SendRequest {
        endpoint: ep.clone(),
        message_id: "msg-files".to_owned(),
        context_id: Some("ctx-1".to_owned()),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text("echo what is in these?".to_owned()),
        attachments,
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
        signed_prompt: None,
    }
}

/// The error of a send that must fail.
async fn refused(client: &A2aAgentClient, req: SendRequest) -> AgentError {
    match client.send_stream(req).await {
        Ok(_) => panic!("the message was sent"),
        Err(e) => e,
    }
}

#[tokio::test]
async fn the_agent_gets_the_text_then_one_url_part_per_file_and_no_bytes() {
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("fake", None);
    let image = file("image/png", Some("dot.png"), b"png bytes");
    let unnamed = file("application/pdf", None, b"pdf bytes");
    let mut stream = client(Some(Arc::new(TestLinks)))
        .send_stream(request(&ep, vec![image.clone(), unnamed.clone()]))
        .await
        .unwrap();
    while let Some(item) = tokio::time::timeout(Duration::from_secs(20), stream.next())
        .await
        .unwrap()
    {
        item.unwrap();
    }
    let call = fake.executions().pop().unwrap();
    // the whole of each part: a `raw` or `data` member, or any other, would fail this
    assert_eq!(
        call.parts,
        vec![
            json!({"text": "echo what is in these?"}),
            json!({"url": test_link(&image.key), "mediaType": "image/png", "filename": "dot.png"}),
            json!({"url": test_link(&unnamed.key), "mediaType": "application/pdf"}),
        ]
    );
}

#[tokio::test]
async fn a_message_with_files_that_get_no_link_reaches_no_agent() {
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("fake", None);
    let files = || vec![file("image/png", None, b"x")];
    // no way to make links at all, a store that never will, and a store that failed for now
    for (links, class) in [
        (None, ErrorClass::Internal),
        (
            Some(Arc::new(NoLink { for_now: false }) as Arc<dyn AttachmentLinks>),
            ErrorClass::Internal,
        ),
        (
            Some(Arc::new(NoLink { for_now: true }) as Arc<dyn AttachmentLinks>),
            ErrorClass::Transient,
        ),
    ] {
        let err = refused(&client(links), request(&ep, files())).await;
        assert_eq!(err.class(), class, "{err}");
    }
    assert!(fake.executions().is_empty());
}

#[tokio::test]
async fn files_on_anything_but_a_text_message_are_refused() {
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("fake", None);
    let mut req = request(&ep, vec![file("image/png", None, b"x")]);
    req.content = SendContent::UiAction {
        action: UiActionData {
            tier: None,
            tier_cap: None,
            surface_id: "s".into(),
            name: "go".into(),
            source_component_id: "go".into(),
            context: serde_json::Map::new(),
            version: UiVersion::V1_0,
            run_id: None,
        },
        at: jiff::Timestamp::UNIX_EPOCH,
    };
    let err = refused(&client(Some(Arc::new(TestLinks))), req).await;
    assert!(matches!(err, AgentError::Unsupported(_)), "{err}");
    assert!(!format!("{err} {err:?}").contains(LINK_SECRET));
    assert!(fake.executions().is_empty());
}

#[tokio::test]
async fn a_message_with_no_file_asks_for_no_link() {
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("fake", None);
    // a signer that fails for good would stop the send if it were asked
    client(Some(Arc::new(NoLink { for_now: false })))
        .send_stream(request(&ep, Vec::new()))
        .await
        .unwrap();
}
