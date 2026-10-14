//! Downloads by a redirect to a presigned `GET` over real HTTP (ADR 0058 A1, step 6b), for the
//! three file routes: the owner's, a signed-in reader's through a link, and anybody's through a
//! public link. With `artifacts.s3.downloads: redirect` and a store that can sign, a route does
//! every check it always did, in the same order, and only then answers `302` to the presigned URL;
//! the checks that refuse are the answers they always were, and never a redirect.
//!
//! The bucket is the in-memory double (`MemoryArtifacts::presigning`), whose URL carries the signed
//! overrides in its query and which plays the browser's `GET` (`download`); a spy over it counts
//! what the route asked of the store, so "it did not read the file" is a count and not a hope.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use orch_api::{ApiConfig, PublicLimits};
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, DownloadMode, NewThread, Permission, Policy,
    PublicView, RoleGrant, ShareKeys, SharingMode, SharingSettings,
};
use orch_core::{Actor, AgentId, AgentTarget, ArtifactData, EventBody, FileRef, ThreadId, UserId};
use orch_ports::memory::{
    MemoryArtifacts, MemoryAuth, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{
    AgentEndpoint, ArtifactError, ArtifactHead, ArtifactKey, ArtifactMeta, ArtifactStore,
    ByteStream, Commit, FixedRegistry, NewEvent, NoModel, PortSet, PresignedGet, PresignedPut,
    Principal, ResponseOverrides, Role, SystemClock, ThreadStore,
};
use secrecy::SecretString;
use serde_json::{Value, json};
use tokio::task::JoinHandle;

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";
const SECRET: &str = "0123456789abcdef0123456789abcdef";

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01";
const ZIP: &[u8] = b"PK\x03\x04 a report that is large enough to be worth a redirect";
const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)" width="9" height="9"><script>alert(2)</script><circle cx="4" cy="4" r="3" fill="teal"/></svg>"#;

/// How the spy fails, when it does: the signing (`presign_get`) or the `HEAD` that says what a file is.
#[derive(Clone, Copy)]
enum Fail {
    Never,
    Unavailable,
    Unsupported,
    /// `presign_get` says the store is not configured.
    SignNotConfigured,
    /// The `HEAD` fails or says less than a redirect needs.
    Head(HeadFail),
}

/// What is wrong with the `HEAD`.
#[derive(Clone, Copy, Debug)]
enum HeadFail {
    Unavailable,
    Unauthenticated,
    Unsupported,
    NotConfigured,
    /// The object is there and says no content type.
    NoContentType,
}

/// What the routes asked of the store.
#[derive(Default)]
struct Counts {
    get: AtomicUsize,
    head: AtomicUsize,
    presign_get: AtomicUsize,
}

/// The in-memory bucket, counting the calls a download route makes and failing the signing when told.
#[derive(Clone)]
struct Spy {
    inner: MemoryArtifacts,
    counts: Arc<Counts>,
    fail: Arc<Mutex<Fail>>,
}

impl Spy {
    fn new() -> Self {
        Spy {
            inner: MemoryArtifacts::presigning(),
            counts: Arc::default(),
            fail: Arc::new(Mutex::new(Fail::Never)),
        }
    }

    fn failing(&self, fail: Fail) {
        *self.fail.lock().unwrap() = fail;
    }
}

impl ArtifactStore for Spy {
    async fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
        self.inner.put(key, bytes, meta).await
    }

    async fn get(
        &self,
        key: &ArtifactKey,
    ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
        self.counts.get.fetch_add(1, Ordering::SeqCst);
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

    fn can_presign(&self) -> bool {
        self.inner.can_presign()
    }

    async fn presign_put(
        &self,
        key: &ArtifactKey,
        meta: &ArtifactMeta,
        ttl: Duration,
    ) -> Result<PresignedPut, ArtifactError> {
        self.inner.presign_put(key, meta, ttl).await
    }

    async fn presign_get(
        &self,
        key: &ArtifactKey,
        overrides: &ResponseOverrides,
        ttl: Duration,
    ) -> Result<PresignedGet, ArtifactError> {
        self.counts.presign_get.fetch_add(1, Ordering::SeqCst);
        let fail = *self.fail.lock().unwrap();
        match fail {
            Fail::Never | Fail::Head(_) => self.inner.presign_get(key, overrides, ttl).await,
            Fail::Unavailable => Err(ArtifactError::unavailable("the signer is down")),
            Fail::Unsupported => Err(ArtifactError::Unsupported),
            Fail::SignNotConfigured => Err(ArtifactError::NotConfigured),
        }
    }

    async fn head(&self, key: &ArtifactKey) -> Result<Option<ArtifactHead>, ArtifactError> {
        self.counts.head.fetch_add(1, Ordering::SeqCst);
        let fail = *self.fail.lock().unwrap();
        let Fail::Head(how) = fail else {
            return self.inner.head(key).await;
        };
        match how {
            HeadFail::Unavailable => Err(ArtifactError::unavailable("the bucket is down")),
            HeadFail::Unauthenticated => Err(ArtifactError::Unauthenticated),
            HeadFail::Unsupported => Err(ArtifactError::Unsupported),
            HeadFail::NotConfigured => Err(ArtifactError::NotConfigured),
            HeadFail::NoContentType => {
                let mut head = self.inner.head(key).await?;
                if let Some(head) = &mut head {
                    head.media_type = None;
                }
                Ok(head)
            }
        }
    }
}

