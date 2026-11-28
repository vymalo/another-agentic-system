//! ADR 0087, decision 3, on the REST fork route (ANR-1375): a fork that writes the person's words —
//! its first message (`after` with `text`) or the replacement an edit makes (`replace` with
//! `text`) — is a prompt, and a signing client's must carry `promptSignature`. The check is the
//! AG-UI run's own (`orch_api::check_prompt_signature`), it runs before anything is written, and the
//! envelope reaches the delegation byte for byte and is in no event and no export.
//!
//! A fork with no message writes no words and is never refused for a missing signature; a client
//! that is not listed, and a deployment that lists none, is never checked.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, Policy};
use orch_core::{AgentId, AgentTarget, SignedPrompt, ThreadId};
use orch_ports::memory::{
    MemoryArtifacts, MemoryAuth, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{
    AgentEndpoint, ClaimOrder, Commit, FixedRegistry, NoModel, OutboxItem, OutboxPayload, PortSet,
    Principal, Role, SystemClock, ThreadStore,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use uuid::Uuid;

const ALICE: &str = "alice@example.com";

/// A compact JWS of three base64url parts with a detached payload: what a signing app sends.
const ENVELOPE: &str =
    "eyJhbGciOiJFZERTQSIsImtpZCI6ImRldmljZS0xI3YxIn0..c2lnbmF0dXJlLW9mLXRoZS1wcm9tcHQ";

type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    MemoryAuth,
    MemoryArtifacts,
>;

struct Resp {
    status: u16,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }

    fn code(&self) -> Option<String> {
        self.json()["code"].as_str().map(str::to_owned)
    }
}

struct Harness {
    base: String,
    client: reqwest::Client,
    app: Arc<App<Stack>>,
    server: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn principal(email: &str, roles: &[&str], client_id: Option<&str>) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        client_id: client_id.map(str::to_owned),
        ..Principal::of(orch_core::UserId::new(email))
    }
}

impl Harness {
    /// `signing_clients` is the deployment's `auth.signingClients`, as the binary reads it. Tokens:
    /// `desktop` is issued to `desktop` (a signing client), `web` to `web-client` (not listed), and
    /// `alice` names no client at all (the proxy header's shape).
    async fn start(signing_clients: &[&str]) -> Self {
        let entry = AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new("plain"),
                "https://plain.example.com/.well-known/agent-card.json".to_owned(),
                None,
            ),
            name: "plain".to_owned(),
        };
        let directory = AgentDirectory::new(vec![entry]);
        let auth = MemoryAuth::new();
        auth.allow("desktop", principal(ALICE, &["user"], Some("desktop")));
        auth.allow("web", principal(ALICE, &["user"], Some("web-client")));
        auth.allow("alice", principal(ALICE, &["user"], None));
        // a token that names no client at all (an empty `azp`): a signing client's, fail closed
        auth.allow("anon-client", principal(ALICE, &["user"], Some("")));
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: MemoryArtifacts::new(),
                    store: MemoryStore::new(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: NoModel,
                    auth: auth.clone(),
                    registry: directory.fixed_registry(),
                    user_tools: orch_ports::NoUserTools::off(),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    policy: Policy::default(),
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let cfg = ApiConfig {
            signing_clients: signing_clients
                .iter()
                .map(|client| (*client).to_owned())
                .collect::<Vec<String>>(),
            ..ApiConfig::default()
        };
        let router = orch_api::router_with_surfaces(Arc::clone(&app), cfg, Vec::new());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            app,
            server,
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<&Value>,
    ) -> Resp {
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send().await.unwrap();
        Resp {
            status: resp.status().as_u16(),
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    async fn post(&self, path: &str, token: &str, body: &Value) -> Resp {
        self.send(reqwest::Method::POST, path, token, Some(body))
            .await
    }

    async fn get(&self, path: &str, token: &str) -> Resp {
        self.send(reqwest::Method::GET, path, token, None).await
    }

    /// A parent thread that has finished its turn, so that it can be forked and edited.
    async fn parent(&self) -> ThreadId {
        let id = self
            .app
            .create_thread(
                &principal(ALICE, &[], None),
                orch_app::NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("plain"),
                        release: None,
                    },
                    text: "echo the parent".to_owned(),
                },
            )
            .await
            .unwrap()
            .id;
        let store = &self.app.ports().store;
        let record = store.get_thread(None, id).await.unwrap().unwrap();
        store
            .commit(
                id,
                record.version,
                Commit {
                    new_state: orch_core::ThreadState::Done,
                    job: None,
                    events: vec![],
                    outbox: vec![],
                    binding: None,
                    now: jiff::Timestamp::now(),
                    lease: None,
                    watches: vec![],
                    timers: vec![],
                    inbox: None,
                    finishes_outbox: None,
                    title: None,
                    description: None,
                    sharing: None,
                    skip_unsent_delegates: false,
                },
            )
            .await
            .unwrap();
        id
    }

    /// How many threads the person holds: the "writes nothing" claim is about *any* thread, not
    /// only the id the caller chose.
    async fn thread_count(&self) -> usize {
        self.app
            .ports()
            .store
            .list_threads(
                Some(&orch_core::UserId::new(ALICE)),
                orch_ports::ThreadListing::default(),
            )
            .await
            .unwrap()
            .len()
    }

    async fn events(&self, id: ThreadId) -> usize {
        self.app
            .list_events(&orch_core::UserId::new(ALICE), id, 0, 1000)
            .await
            .unwrap()
            .len()
    }

    /// The outbox rows of `id`, claimed for the test (no dispatcher runs in this harness).
    async fn outbox(&self, id: ThreadId) -> Vec<OutboxItem> {
        self.app
            .ports()
            .store
            .claim_outbox(
                "fork-signature-test",
                jiff::Timestamp::now(),
                Duration::from_secs(30),
                1000,
                ClaimOrder::Fifo,
            )
            .await
            .unwrap()
            .into_iter()
            .filter(|item| item.thread_id == id)
            .collect()
    }

    /// The envelope of `id`'s one delegation, if it has one.
    async fn delegated_envelope(&self, id: ThreadId) -> Option<Option<SignedPrompt>> {
        self.outbox(id)
            .await
            .into_iter()
            .find_map(|item| match item.payload {
                OutboxPayload::Delegate { signed_prompt, .. } => Some(signed_prompt),
                _ => None,
            })
    }
}

