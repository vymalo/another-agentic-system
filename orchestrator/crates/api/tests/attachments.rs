//! Attachments (ADR 0067, wave 2 slice 12, server half; contract `docs/api/attachments.md`): a
//! person sends a file to one of their own threads, same origin, as the raw body of one request,
//! and the orchestrator keeps it in the artifact store by its hash (ADR 0032) — never by a
//! presigned upload, which ADR 0058 gave the agents and never the browser.
//!
//! The rules this file pins, each in the orchestrator and not in a client (ADR 0067, rule 1):
//!
//! - the size is checked **before anything is stored**: a body over `artifacts.maxFileBytes` (10
//!   MiB by default) is a `413` and the store's `put` is never called (the counting store below is
//!   how that is a count and not a hope), and the very same body is kept when the deployment
//!   allows it, so the test can tell a limit that works from one that refuses everything;
//! - the store is addressed by the content's hash, so the same bytes sent twice are one object
//!   (and the two answers name the same hash);
//! - only the thread's owner writes an attachment to it and only the owner reads it back through
//!   the file route every file already has (ADR 0039): another person gets the same `404` for the
//!   upload and for the read.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, FileLimits, NewThread, Policy, RoleGrant,
};
use orch_core::{AgentId, AgentTarget, ThreadId, UserId};
use orch_ports::memory::{MemoryArtifacts, MemoryAuth, MemoryStore, MemoryWakeup, SeqIds};
use orch_ports::{
    AgentEndpoint, ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream,
    FixedRegistry, NoModel, PortSet, Principal, Role, SystemClock,
};
use serde_json::Value;
use tokio::task::JoinHandle;

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";
const MIB: u64 = 1024 * 1024;

/// A PNG's magic bytes: the type the store sniffs instead of the one the client declares.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01";

/// What the routes asked of the store: `put` is the only call that stores anything.
#[derive(Default)]
struct Counts {
    puts: AtomicUsize,
    /// `put` per key, so "stored once" is the number of keys and not the number of requests.
    by_key: Mutex<BTreeMap<String, usize>>,
}

/// The in-memory artifact store, counting what the attachment route did to it.
#[derive(Clone)]
struct Counting {
    inner: MemoryArtifacts,
    counts: Arc<Counts>,
}

impl Counting {
    fn new() -> Self {
        Counting {
            inner: MemoryArtifacts::new(),
            counts: Arc::default(),
        }
    }

    fn puts(&self) -> usize {
        self.counts.puts.load(Ordering::SeqCst)
    }

    fn objects(&self) -> usize {
        self.inner.len()
    }

    fn contains(&self, key: &ArtifactKey) -> bool {
        self.inner.contains(key)
    }
}

impl ArtifactStore for Counting {
    async fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
        self.counts.puts.fetch_add(1, Ordering::SeqCst);
        *self
            .counts
            .by_key
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_default() += 1;
        self.inner.put(key, bytes, meta).await
    }

    async fn get(
        &self,
        key: &ArtifactKey,
    ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
        self.inner.get(key).await
    }

    async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
        self.inner.delete(key).await
    }

    async fn copy(&self, from: &ArtifactKey, to: &ArtifactKey) -> Result<(), ArtifactError> {
        self.inner.copy(from, to).await
    }

    async fn delete_prefix(&self, thread: ThreadId) -> Result<u64, ArtifactError> {
        self.inner.delete_prefix(thread).await
    }
}

type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    orch_ports::memory::ScriptedAgent,
    SystemClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    MemoryAuth,
    Counting,
>;

struct Harness {
    base: String,
    client: reqwest::Client,
    app: Arc<App<Stack>>,
    bucket: Counting,
    server: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

struct Resp {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .unwrap_or_else(|| panic!("no {name} header in {:?}", self.headers))
            .to_str()
            .unwrap()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }

    /// The `Problem` of a refusal, its `code` beside it.
    fn problem(&self) -> (String, Value) {
        assert_eq!(
            self.header("content-type"),
            "application/problem+json",
            "a refusal is a problem: {}",
            String::from_utf8_lossy(&self.body)
        );
        let body = self.json();
        let code = body["code"].as_str().unwrap_or_default().to_owned();
        (code, body)
    }
}

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