type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    MemoryAuth,
    Spy,
>;

struct Harness {
    base: String,
    /// A client that never follows a redirect: a stray `302` is an assertion, not a hidden second request.
    client: reqwest::Client,
    app: Arc<App<Stack>>,
    store: MemoryStore,
    bucket: Spy,
    contract: Value,
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

    fn has(&self, name: &str) -> bool {
        self.headers.contains_key(name)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The `Location` as the store's presigned `GET` and what the double's bucket answers it with.
    fn followed(&self, bucket: &MemoryArtifacts) -> orch_ports::memory::Download {
        let url = self.header("location").to_owned();
        let get = PresignedGet {
            url,
            expires_at: jiff::Timestamp::now(),
        };
        bucket.download(&get, jiff::Timestamp::now()).unwrap()
    }

    /// The seconds the URL of the `Location` lives from now.
    fn lives(&self) -> i64 {
        let url = self.header("location");
        let expires: i64 = url
            .split(['?', '&'])
            .find_map(|p| p.strip_prefix("expires="))
            .unwrap()
            .parse()
            .unwrap();
        expires - jiff::Timestamp::now().as_second()
    }
}

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

/// A `user`, and a `reader` who holds `thread.read` and nothing that opens a file.
fn policy() -> Policy {
    Policy::new(
        BTreeMap::from([
            (Role::new("user"), RoleGrant::user()),
            (
                Role::new("reader"),
                RoleGrant {
                    permissions: BTreeSet::from([Permission::ThreadRead]),
                    ..RoleGrant::none()
                },
            ),
        ]),
        Some(Role::new("user")),
    )
    .unwrap()
}

impl Harness {
    async fn start(downloads: DownloadMode) -> Self {
        Self::over(Spy::new(), downloads, true).await
    }