fn new_id() -> String {
    Uuid::now_v7().to_string()
}

fn refused(resp: &Resp) {
    assert_eq!(resp.status, 403, "{}", String::from_utf8_lossy(&resp.body));
    let problem = resp.json();
    assert_eq!(problem["code"], "prompt_signature_invalid", "{problem}");
    assert_eq!(problem["reason"], "invalid", "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .unwrap()
            .contains("couldn't be verified"),
        "{problem}"
    );
    // the envelope is in no answer
    assert!(!problem.to_string().contains("c2lnbmF0dXJl"));
}

#[tokio::test]
async fn a_signing_clients_unsigned_fork_with_text_is_refused_and_writes_nothing() {
    let h = Harness::start(&["desktop"]).await;
    let parent = h.parent().await;
    let events = h.events(parent).await;
    let threads = h.thread_count().await;
    let fork = new_id();
    let resp = h
        .post(
            &format!("/api/threads/{parent}/fork"),
            "desktop",
            &json!({"after": 1, "text": "hello from a thief", "id": fork.as_str()}),
        )
        .await;
    refused(&resp);
    assert_eq!(
        h.thread_count().await,
        threads,
        "the refused fork wrote a thread"
    );
    // nothing was written: no thread with the id the caller chose, and the parent's log is as it was
    let read = h.get(&format!("/api/threads/{fork}"), "desktop").await;
    assert_eq!(read.status, 404, "the refused fork wrote a thread");
    assert_eq!(h.events(parent).await, events, "the parent's log changed");
    // and no delegation was written for it
    assert_eq!(
        h.outbox(ThreadId(Uuid::parse_str(&fork).unwrap()))
            .await
            .len(),
        0
    );
}

#[tokio::test]
async fn a_signing_clients_unsigned_edit_is_refused_and_writes_nothing() {
    let h = Harness::start(&["desktop"]).await;
    let parent = h.parent().await;
    let events = h.events(parent).await;
    let threads = h.thread_count().await;
    let fork = new_id();
    let resp = h
        .post(
            &format!("/api/threads/{parent}/fork"),
            "desktop",
            &json!({"replace": 1, "text": "rewritten by a thief", "id": fork.as_str()}),
        )
        .await;
    refused(&resp);
    assert_eq!(
        h.thread_count().await,
        threads,
        "the refused edit wrote a thread"
    );
    let read = h.get(&format!("/api/threads/{fork}"), "desktop").await;
    assert_eq!(read.status, 404, "the refused edit wrote a thread");
    assert_eq!(h.events(parent).await, events, "the parent's log changed");
}