impl Harness {
    /// A deployment whose `artifacts.maxFileBytes` is `max_file_bytes`.
    async fn start(max_file_bytes: u64) -> Self {
        let directory = AgentDirectory::new(vec![AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new("plain"),
                "https://plain.example.com/.well-known/agent-card.json",
                None,
            ),
            name: "plain".to_owned(),
        }]);
        let auth = MemoryAuth::new();
        auth.allow("alice", principal(ALICE, &["user"]));
        auth.allow("bob", principal(BOB, &["user"]));
        let store = MemoryStore::new();
        let bucket = Counting::new();
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: bucket.clone(),
                    store,
                    wakeup: MemoryWakeup::new(),
                    agents: orch_ports::memory::ScriptedAgent::new(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: NoModel,
                    auth,
                    registry: directory.fixed_registry(),
                    user_tools: orch_ports::NoUserTools::off(),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    files: FileLimits {
                        max_file_bytes,
                        ..FileLimits::default()
                    },
                    policy: Policy::new(
                        BTreeMap::from([(Role::new("user"), RoleGrant::user())]),
                        Some(Role::new("user")),
                    )
                    .unwrap(),
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router =
            orch_api::router_with_surfaces(Arc::clone(&app), ApiConfig::default(), Vec::new());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            app,
            bucket,
            server,
        }
    }

    /// A thread of `owner`'s, with its first message in the log.
    async fn thread(&self, owner: &str) -> ThreadId {
        self.app
            .create_thread(
                &UserId::new(owner),
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("plain"),
                        release: None,
                    },
                    text: "hi".to_owned(),
                },
            )
            .await
            .unwrap()
            .id
    }

    async fn upload(
        &self,
        thread: ThreadId,
        token: Option<&str>,
        content_type: Option<&str>,
        disposition: Option<&str>,
        bytes: Vec<u8>,
    ) -> Resp {
        let mut req = self
            .client
            .post(format!("{}/api/threads/{thread}/attachments", self.base))
            .body(bytes);
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        if let Some(kind) = content_type {
            req = req.header(reqwest::header::CONTENT_TYPE, kind);
        }
        if let Some(value) = disposition {
            req = req.header(reqwest::header::CONTENT_DISPOSITION, value);
        }
        let resp = req.send().await.unwrap();
        Resp {
            status: resp.status().as_u16(),
            headers: resp.headers().clone(),
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    async fn get(&self, path: &str, token: Option<&str>) -> Resp {
        let mut req = self.client.get(format!("{}{path}", self.base));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await.unwrap();
        Resp {
            status: resp.status().as_u16(),
            headers: resp.headers().clone(),
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }
}

/// The 11 MiB body of the size cases: incompressible enough that nothing collapses it.
fn eleven_mib() -> Vec<u8> {
    (0..11 * MIB).map(|i| (i % 251) as u8).collect()
}

#[tokio::test]
async fn a_body_over_the_limit_is_refused_before_anything_is_stored() {
    let h = Harness::start(10 * MIB).await;
    let thread = h.thread(ALICE).await;
    let before = h.bucket.puts();

    let resp = h
        .upload(
            thread,
            Some("alice"),
            Some("application/octet-stream"),
            Some("attachment; filename=\"big.bin\""),
            eleven_mib(),
        )
        .await;

    assert_eq!(resp.status, 413, "{}", String::from_utf8_lossy(&resp.body));
    let (code, problem) = resp.problem();
    assert_eq!(code, "too_large", "{problem}");
    assert_eq!(
        h.bucket.puts(),
        before,
        "the store was asked to keep a body that is over the limit"
    );
    assert_eq!(h.bucket.objects(), 0, "an object was stored");
}

/// The ceiling is the contract's, not the deployment's: a deployment that allows more still
/// refuses an 11 MiB body, and a body of exactly 10 MiB is kept.
#[tokio::test]
async fn a_deployment_may_lower_the_ceiling_and_never_raise_it() {
    let h = Harness::start(64 * MIB).await;
    let thread = h.thread(ALICE).await;

    let over = h
        .upload(
            thread,
            Some("alice"),
            Some("application/octet-stream"),
            Some("attachment; filename=\"big.bin\""),
            eleven_mib(),
        )
        .await;
    assert_eq!(over.status, 413, "{}", String::from_utf8_lossy(&over.body));
    assert_eq!(h.bucket.objects(), 0);

    let exact = h
        .upload(
            thread,
            Some("alice"),
            Some("application/octet-stream"),
            Some("attachment; filename=\"ten.bin\""),
            eleven_mib()[..(10 * MIB as usize)].to_vec(),
        )
        .await;
    assert_eq!(
        exact.status,
        201,
        "{}",
        String::from_utf8_lossy(&exact.body)
    );
    assert_eq!(exact.json()["size"], 10 * MIB);
    assert_eq!(h.bucket.objects(), 1);
}

#[tokio::test]
async fn the_same_bytes_are_one_object_and_the_answer_is_its_hash() {
    let h = Harness::start(10 * MIB).await;
    let thread = h.thread(ALICE).await;

    let first = h
        .upload(
            thread,
            Some("alice"),
            Some("application/octet-stream"),
            Some("attachment; filename=\"screenshot.png\""),
            PNG.to_vec(),
        )
        .await;
    assert_eq!(
        first.status,
        201,
        "{}",
        String::from_utf8_lossy(&first.body)
    );
    let kept = first.json();
    let sha = kept["sha256"].as_str().unwrap().to_owned();
    assert_eq!(sha.len(), 64);
    assert!(
        sha.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    // the type is the bytes' own, not the generic one the client declared
    assert_eq!(kept["mediaType"], "image/png");
    assert_eq!(kept["filename"], "screenshot.png");
    assert_eq!(kept["size"], PNG.len());
    assert_eq!(
        kept["url"],
        format!("/api/threads/{thread}/artifacts/{sha}"),
        "the client reads it where every file of a thread is read"
    );
    assert_eq!(
        first.header("location"),
        format!("/api/threads/{thread}/artifacts/{sha}")
    );
    let key = ArtifactKey::new(thread, hex(&sha));
    assert!(h.bucket.contains(&key));

    // the same bytes again: the same answer, and one object in the store
    let second = h
        .upload(
            thread,
            Some("alice"),
            Some("image/png"),
            Some("attachment; filename=\"shot.png\""),
            PNG.to_vec(),
        )
        .await;
    assert_eq!(second.status, 201);
    assert_eq!(second.json()["sha256"], sha.as_str());
    assert_eq!(h.bucket.objects(), 1, "the content's hash is the key");
    assert_eq!(
        h.bucket.counts.by_key.lock().unwrap().get(&key.to_string()),
        Some(&2),
        "both requests wrote the one object of that key"
    );

    // and the owner reads the bytes back through the file route that already existed
    let read = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}"),
            Some("alice"),
        )
        .await;
    assert_eq!(read.status, 200);
    assert_eq!(read.body, PNG);
    assert_eq!(read.header("content-type"), "image/png");
    assert!(
        read.header("content-disposition").starts_with("inline;"),
        "a PNG is a preview type: {}",
        read.header("content-disposition")
    );
    assert_eq!(
        read.header("cache-control"),
        "private, max-age=31536000, immutable"
    );
}

#[tokio::test]
async fn only_the_owner_writes_it_and_only_the_owner_reads_it() {
    let h = Harness::start(10 * MIB).await;
    let thread = h.thread(ALICE).await;

    // bob cannot put a file into alice's thread, and nothing is stored for the attempt
    let refused = h
        .upload(
            thread,
            Some("bob"),
            Some("application/octet-stream"),
            Some("attachment; filename=\"note.txt\""),
            b"not mine".to_vec(),
        )
        .await;
    assert_eq!(
        refused.status,
        404,
        "{}",
        String::from_utf8_lossy(&refused.body)
    );
    assert_eq!(h.bucket.puts(), 0, "a refused upload stored something");

    let kept = h
        .upload(
            thread,
            Some("alice"),
            Some("text/plain"),
            Some("attachment; filename=\"note.txt\""),
            b"mine".to_vec(),
        )
        .await;
    assert_eq!(kept.status, 201);
    let sha = kept.json()["sha256"].as_str().unwrap().to_owned();
    let path = format!("/api/threads/{thread}/artifacts/{sha}");

    let owner = h.get(&path, Some("alice")).await;
    assert_eq!(owner.status, 200);
    assert_eq!(owner.body, b"mine");

    let other = h.get(&path, Some("bob")).await;
    assert_eq!(
        other.status,
        404,
        "{}",
        String::from_utf8_lossy(&other.body)
    );
    assert!(other.body != b"mine");

    let nobody = h.get(&path, None).await;
    assert_eq!(nobody.status, 401);
}

#[tokio::test]
async fn an_empty_body_and_a_type_that_cannot_be_read_are_refused() {
    let h = Harness::start(10 * MIB).await;
    let thread = h.thread(ALICE).await;

    let empty = h
        .upload(thread, Some("alice"), Some("text/plain"), None, Vec::new())
        .await;
    assert_eq!(empty.status, 400);
    assert_eq!(empty.problem().0, "empty_body");

    let bad_type = h
        .upload(
            thread,
            Some("alice"),
            Some("not a media type"),
            None,
            b"x".to_vec(),
        )
        .await;
    assert_eq!(bad_type.status, 400);
    assert_eq!(bad_type.problem().0, "bad_media_type");

    let bad_name = h
        .upload(
            thread,
            Some("alice"),
            Some("text/plain"),
            Some("attachment; filename=\"a\\\"b"),
            b"x".to_vec(),
        )
        .await;
    assert_eq!(bad_name.status, 400);
    assert_eq!(bad_name.problem().0, "bad_filename");

    assert_eq!(h.bucket.puts(), 0, "a refused upload stored something");
}

/// The contract page the route is built from, and its row in the index (ADR 0067, rule 1: a
/// feature is implementable from `docs/api/` alone).
#[test]
fn the_contract_page_exists_and_is_indexed() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../docs/api");
    let page = std::fs::read_to_string(format!("{root}/attachments.md"))
        .expect("docs/api/attachments.md is the contract of this route");
    for member in [
        "POST /api/threads/{threadId}/attachments",
        "10 MiB",
        "413",
        "too_large",
        "Content-Disposition",
        "artifact store",
    ] {
        assert!(
            page.contains(member),
            "the contract says nothing of {member}"
        );
    }
    let index = std::fs::read_to_string(format!("{root}/README.md")).unwrap();
    assert!(
        index.contains("[`attachments.md`](attachments.md)"),
        "the contract page is not in the docs/api index"
    );
}

/// The hash of `hex`, as the key wants it.
fn hex(text: &str) -> [u8; 32] {
    let digit = |b: u8| match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        _ => panic!("not a lower-case hex digit"),
    };
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        *slot = digit(pair[0]) << 4 | digit(pair[1]);
    }
    out
}