    async fn over(bucket: Spy, downloads: DownloadMode, public_files: bool) -> Self {
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
        auth.allow("reader", principal("reader@example.com", &["reader"]));
        let store = MemoryStore::new();
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: bucket.clone(),
                    store: store.clone(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: NoModel,
                    auth,
                    registry: directory.fixed_registry(),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    sharing: SharingSettings::new(
                        SharingMode::Public,
                        Some(ShareKeys::new(SecretString::from(SECRET.to_owned()), None).unwrap()),
                        PublicView {
                            step_io: false,
                            files: public_files,
                        },
                    )
                    .unwrap(),
                    downloads,
                    policy: policy(),
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // generous: a case here sends a link many requests, and the limiter is not what it tests
        let cfg = ApiConfig {
            public_limits: Some(PublicLimits {
                per_link_per_second: 1_000,
                total_per_second: 10_000,
                ..PublicLimits::default()
            }),
            ..ApiConfig::default()
        };
        let router = orch_api::router_with_surfaces(Arc::clone(&app), cfg, Vec::new());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let contract = serde_norway::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../docs/api/chat-api.yaml"
            ))
            .unwrap(),
        )
        .unwrap();
        Harness {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            app,
            store,
            bucket,
            contract,
            server,
        }
    }

    async fn get_with(&self, path: &str, bearer: Option<&str>, dest: Option<&str>) -> Resp {
        let mut req = self.client.get(format!("{}{path}", self.base));
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        }
        if let Some(dest) = dest {
            req = req.header("Sec-Fetch-Dest", dest);
        }
        let resp = req.send().await.unwrap();
        Resp {
            status: resp.status().as_u16(),
            headers: resp.headers().clone(),
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    async fn get(&self, path: &str, bearer: Option<&str>) -> Resp {
        self.get_with(path, bearer, None).await
    }

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

    /// Keeps a file in the thread's folder and names it in the thread's log, as the ingest does;
    /// returns its hash in hex.
    async fn keep(
        &self,
        thread: ThreadId,
        media_type: &str,
        filename: Option<&str>,
        bytes: &[u8],
    ) -> String {
        let meta = ArtifactMeta::of(media_type, filename.map(str::to_owned), bytes);
        let key = meta.key(thread);
        self.bucket
            .put(&key, Bytes::copy_from_slice(bytes), &meta)
            .await
            .unwrap();
        let sha = key.sha256_hex();
        let record = self.store.get_thread(None, thread).await.unwrap().unwrap();
        self.store
            .commit(
                thread,
                record.version,
                Commit {
                    new_state: record.state,
                    job: None,
                    events: vec![NewEvent {
                        at: jiff::Timestamp::now(),
                        actor: Actor::agent(&AgentId::new("plain"), None),
                        body: EventBody::Artifact(ArtifactData {
                            name: "file".into(),
                            mime_type: Some(media_type.to_owned()),
                            uri: None,
                            text: None,
                            file: Some(FileRef {
                                sha256: sha.clone(),
                                size: meta.size,
                                filename: meta.filename.clone(),
                            }),
                        }),
                        idempotency_key: None,
                    }],
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
        sha
    }

    /// Shares alice's thread as `visibility` and returns the link's token.
    async fn share(&self, thread: ThreadId, visibility: &str) -> String {
        let r = self
            .client
            .put(format!("{}/api/threads/{thread}/share", self.base))
            .bearer_auth("alice")
            .json(&json!({ "visibility": visibility }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        let link: Value = r.json().await.unwrap();
        link["url"]
            .as_str()
            .unwrap()
            .strip_prefix("/s/")
            .unwrap()
            .to_owned()
    }

    fn documents(&self, path: &str, status: u16) -> bool {
        self.contract["paths"][path]["get"]["responses"]
            .get(status.to_string())
            .is_some()
    }

    fn counts(&self) -> (usize, usize, usize) {
        let c = &self.bucket.counts;
        (
            c.get.load(Ordering::SeqCst),
            c.head.load(Ordering::SeqCst),
            c.presign_get.load(Ordering::SeqCst),
        )
    }
}

fn redirect(secs: u64) -> DownloadMode {
    DownloadMode::redirect(Duration::from_secs(secs)).unwrap()
}

fn own(thread: ThreadId, sha: &str) -> String {
    format!("/api/threads/{thread}/artifacts/{sha}")
}

// ---- the owner's route ---------------------------------------------------------------------------

#[tokio::test]
async fn the_owner_is_sent_to_a_presigned_get_that_carries_the_headers_the_route_decided() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let sha = h
        .keep(thread, "application/zip", Some("r&d report.zip"), ZIP)
        .await;

    let r = h.get(&own(thread, &sha), Some("alice")).await;

    assert_eq!(r.status, 302, "{}", r.text());
    assert!(r.body.is_empty());
    assert_eq!(r.header("cache-control"), "private, no-store");
    // the page that asked may be a share link's (`/s/<token>`): the files host is not told
    assert_eq!(r.header("referrer-policy"), "no-referrer");
    let location = r.header("location");
    assert!(
        location.starts_with(&format!("memory://bucket/threads/{thread}/{sha}?")),
        "{location}"
    );
    // the URL lives the lifetime of the deployment
    assert!((298..=300).contains(&r.lives()), "{}", r.lives());
    // and the bucket, asked the URL alone, answers with the headers the route signed: an attachment
    // under the cleaned name, a type that is no script or style, a cache that is the URL's life
    let served = r.followed(&h.bucket.inner);
    assert_eq!(served.bytes, ZIP);
    assert_eq!(
        served.content_disposition.as_deref(),
        Some("attachment; filename=\"r&d report.zip\"")
    );
    assert_eq!(served.content_type, "application/octet-stream");
    assert_eq!(
        served.cache_control.as_deref(),
        Some("private, max-age=300")
    );
    // the route asked what the file is, and signed; it did not read the file
    assert_eq!(h.counts(), (0, 1, 1));
    // none of the headers of a streamed file is on a redirect that has no body to carry them
    assert!(!r.has("content-disposition") && !r.has("etag"));
    // the contract says it
    assert!(h.documents("/api/threads/{threadId}/artifacts/{sha256}", 302));
}

#[tokio::test]
async fn a_name_that_is_not_ascii_has_both_forms_and_one_that_is_hostile_is_cleaned_as_ever() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let name_of = |r: &Resp| {
        r.followed(&h.bucket.inner)
            .content_disposition
            .unwrap_or_default()
    };
    let sha = h
        .keep(
            thread,
            "application/pdf",
            Some("Übersicht €.pdf"),
            b"%PDF one",
        )
        .await;
    let r = h.get(&own(thread, &sha), Some("alice")).await;
    assert_eq!(r.status, 302);
    assert_eq!(
        name_of(&r),
        "attachment; filename=\"_bersicht _.pdf\"; filename*=UTF-8''%C3%9Cbersicht%20%E2%82%AC.pdf"
    );
    // a path, a quote and a semicolon in a name cannot end the header or change the name
    let sha = h
        .keep(
            thread,
            "application/pdf",
            Some("../../a\";b.pdf"),
            b"%PDF two",
        )
        .await;
    let r = h.get(&own(thread, &sha), Some("alice")).await;
    assert_eq!(
        name_of(&r),
        "attachment; filename=\"a__b.pdf\"; filename*=UTF-8''a%22%3Bb.pdf"
    );
    // a file with no name is named by its hash
    let named = h
        .keep(thread, "application/zip", None, b"another file")
        .await;
    let r = h.get(&own(thread, &named), Some("alice")).await;
    assert_eq!(
        name_of(&r),
        format!("attachment; filename=\"artifact-{}\"", &named[..8])
    );
}

#[tokio::test]
async fn a_preview_type_is_inline_with_its_own_type_and_a_download_of_it_is_an_attachment() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    for (media_type, name, bytes, inline_type) in [
        ("image/png", "chart.png", PNG, "image/png"),
        (
            "text/plain",
            "notes.txt",
            b"hello".as_slice(),
            "text/plain; charset=utf-8",
        ),
        (
            "application/json",
            "data.json",
            b"{}".as_slice(),
            "application/json; charset=utf-8",
        ),
    ] {
        let sha = h.keep(thread, media_type, Some(name), bytes).await;
        let r = h.get(&own(thread, &sha), Some("alice")).await;
        assert_eq!(r.status, 302, "{media_type}");
        let served = r.followed(&h.bucket.inner);
        assert_eq!(
            served.content_disposition.as_deref(),
            Some(format!("inline; filename=\"{name}\"").as_str()),
            "{media_type}"
        );
        assert_eq!(served.content_type, inline_type);

        let r = h
            .get(&format!("{}?download=1", own(thread, &sha)), Some("alice"))
            .await;
        assert_eq!(r.status, 302, "{media_type} download");
        let served = r.followed(&h.bucket.inner);
        assert_eq!(
            served.content_disposition.as_deref(),
            Some(format!("attachment; filename=\"{name}\"").as_str()),
            "{media_type} download"
        );
        // a download is a preview type's own type, with no charset: it is saved, not shown
        assert_eq!(served.content_type, media_type);
    }
}

#[tokio::test]
async fn every_other_type_is_an_attachment_of_octet_stream_whatever_it_was_kept_with() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    for media_type in [
        "text/javascript",
        "text/css",
        "text/html",
        "application/pdf",
        "application/zip",
    ] {
        let sha = h
            .keep(thread, media_type, Some("f.bin"), media_type.as_bytes())
            .await;
        for query in ["", "?download=1"] {
            let r = h
                .get(&format!("{}{query}", own(thread, &sha)), Some("alice"))
                .await;
            assert_eq!(r.status, 302, "{media_type}{query}");
            let served = r.followed(&h.bucket.inner);
            assert_eq!(
                served.content_type, "application/octet-stream",
                "{media_type}"
            );
            assert_eq!(
                served.content_disposition.as_deref(),
                Some("attachment; filename=\"f.bin\""),
                "{media_type}"
            );
        }
    }
}