#[tokio::test]
async fn a_signed_fork_and_edit_are_accepted_and_the_envelope_reaches_the_delegation() {
    let h = Harness::start(&["desktop"]).await;
    let parent = h.parent().await;

    // the fork's first message
    let fork = new_id();
    let resp = h
        .post(
            &format!("/api/threads/{parent}/fork"),
            "desktop",
            &json!({"after": 1, "text": "hello", "id": fork.as_str(), "promptSignature": ENVELOPE}),
        )
        .await;
    assert_eq!(resp.status, 201, "{}", String::from_utf8_lossy(&resp.body));
    let fork_id = ThreadId(Uuid::parse_str(&fork).unwrap());
    assert_eq!(
        h.delegated_envelope(fork_id).await,
        Some(Some(SignedPrompt::envelope_only(ENVELOPE.to_owned()))),
        "the envelope did not reach the delegation"
    );

    // the edit's replacement
    let edit = new_id();
    let resp = h
        .post(
            &format!("/api/threads/{parent}/fork"),
            "desktop",
            &json!({"replace": 1, "text": "hello again", "id": edit.as_str(), "promptSignature": ENVELOPE}),
        )
        .await;
    assert_eq!(resp.status, 201, "{}", String::from_utf8_lossy(&resp.body));
    let edit_id = ThreadId(Uuid::parse_str(&edit).unwrap());
    assert_eq!(
        h.delegated_envelope(edit_id).await,
        Some(Some(SignedPrompt::envelope_only(ENVELOPE.to_owned()))),
        "an edit's envelope did not reach the delegation"
    );

    // the envelope is in no export and no event, byte for byte only in the delegation
    for id in [fork_id, edit_id] {
        let export = h.get(&format!("/api/threads/{id}/export"), "desktop").await;
        assert_eq!(export.status, 200);
        let export = String::from_utf8_lossy(&export.body).to_string();
        assert!(!export.contains(ENVELOPE), "the envelope is in the export");
        assert!(
            !export.contains("promptSignature")
                && !export.contains("prompt_signature")
                && !export.contains("signedPrompt")
                && !export.contains("signed_prompt")
                && !export.contains("afterStopSignedPrompt"),
            "the export names the envelope"
        );
        let events = h
            .app
            .list_events(&orch_core::UserId::new(ALICE), id, 0, 1000)
            .await
            .unwrap();
        let events = serde_json::to_string(&events).unwrap();
        assert!(!events.contains(ENVELOPE), "the envelope is in an event");
    }
}

#[tokio::test]
async fn a_client_that_is_not_listed_is_never_checked() {
    let h = Harness::start(&["desktop"]).await;
    let parent = h.parent().await;
    // the negative control: the list names `desktop`, the request comes from `web-client` unsigned
    for body in [
        json!({"after": 1, "text": "hello"}),
        json!({"replace": 1, "text": "hello"}),
    ] {
        let resp = h
            .post(&format!("/api/threads/{parent}/fork"), "web", &body)
            .await;
        assert_eq!(
            resp.status,
            201,
            "{body}: {}",
            String::from_utf8_lossy(&resp.body)
        );
    }
    // and a credential that names no client at all (the proxy header's shape) is never checked
    let resp = h
        .post(
            &format!("/api/threads/{parent}/fork"),
            "alice",
            &json!({"after": 1, "text": "hello"}),
        )
        .await;
    assert_eq!(resp.status, 201);
}

#[tokio::test]
async fn a_fork_with_no_message_is_never_checked() {
    let h = Harness::start(&["desktop"]).await;
    let parent = h.parent().await;
    // a fork from here writes no words, so a signing client's body needs no envelope
    let resp = h
        .post(
            &format!("/api/threads/{parent}/fork"),
            "desktop",
            &json!({"after": 1}),
        )
        .await;
    assert_eq!(resp.status, 201, "{}", String::from_utf8_lossy(&resp.body));
}

#[tokio::test]
async fn a_signature_without_text_is_refused() {
    let h = Harness::start(&["desktop"]).await;
    let parent = h.parent().await;
    let resp = h
        .post(
            &format!("/api/threads/{parent}/fork"),
            "desktop",
            &json!({"after": 1, "promptSignature": ENVELOPE}),
        )
        .await;
    assert_eq!(resp.status, 400, "{}", String::from_utf8_lossy(&resp.body));
    assert!(resp.code().is_none(), "{}", resp.json());
}

#[tokio::test]
async fn a_deployment_that_lists_no_signing_client_asks_nobody() {
    let h = Harness::start(&[]).await;
    let parent = h.parent().await;
    let resp = h
        .post(
            &format!("/api/threads/{parent}/fork"),
            "desktop",
            &json!({"after": 1, "text": "hello"}),
        )
        .await;
    assert_eq!(resp.status, 201, "{}", String::from_utf8_lossy(&resp.body));
}

#[tokio::test]
async fn a_token_that_names_no_client_is_a_signing_client_and_is_refused_unsigned() {
    // `Some("")` is the token-with-no-client sentinel: a signing client's, fail closed.
    let h = Harness::start(&["desktop"]).await;
    let parent = h.parent().await;
    let fork = new_id();
    let resp = h
        .post(
            &format!("/api/threads/{parent}/fork"),
            "anon-client",
            &json!({"after": 1, "text": "hello from a thief", "id": fork.as_str()}),
        )
        .await;
    refused(&resp);
    let read = h.get(&format!("/api/threads/{fork}"), "anon-client").await;
    assert_eq!(read.status, 404, "the refused fork wrote a thread");
}

#[tokio::test]
async fn a_signing_clients_empty_text_is_still_the_persons_words() {
    let h = Harness::start(&["desktop"]).await;
    let parent = h.parent().await;
    let fork = new_id();
    let resp = h
        .post(
            &format!("/api/threads/{parent}/fork"),
            "desktop",
            &json!({"after": 1, "text": "", "id": fork.as_str()}),
        )
        .await;
    refused(&resp);
    let read = h.get(&format!("/api/threads/{fork}"), "desktop").await;
    assert_eq!(read.status, 404, "the refused fork wrote a thread");
}
