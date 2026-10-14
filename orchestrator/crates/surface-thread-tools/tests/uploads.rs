//! `request_upload` through the endpoint (`uploads/v1`, ADR 0058, `docs/api/uploads-v1.md` section
//! 3): over real HTTP with the token the adapter mints, on the in-memory store that plays a bucket
//! that presigns. The tool is listed to the addressed agent only while the deployment offers uploads
//! and the agent's live card lists the extension; a call gives a presigned `PUT` and an id; and every
//! way it is refused is a tool error that starts with the contract's code, never a protocol error.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, FileLimits, Inbound, NewThread, UploadSettings,
};
use orch_core::{AgentId, AgentTarget, KnownExtension, ThreadId, UserId};
use orch_ports::memory::{MemoryArtifacts, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{AgentEndpoint, ArtifactStore, FixedRegistry, IdGen, PortSet, SystemClock};
use orch_surface_thread_tools::{ThreadToolsConfig, UploadTools};
use orch_thread_token::{Claims, verify_upload_id};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{ALICE, call, claims, connect, keys, token, tool_names};
use tokio::task::JoinHandle;

type UploadPorts<X> = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    orch_ports::NoModel,
    FixedRegistry,
    orch_auth_header::HeaderAuth,
    X,
>;

const GIB: u64 = 1 << 30;
const PDF: &[u8] = b"%PDF-1.7 a report that is too large to go in a message";

fn hash_of(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// What the endpoint serves over: whether the deployment offers uploads, what the store can do and
/// what the agent's card lists.
struct Rig<X: ArtifactStore> {
    app: Arc<App<UploadPorts<X>>>,
    agent: ScriptedAgent,
    base: String,
    server: JoinHandle<()>,
}

impl<X: ArtifactStore> Drop for Rig<X> {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn rig<X: ArtifactStore>(
    store: X,
    uploads: Option<UploadSettings>,
    files: FileLimits,
) -> Rig<X> {
    let entry = |id: &str| AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: id.to_owned(),
    };
    let directory = AgentDirectory::new(vec![entry("plain"), entry("coder")]);
    let agent = ScriptedAgent::new();
    agent.set_extensions(
        "plain",
        &[KnownExtension::Uploads, KnownExtension::ThreadTools],
    );
    let app = Arc::new(
        App::new(
            PortSet {
                artifacts: store,
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: agent.clone(),
                clock: SystemClock,
                ids: SeqIds::default(),
                model: orch_ports::NoModel,
                auth: orch_auth_header::HeaderAuth::new(),
                registry: directory.fixed_registry(),
            },
            directory,
            AppConfig {
                files,
                uploads,
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let config = ThreadToolsConfig::new(keys(), ["127.0.0.1", "localhost"])
        .unwrap()
        .with_provider(UploadTools::new(Arc::clone(&app)));
    let router = orch_api::router_with_surfaces(
        Arc::clone(&app),
        ApiConfig::default(),
        vec![orch_surface_thread_tools::routes(Arc::clone(&app), config)],
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Rig {
        app,
        agent,
        base: format!("http://{addr}"),
        server,
    }
}

impl<X: ArtifactStore> Rig<X> {
    /// A thread of Alice's addressed to `plain`, `queued`: no dispatcher runs, so it stays open.
    async fn thread(&self) -> ThreadId {
        let id = ThreadId(SeqIds::default().new_id());
        self.app
            .create_thread_as(
                &UserId::new(ALICE),
                id,
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("plain"),
                        release: None,
                    },
                    text: "files please".to_owned(),
                },
                Inbound::default(),
            )
            .await
            .unwrap();
        id
    }

    fn url(&self, thread: ThreadId) -> String {
        format!("{}/thread-tools/{thread}/mcp", self.base)
    }

    /// A client with the addressed agent's token for job `job`.
    async fn client(&self, thread: ThreadId, job: u32) -> support::Client {
        let claims = Claims {
            job,
            ..claims(thread, "plain")
        };
        connect(&self.url(thread), &token(&keys(), &claims)).await
    }
}

fn args(bytes: &[u8]) -> Value {
    json!({
        "filename": "report.pdf",
        "mediaType": "application/pdf",
        "size": bytes.len(),
        "sha256": hash_of(bytes),
    })
}

#[tokio::test]
async fn the_tool_is_listed_to_the_addressed_agent_while_the_deployment_offers_uploads_and_the_card_lists_them()
 {
    let rig = rig(
        MemoryArtifacts::presigning(),
        Some(UploadSettings::new(GIB, Duration::from_secs(300), keys()).unwrap()),
        FileLimits::default(),
    )
    .await;
    let thread = rig.thread().await;
    let client = rig.client(thread, 1).await;
    let names = tool_names(&client).await;
    assert!(names.contains(&"request_upload".to_owned()), "{names:?}");
    // beside the built-in ones, first
    assert_eq!(&names[..2], ["get_ui_catalog", "turn_output"]);
    let tool = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .find(|t| t.name == "request_upload")
        .unwrap();
    assert_eq!(tool.input_schema.get("type"), Some(&json!("object")));
    assert_eq!(
        tool.input_schema.get("required"),
        Some(&json!(["filename", "mediaType", "size", "sha256"]))
    );
    assert!(tool.output_schema.is_some());

    // the card is read for every list: an agent that drops the extension loses the tool at once, and
    // gets it back when it lists it again
    rig.agent
        .set_extensions("plain", &[KnownExtension::ThreadTools]);
    assert!(
        !tool_names(&client)
            .await
            .contains(&"request_upload".to_owned())
    );
    rig.agent.set_extensions(
        "plain",
        &[KnownExtension::Uploads, KnownExtension::ThreadTools],
    );
    assert!(
        tool_names(&client)
            .await
            .contains(&"request_upload".to_owned())
    );
    // a card that cannot be read is not a card that lists them
    rig.agent.set_card_down("plain", true);
    assert!(
        !tool_names(&client)
            .await
            .contains(&"request_upload".to_owned())
    );
}

#[tokio::test]
async fn the_tool_is_not_listed_without_an_offer_and_a_call_says_why() {
    // no limit configured, and a store that cannot presign: nothing is offered
    for (store_presigns, uploads, code) in [
        (true, None, "not_activated"),
        (
            false,
            Some(UploadSettings::new(GIB, Duration::from_secs(300), keys()).unwrap()),
            "unsupported",
        ),
    ] {
        let store = if store_presigns {
            MemoryArtifacts::presigning()
        } else {
            MemoryArtifacts::new()
        };
        let rig = rig(store, uploads, FileLimits::default()).await;
        let thread = rig.thread().await;
        let client = rig.client(thread, 1).await;
        assert!(
            !tool_names(&client)
                .await
                .contains(&"request_upload".to_owned())
        );
        // a call by name is still answered, with the reason, as a tool error
        let outcome = call(&client, "request_upload", args(PDF)).await;
        assert!(outcome.is_error);
        assert!(
            outcome.text.starts_with(&format!("{code}: ")),
            "{}",
            outcome.text
        );
    }
}

#[tokio::test]
async fn a_call_gives_a_presigned_put_for_the_threads_key_and_an_id_the_dispatcher_can_read() {
    let store = MemoryArtifacts::presigning();
    let rig = rig(
        store.clone(),
        Some(UploadSettings::new(GIB, Duration::from_secs(300), keys()).unwrap()),
        FileLimits::default(),
    )
    .await;
    let thread = rig.thread().await;
    let client = rig.client(thread, 1).await;
    let before = Timestamp::now().as_second();

    let outcome = call(&client, "request_upload", args(PDF)).await;

    assert!(!outcome.is_error, "{}", outcome.text);
    let slot = &outcome.value;
    assert_eq!(slot["method"], "PUT");
    let key = format!("threads/{thread}/{}", hash_of(PDF));
    assert!(slot["url"].as_str().unwrap().contains(&key), "{slot}");
    assert_eq!(slot["headers"]["content-type"], "application/pdf");
    let expires: Timestamp = slot["expiresAt"].as_str().unwrap().parse().unwrap();
    assert!(
        (before + 299..=before + 305).contains(&expires.as_second()),
        "{expires}"
    );
    // the id says the thread, the job, the content, and is signed with the thread-tools key
    let id = verify_upload_id(&keys(), slot["uploadId"].as_str().unwrap()).unwrap();
    assert_eq!((id.thread, id.job, id.size), (thread, 1, PDF.len() as u64));
    assert_eq!(id.expires_at, expires);
    // the text is the same JSON as the structured content
    let text: Value = serde_json::from_str(&outcome.text).unwrap();
    assert_eq!(&text, slot);
    // nothing was written: not to the store, not to the log
    assert!(store.is_empty());
    let events = rig
        .app
        .list_events(&UserId::new(ALICE), thread, 0, 100)
        .await
        .unwrap();
    let logged = serde_json::to_string(&events).unwrap();
    assert!(
        !logged.contains(slot["uploadId"].as_str().unwrap()) && !logged.contains("memory://"),
        "{logged}"
    );
}

#[tokio::test]
async fn every_refusal_is_a_tool_error_that_starts_with_the_contracts_code() {
    let store = MemoryArtifacts::presigning();
    // a job keeps at most four bytes of files, and one file may be at most 1 KiB
    let rig = rig(
        store,
        Some(UploadSettings::new(1024, Duration::from_secs(300), keys()).unwrap()),
        FileLimits {
            max_file_bytes: 1024,
            max_per_job_bytes: 4,
            max_files_per_job: 1,
        },
    )
    .await;
    let thread = rig.thread().await;
    let client = rig.client(thread, 1).await;
    let refused = |outcome: &support::Outcome, code: &str| {
        assert!(outcome.is_error, "{outcome:?}");
        assert!(
            outcome.text.starts_with(&format!("{code}: ")),
            "{code}: {}",
            outcome.text
        );
    };

    // over the most one file may be (a size, not bytes: nothing is sent)
    let too_large = json!({"filename": "big.bin", "mediaType": "application/octet-stream",
                           "size": 1025, "sha256": "ab".repeat(32)});
    let outcome = call(&client, "request_upload", too_large).await;
    refused(&outcome, "too_large");
    assert!(outcome.text.contains("1024"), "{}", outcome.text);

    // the job has room for four bytes, and this is five
    refused(
        &call(&client, "request_upload", args(b"small")).await,
        "over_limit",
    );

    // a member missing, mistyped, unknown, or a value the contract refuses
    for bad in [
        json!({}),
        json!({"filename": "a.pdf", "mediaType": "application/pdf", "size": 5}),
        json!({"filename": 7, "mediaType": "application/pdf", "size": 5, "sha256": hash_of(b"x")}),
        json!({"filename": "a.pdf", "mediaType": "application/pdf", "size": 0, "sha256": hash_of(b"x")}),
        json!({"filename": "a.pdf", "mediaType": "pdf", "size": 5, "sha256": hash_of(b"x")}),
        json!({"filename": "a.pdf", "mediaType": "application/pdf", "size": 5, "sha256": "ZZ"}),
        json!({"filename": "..", "mediaType": "application/pdf", "size": 5, "sha256": hash_of(b"x")}),
        json!({"filename": "a.pdf", "mediaType": "application/pdf", "size": 5, "sha256": hash_of(b"x"), "extra": true}),
    ] {
        refused(
            &call(&client, "request_upload", bad.clone()).await,
            "bad_request",
        );
    }

    // a card that no longer lists the extension: not activated
    rig.agent.set_extensions("plain", &[]);
    refused(
        &call(&client, "request_upload", args(b"tiny")).await,
        "not_activated",
    );
    rig.agent.set_extensions(
        "plain",
        &[KnownExtension::Uploads, KnownExtension::ThreadTools],
    );

    // a token of an earlier job: the task is over
    let stale = rig.client(thread, 2).await;
    refused(
        &call(&stale, "request_upload", args(b"tiny")).await,
        "task_over",
    );

    // none of it wrote anything
    let events = rig
        .app
        .list_events(&UserId::new(ALICE), thread, 0, 100)
        .await
        .unwrap();
    assert_eq!(events.len(), 1, "only the person's message: {events:?}");
}

#[tokio::test]
async fn an_agent_that_is_not_the_threads_gets_nothing() {
    // a token for another agent than the thread's is the guard's 401, before any tool runs
    let rig = rig(
        MemoryArtifacts::presigning(),
        Some(UploadSettings::new(GIB, Duration::from_secs(300), keys()).unwrap()),
        FileLimits::default(),
    )
    .await;
    let thread = rig.thread().await;
    let claims = claims(thread, "coder");
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = http
        .post(rig.url(thread))
        .bearer_auth(token(&keys(), &claims))
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn an_asked_agent_is_offered_nothing_and_refused_a_slot() {
    // The orchestrator keeps no file of an asked agent: it is not offered the tool, and a call by
    // name is `caller_not_allowed` whatever the deployment offers.
    use orch_app::AskCall;
    use orch_core::Caller;
    let h = support::Harness::start_full(keys(), AppConfig::default(), |app, config| {
        config.with_provider(UploadTools::new(Arc::clone(app)))
    })
    .await;
    let thread = h.asking_thread("plain", "slow", &["coder"]).await;
    h.app
        .ask(AskCall {
            thread,
            job: 1,
            caller: Caller::Main,
            asker: AgentId::new("plain"),
            agent: AgentId::new("coder"),
            text: "slow work".to_owned(),
            call_id: Some("c1".to_owned()),
            parent_step: None,
            timeout: None,
        })
        .await
        .unwrap();
    let coder = connect(&h.url(thread), &support::ask_token(thread, 1, "coder", 1)).await;
    assert!(
        !tool_names(&coder)
            .await
            .contains(&"request_upload".to_owned())
    );
    let outcome = call(&coder, "request_upload", args(PDF)).await;
    assert!(outcome.is_error);
    assert!(
        outcome.text.starts_with("caller_not_allowed: "),
        "{}",
        outcome.text
    );
}