#[tokio::test]
async fn an_svg_shown_inline_is_never_redirected_and_its_download_is() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let sha = h
        .keep(thread, "image/svg+xml", Some("mark.svg"), SVG.as_bytes())
        .await;

    // inline: read, sanitized and streamed by the orchestrator, with the headers of a streamed file
    let r = h.get(&own(thread, &sha), Some("alice")).await;
    assert_eq!(r.status, 200);
    let body = r.text();
    assert!(body.contains("<circle") && !body.contains("script") && !body.contains("onload"));
    assert_eq!(r.header("content-type"), "image/svg+xml");
    assert_eq!(
        r.header("content-disposition"),
        "inline; filename=\"mark.svg\""
    );
    assert_eq!(
        r.header("cache-control"),
        "private, max-age=31536000, immutable"
    );
    assert!(!r.has("location"));
    assert_eq!(h.counts().2, 0, "nothing was signed for it");

    // a download of it: the original bytes, as an attachment, from the bucket
    let r = h
        .get(&format!("{}?download=1", own(thread, &sha)), Some("alice"))
        .await;
    assert_eq!(r.status, 302);
    let served = r.followed(&h.bucket.inner);
    assert_eq!(served.bytes, SVG.as_bytes());
    assert_eq!(served.content_type, "image/svg+xml");
    assert_eq!(
        served.content_disposition.as_deref(),
        Some("attachment; filename=\"mark.svg\"")
    );
}

#[tokio::test]
async fn streaming_is_what_a_deployment_gets_unless_it_asks_and_what_a_store_that_cannot_sign_gets()
{
    // `stream` is the default, even with a store that signs
    let h = Harness::start(DownloadMode::Stream).await;
    let thread = h.thread(ALICE).await;
    let sha = h.keep(thread, "image/png", Some("chart.png"), PNG).await;
    let r = h.get(&own(thread, &sha), Some("alice")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, PNG);
    assert_eq!(
        r.header("content-disposition"),
        "inline; filename=\"chart.png\""
    );
    assert_eq!(h.counts(), (1, 0, 0), "read, not asked about, not signed");

    // `redirect` over a store that cannot sign streams too
    let plain = Spy {
        inner: MemoryArtifacts::new(),
        counts: Arc::default(),
        fail: Arc::new(Mutex::new(Fail::Never)),
    };
    let h = Harness::over(plain, redirect(300), true).await;
    let thread = h.thread(ALICE).await;
    let sha = h.keep(thread, "image/png", Some("chart.png"), PNG).await;
    let r = h.get(&own(thread, &sha), Some("alice")).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body, PNG);
    assert_eq!(h.counts().2, 0);
}

// ---- a refusal is never a redirect ---------------------------------------------------------------

#[tokio::test]
async fn every_check_still_comes_first_and_every_miss_is_the_answer_it_always_was() {
    let h = Harness::start(redirect(300)).await;
    let alices = h.thread(ALICE).await;
    let other = h.thread(ALICE).await;
    let bobs = h.thread(BOB).await;
    let sha = h.keep(alices, "application/zip", Some("r.zip"), ZIP).await;

    // the control: the owner is redirected
    assert_eq!(h.get(&own(alices, &sha), Some("alice")).await.status, 302);

    let mut misses: Vec<(String, Resp)> = Vec::new();
    // `true`: the request got as far as the store, which the owner's own route has to ask (it
    // keeps no list of a thread's files, the folder is the thread's); the rest is refused before
    for (what, path, bearer, reaches_the_store) in [
        (
            "somebody else's thread",
            own(alices, &sha),
            Some("bob"),
            false,
        ),
        (
            "another thread of the owner",
            own(other, &sha),
            Some("alice"),
            true,
        ),
        (
            "bob's thread, alice's hash",
            own(bobs, &sha),
            Some("bob"),
            true,
        ),
        (
            "a hash the thread holds no file for",
            own(alices, &"0".repeat(64)),
            Some("alice"),
            true,
        ),
        (
            "a hash that is short",
            own(alices, &sha[..63]),
            Some("alice"),
            false,
        ),
        (
            "a hash in capitals",
            own(alices, &sha.to_uppercase()),
            Some("alice"),
            false,
        ),
        (
            "a thread that does not exist",
            own(ThreadId(uuid::Uuid::from_u128(9)), &sha),
            Some("alice"),
            false,
        ),
        ("no identity", own(alices, &sha), None, false),
        (
            "an identity nobody knows",
            own(alices, &sha),
            Some("nobody"),
            false,
        ),
        (
            "a role that opens no file",
            own(alices, &sha),
            Some("reader"),
            false,
        ),
    ] {
        let before = h.counts();
        let r = h.get(&path, bearer).await;
        assert!(!r.has("location"), "{what}: a refusal is no redirect");
        let after = h.counts();
        // never read, never signed
        assert_eq!((after.0, after.2), (before.0, before.2), "{what}");
        // and a request refused before the store is not even asked what is there
        assert_eq!(
            after.1 - before.1,
            usize::from(reaches_the_store),
            "{what}: the store was asked a thing it need not be"
        );
        misses.push((what.to_owned(), r));
    }
    let status = |what: &str| misses.iter().find(|(w, _)| w == what).unwrap().1.status;
    for what in [
        "somebody else's thread",
        "another thread of the owner",
        "bob's thread, alice's hash",
        "a hash the thread holds no file for",
        "a hash that is short",
        "a hash in capitals",
        "a thread that does not exist",
    ] {
        assert_eq!(status(what), 404, "{what}");
    }
    assert_eq!(status("no identity"), 401);
    assert_eq!(status("an identity nobody knows"), 401);
    assert_eq!(status("a role that opens no file"), 403);
    // somebody else's thread and one that does not exist look the same
    let body = |what: &str| {
        misses
            .iter()
            .find(|(w, _)| w == what)
            .unwrap()
            .1
            .body
            .clone()
    };
    assert_eq!(
        body("somebody else's thread"),
        body("a thread that does not exist")
    );

    // the browser asks to run or apply the file: refused before the file is looked at, opened or signed
    let asked = h.counts();
    for dest in REFUSED_DESTINATIONS {
        let r = h
            .get_with(&own(alices, &sha), Some("alice"), Some(dest))
            .await;
        assert_eq!(r.status, 403, "{dest}");
        assert!(!r.has("location"), "{dest}");
        assert_eq!(h.counts(), asked, "{dest}: the store was not asked");
    }
    // a navigation, an image and a fetch are fine
    for dest in ["document", "image", "empty", "iframe"] {
        let r = h
            .get_with(&own(alices, &sha), Some("alice"), Some(dest))
            .await;
        assert_eq!(r.status, 302, "{dest}");
    }
    // a value of `download` that is not one is a 400, before anything else
    let before = h.counts();
    let r = h
        .get(&format!("{}?download=2", own(alices, &sha)), Some("alice"))
        .await;
    assert_eq!(r.status, 400);
    assert!(!r.has("location"));
    assert_eq!(h.counts(), before);
}

/// The destinations a browser fetches to run or apply what it gets: a file is never sent for them.
const REFUSED_DESTINATIONS: [&str; 6] = [
    "script",
    "worker",
    "sharedworker",
    "serviceworker",
    "style",
    "object",
];

#[tokio::test]
async fn the_link_routes_refuse_what_a_browser_would_run_before_they_look_at_the_file() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let sha = h.keep(thread, "application/zip", Some("r.zip"), ZIP).await;
    let token = h.share(thread, "public").await;
    let routes = [
        (
            "a signed-in reader",
            format!("/api/shared/{token}/artifacts/{sha}"),
            Some("bob"),
        ),
        (
            "anybody",
            format!("/api/public/shared/{token}/artifacts/{sha}"),
            None,
        ),
    ];
    for (what, path, bearer) in &routes {
        let asked = h.counts();
        for dest in REFUSED_DESTINATIONS {
            let r = h.get_with(path, *bearer, Some(dest)).await;
            assert_eq!(r.status, 403, "{what}: {dest}");
            assert!(!r.has("location"), "{what}: {dest}");
            assert_eq!(h.counts(), asked, "{what}: {dest}: the store was not asked");
        }
        // a value of `download` that is not one is a 400, before anything else
        let r = h.get(&format!("{path}?download=2"), *bearer).await;
        assert_eq!(r.status, 400, "{what}");
        assert!(!r.has("location"), "{what}");
        assert_eq!(
            h.counts(),
            asked,
            "{what}: download=2: the store was not asked"
        );
        // a navigation, an image and a fetch are fine
        for dest in ["document", "image", "empty", "iframe"] {
            let r = h.get_with(path, *bearer, Some(dest)).await;
            assert_eq!(r.status, 302, "{what}: {dest}");
        }
    }
}

#[tokio::test]
async fn a_file_the_store_no_longer_holds_is_the_same_404_and_not_a_redirect_to_nothing() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let sha = h.keep(thread, "application/zip", Some("r.zip"), ZIP).await;
    let token = h.share(thread, "public").await;
    // the log names the file and the link serves the thread; the bucket no longer has the object
    let key = ArtifactKey::parse(&format!("threads/{thread}/{sha}")).unwrap();
    h.bucket.delete(&key).await.unwrap();
    for (what, path, bearer) in [
        ("the owner", own(thread, &sha), Some("alice")),
        (
            "a signed-in reader of the link",
            format!("/api/shared/{token}/artifacts/{sha}"),
            Some("bob"),
        ),
        (
            "anybody, by the public link",
            format!("/api/public/shared/{token}/artifacts/{sha}"),
            None,
        ),
    ] {
        let r = h.get(&path, bearer).await;
        assert_eq!(r.status, 404, "{what}");
        assert!(!r.has("location"), "{what}");
    }
    assert_eq!(
        h.counts().2,
        0,
        "a file that is not there is not signed for"
    );
}

#[tokio::test]
async fn a_store_that_cannot_sign_now_is_a_5xx_that_says_so_and_never_the_file_by_another_road() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let sha = h.keep(thread, "application/zip", Some("r.zip"), ZIP).await;

    h.bucket.failing(Fail::Unavailable);
    let r = h.get(&own(thread, &sha), Some("alice")).await;
    assert_eq!(r.status, 503);
    assert!(r.has("retry-after"));
    assert_eq!(r.header("content-type"), "application/problem+json");
    assert!(!r.text().contains("PK"), "the file is not sent instead");
    assert!(!r.has("location"));

    // a store that says it signs and does not is a bug of the deployment: a 500, not a 400, and again not the file
    h.bucket.failing(Fail::Unsupported);
    let r = h.get(&own(thread, &sha), Some("alice")).await;
    assert_eq!(r.status, 500);
    assert!(!r.text().contains("PK") && !r.has("location"));
    assert_eq!(h.counts().0, 0, "the file was never read to be sent");

    // and it recovers
    h.bucket.failing(Fail::Never);
    assert_eq!(h.get(&own(thread, &sha), Some("alice")).await.status, 302);
}

#[tokio::test]
async fn a_store_that_cannot_say_what_a_file_is_changes_no_refusal_and_is_the_error_it_is_for_a_reader()
 {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let sha = h.keep(thread, "application/zip", Some("r.zip"), ZIP).await;
    let public = h.share(thread, "public").await;
    // a thread shared as `internal` only (a thread has one link: sharing again changes it)
    let internal_thread = h.thread(ALICE).await;
    let internal_sha = h
        .keep(internal_thread, "application/zip", Some("i.zip"), ZIP)
        .await;
    let internal = h.share(internal_thread, "internal").await;
    let signed_in = format!("/api/shared/{public}/artifacts/{sha}");
    let anonymous = format!("/api/public/shared/{public}/artifacts/{sha}");

    // what a reader of the file gets when the `HEAD` fails: the store's own error, on every route
    for (how, status, retry_after) in [
        (HeadFail::Unavailable, 503, true),
        (HeadFail::Unauthenticated, 500, false),
        (HeadFail::Unsupported, 500, false),
        (HeadFail::NoContentType, 500, false),
        (HeadFail::NotConfigured, 404, false),
    ] {
        h.bucket.failing(Fail::Head(how));
        for (what, path, bearer) in [
            ("the owner", own(thread, &sha), Some("alice")),
            (
                "a signed-in reader of the link",
                signed_in.clone(),
                Some("bob"),
            ),
            ("anybody, by the public link", anonymous.clone(), None),
        ] {
            let r = h.get(&path, bearer).await;
            assert_eq!(r.status, status, "{how:?}: {what}: {}", r.text());
            assert_eq!(r.has("retry-after"), retry_after, "{how:?}: {what}");
            assert!(!r.has("location"), "{how:?}: {what}");
            assert!(!r.text().contains("PK"), "{how:?}: {what}: not the file");
            assert_eq!(h.counts().0, 0, "{how:?}: {what}: the file was never read");
            assert_eq!(h.counts().2, 0, "{how:?}: {what}: nothing was signed");
        }

        // and a request that is refused is refused as it always was, whatever the bucket is doing:
        // never a 5xx, and the store is not asked at all
        let asked = h.counts();
        for (what, path, bearer, dest, expected) in [
            (
                "somebody else's thread",
                own(thread, &sha),
                Some("bob"),
                None,
                404,
            ),
            ("no identity", own(thread, &sha), None, None, 401),
            (
                "an identity nobody knows",
                own(thread, &sha),
                Some("nobody"),
                None,
                401,
            ),
            (
                "a role that opens no file",
                own(thread, &sha),
                Some("reader"),
                None,
                403,
            ),
            (
                "a hash the link's thread does not name",
                format!("/api/shared/{public}/artifacts/{}", "0".repeat(64)),
                Some("bob"),
                None,
                404,
            ),
            (
                "a link that is not one",
                format!("/api/shared/not-a-link/artifacts/{sha}"),
                Some("bob"),
                None,
                404,
            ),
            (
                "a public path of an internal link",
                format!("/api/public/shared/{internal}/artifacts/{internal_sha}"),
                None,
                None,
                404,
            ),
            (
                "a browser asking to run the file",
                own(thread, &sha),
                Some("alice"),
                Some("script"),
                403,
            ),
            (
                "a browser asking to run the file by a link",
                anonymous.clone(),
                None,
                Some("worker"),
                403,
            ),
        ] {
            let r = h.get_with(&path, bearer, dest).await;
            assert_eq!(r.status, expected, "{how:?}: {what}");
            assert!(!r.has("location"), "{how:?}: {what}");
            assert_eq!(
                h.counts(),
                asked,
                "{how:?}: {what}: a refusal asks the store nothing"
            );
        }
    }
    // taking the link down: a request is a 404 whatever the bucket does, and asks it nothing
    h.bucket.failing(Fail::Head(HeadFail::Unavailable));
    let r = h
        .client
        .delete(format!("{}/api/threads/{thread}/share", h.base))
        .bearer_auth("alice")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let asked = h.counts();
    for (what, path, bearer) in [
        (
            "a signed-in reader of a revoked link",
            signed_in,
            Some("bob"),
        ),
        ("anybody, by a revoked public link", anonymous, None),
    ] {
        let r = h.get(&path, bearer).await;
        assert_eq!(r.status, 404, "{what}");
        assert!(!r.has("location"), "{what}");
        assert_eq!(h.counts(), asked, "{what}");
    }

    // it recovers
    h.bucket.failing(Fail::Never);
    assert_eq!(h.get(&own(thread, &sha), Some("alice")).await.status, 302);
}

#[tokio::test]
async fn a_signer_that_says_it_is_not_configured_is_the_same_404_as_a_head_that_says_so() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let sha = h.keep(thread, "application/zip", Some("r.zip"), ZIP).await;
    h.bucket.failing(Fail::SignNotConfigured);
    let r = h.get(&own(thread, &sha), Some("alice")).await;
    assert_eq!(r.status, 404, "{}", r.text());
    assert!(!r.has("location"));
    assert_eq!(h.counts().0, 0, "and the file was not sent instead");
}

// ---- through a link ------------------------------------------------------------------------------

#[tokio::test]
async fn a_reader_of_a_link_is_sent_for_at_most_a_minute_and_the_response_is_never_kept() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let sha = h.keep(thread, "application/zip", Some("r.zip"), ZIP).await;
    let token = h.share(thread, "public").await;
    let signed_in = format!("/api/shared/{token}/artifacts/{sha}");
    let public = format!("/api/public/shared/{token}/artifacts/{sha}");

    for (what, path, bearer) in [
        ("a signed-in reader", &signed_in, Some("bob")),
        ("anybody", &public, None),
    ] {
        let r = h.get(path, bearer).await;
        assert_eq!(r.status, 302, "{what}: {}", r.text());
        // the link's own rule for everything it serves, which a redirect is not exempt from
        assert_eq!(r.header("cache-control"), "no-store", "{what}");
        assert_eq!(r.header("x-robots-tag"), "noindex, nofollow", "{what}");
        assert_eq!(r.header("referrer-policy"), "no-referrer", "{what}");
        assert!((58..=60).contains(&r.lives()), "{what}: {}", r.lives());
        let served = r.followed(&h.bucket.inner);
        assert_eq!(served.bytes, ZIP);
        assert_eq!(
            served.content_disposition.as_deref(),
            Some("attachment; filename=\"r.zip\"")
        );
        assert_eq!(served.content_type, "application/octet-stream");
        // a URL that lives a minute says so to the browser
        assert_eq!(
            served.cache_control.as_deref(),
            Some("private, max-age=60"),
            "{what}"
        );
    }
    assert!(h.documents("/api/shared/{token}/artifacts/{sha256}", 302));
    assert!(h.documents("/api/public/shared/{token}/artifacts/{sha256}", 302));

    // the owner still has the deployment's lifetime on her own route
    let r = h.get(&own(thread, &sha), Some("alice")).await;
    assert!((298..=300).contains(&r.lives()));

    // a deployment whose lifetime is already short keeps it
    let short = Harness::start(redirect(30)).await;
    let thread = short.thread(ALICE).await;
    let sha = short
        .keep(thread, "application/zip", Some("r.zip"), ZIP)
        .await;
    let token = short.share(thread, "public").await;
    let r = short
        .get(&format!("/api/public/shared/{token}/artifacts/{sha}"), None)
        .await;
    assert!((28..=30).contains(&r.lives()), "{}", r.lives());
}

#[tokio::test]
async fn a_link_that_does_not_work_is_the_one_404_and_never_a_redirect() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let sha = h.keep(thread, "application/zip", Some("r.zip"), ZIP).await;
    let unnamed = h
        .keep(
            h.thread(ALICE).await,
            "application/zip",
            None,
            b"someone else's",
        )
        .await;
    let token = h.share(thread, "internal").await;

    // an internal link is no public one, and the store is not asked about it
    let public = format!("/api/public/shared/{token}/artifacts/{sha}");
    let r = h.get(&public, None).await;
    assert_eq!(r.status, 404);
    assert!(!r.has("location"));
    assert_eq!(
        h.counts(),
        (0, 0, 0),
        "an anonymous reader of an internal link"
    );
    // a signed-in reader of it is sent
    assert_eq!(
        h.get(&format!("/api/shared/{token}/artifacts/{sha}"), Some("bob"))
            .await
            .status,
        302
    );
    // what the control asked of the store; a refusal asks it nothing more
    let asked = h.counts();
    assert_eq!(asked, (0, 1, 1));
    // a file the thread's log does not name, a bad link, no identity, a role that opens no file
    for (what, path, bearer, status) in [
        (
            "a file of another thread",
            format!("/api/shared/{token}/artifacts/{unnamed}"),
            Some("bob"),
            404,
        ),
        (
            "a made-up hash",
            format!("/api/shared/{token}/artifacts/{}", "ab".repeat(32)),
            Some("bob"),
            404,
        ),
        (
            "a bad link",
            format!("/api/shared/not-a-link/artifacts/{sha}"),
            Some("bob"),
            404,
        ),
        (
            "no identity",
            format!("/api/shared/{token}/artifacts/{sha}"),
            None,
            401,
        ),
        (
            "a role that opens no file",
            format!("/api/shared/{token}/artifacts/{sha}"),
            Some("reader"),
            403,
        ),
    ] {
        let r = h.get(&path, bearer).await;
        assert_eq!(r.status, status, "{what}");
        assert!(!r.has("location"), "{what}");
        assert_eq!(h.counts(), asked, "{what}: the store was not asked");
    }
    // taking the link down: the next request is a 404, not a redirect
    let r = h
        .client
        .delete(format!("{}/api/threads/{thread}/share", h.base))
        .bearer_auth("alice")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let r = h
        .get(&format!("/api/shared/{token}/artifacts/{sha}"), Some("bob"))
        .await;
    assert_eq!(r.status, 404);
    assert!(!r.has("location"));
    assert_eq!(h.counts(), asked, "a revoked link: the store was not asked");
    // and a public reader has no files unless the deployment says so
    let closed = Harness::over(Spy::new(), redirect(300), false).await;
    let thread = closed.thread(ALICE).await;
    let sha = closed
        .keep(thread, "application/zip", Some("r.zip"), ZIP)
        .await;
    let token = closed.share(thread, "public").await;
    let r = closed
        .get(&format!("/api/public/shared/{token}/artifacts/{sha}"), None)
        .await;
    assert_eq!(r.status, 404);
    assert!(!r.has("location"));
    assert_eq!(
        closed.counts(),
        (0, 0, 0),
        "a public link that shows no files: the store was not asked"
    );
}

#[tokio::test]
async fn an_svg_through_a_link_is_streamed_inline_and_redirected_to_download() {
    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let sha = h
        .keep(thread, "image/svg+xml", Some("mark.svg"), SVG.as_bytes())
        .await;
    let token = h.share(thread, "public").await;
    for (path, bearer) in [
        (format!("/api/shared/{token}/artifacts/{sha}"), Some("bob")),
        (format!("/api/public/shared/{token}/artifacts/{sha}"), None),
    ] {
        let r = h.get(&path, bearer).await;
        assert_eq!(r.status, 200, "{path}");
        assert!(r.text().contains("<circle") && !r.text().contains("script"));
        assert_eq!(r.header("cache-control"), "no-store");
        let r = h.get(&format!("{path}?download=1"), bearer).await;
        assert_eq!(r.status, 302, "{path}");
        assert_eq!(r.followed(&h.bucket.inner).bytes, SVG.as_bytes());
    }
}

#[tokio::test]
async fn an_svg_too_large_to_be_cleaned_is_redirected_as_the_attachment_it_would_be_streamed_as() {
    // the same file, in a deployment that streams and in one that redirects
    let big = vec![b' '; usize::try_from(orch_api::MAX_SVG_INLINE_BYTES).unwrap() + 1];
    let streaming = Harness::start(DownloadMode::Stream).await;
    let thread = streaming.thread(ALICE).await;
    let sha = streaming
        .keep(thread, "image/svg+xml", Some("big.svg"), &big)
        .await;
    let streamed = streaming.get(&own(thread, &sha), Some("alice")).await;
    assert_eq!(streamed.status, 200);
    assert_eq!(
        streamed.header("content-disposition"),
        "attachment; filename=\"big.svg\""
    );
    assert_eq!(streamed.header("content-type"), "image/svg+xml");

    let h = Harness::start(redirect(300)).await;
    let thread = h.thread(ALICE).await;
    let sha = h.keep(thread, "image/svg+xml", Some("big.svg"), &big).await;
    let token = h.share(thread, "public").await;
    for (what, path, bearer) in [
        ("the owner", own(thread, &sha), Some("alice")),
        (
            "a signed-in reader of a link",
            format!("/api/shared/{token}/artifacts/{sha}"),
            Some("bob"),
        ),
        (
            "anybody, by a public link",
            format!("/api/public/shared/{token}/artifacts/{sha}"),
            None,
        ),
    ] {
        let before = h.counts();
        let r = h.get(&path, bearer).await;
        // (the body of a streamed file is two megabytes: not in the message)
        assert_eq!(r.status, 302, "{what}");
        let served = r.followed(&h.bucket.inner);
        // what the streamed response says: an attachment, in the type it is kept with
        assert_eq!(
            served.content_disposition.as_deref(),
            Some(streamed.header("content-disposition")),
            "{what}"
        );
        assert_eq!(
            served.content_type,
            streamed.header("content-type"),
            "{what}"
        );
        assert_eq!(served.bytes.len(), big.len(), "{what}");
        // and it was not read: one `HEAD`, one signing, no `GET`
        let after = h.counts();
        assert_eq!(
            (after.0 - before.0, after.1 - before.1, after.2 - before.2),
            (0, 1, 1),
            "{what}"
        );
    }
}
