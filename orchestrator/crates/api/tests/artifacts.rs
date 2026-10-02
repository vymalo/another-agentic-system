//! `GET /api/threads/{threadId}/artifacts/{sha256}` over real HTTP (ADR 0032): who may read a file,
//! how it is sent (headers, inline or attachment, a sanitized SVG), and that a large one is
//! streamed, not held whole.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, NewThread};
use orch_core::{AgentId, AgentTarget, ThreadId, UserId};
use orch_ports::memory::{MemoryArtifacts, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{
    AgentEndpoint, ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream,
    FixedRegistry, NoArtifacts, NoModel, PortSet, SystemClock,
};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01";
const SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" onload="alert(1)" width="9" height="9"><script>alert(2)</script><circle cx="4" cy="4" r="3" fill="teal"/></svg>"#;

type Stack<X> = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    orch_auth_header::HeaderAuth,
    X,
>;

struct Harness<X: ArtifactStore> {
    base: String,
    client: reqwest::Client,
    app: Arc<App<Stack<X>>>,
    store: X,
    server: JoinHandle<()>,
}

impl<X: ArtifactStore> Drop for Harness<X> {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl<X: ArtifactStore + Clone> Harness<X> {
    async fn start(store: X) -> Self {
        let directory = AgentDirectory::new(vec![AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new("plain"),
                "https://plain.example.com/.well-known/agent-card.json",
                None,
            ),
            name: "Plain".to_owned(),
        }]);
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: store.clone(),
                    store: MemoryStore::new(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: NoModel,
                    auth: orch_auth_header::HeaderAuth::new(),
                    registry: directory.fixed_registry(),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    ..AppConfig::default()
                },
            )
            .expect("a valid gate"),
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
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            app,
            store,
            server,
        }
    }

    /// A thread of `user`, which nothing works on (there is no dispatcher).
    async fn thread(&self, user: &str) -> ThreadId {
        self.app
            .create_thread(
                &UserId::new(user),
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

    /// Keeps a file in the thread the way the ingest does, and returns its hash in hex.
    async fn keep(
        &self,
        thread: ThreadId,
        media_type: &str,
        filename: Option<&str>,
        bytes: &[u8],
    ) -> String {
        let meta = ArtifactMeta::of(media_type, filename.map(str::to_owned), bytes);
        let key = meta.key(thread);
        self.store
            .put(&key, Bytes::copy_from_slice(bytes), &meta)
            .await
            .unwrap();
        key.sha256_hex()
    }

    async fn get(&self, path: &str, user: Option<&str>) -> reqwest::Response {
        let mut req = self.client.get(format!("{}{path}", self.base));
        if let Some(user) = user {
            req = req.header("X-Auth-Request-Email", user);
        }
        req.send().await.unwrap()
    }
}

fn header<'a>(response: &'a reqwest::Response, name: &str) -> &'a str {
    response
        .headers()
        .get(name)
        .unwrap_or_else(|| panic!("no {name} header in {:?}", response.headers()))
        .to_str()
        .unwrap()
}

/// What every file response carries, inline or not.
fn assert_safe_headers(response: &reqwest::Response, sha: &str) {
    assert_eq!(header(response, "x-content-type-options"), "nosniff");
    assert_eq!(
        header(response, "content-security-policy"),
        "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox"
    );
    assert_eq!(
        header(response, "cache-control"),
        "private, max-age=31536000, immutable"
    );
    assert_eq!(header(response, "etag"), format!("\"{sha}\""));
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn the_owner_gets_an_image_inline_with_every_safety_header() {
    let h = Harness::start(MemoryArtifacts::new()).await;
    let thread = h.thread(ALICE).await;
    let sha = h.keep(thread, "image/png", Some("chart.png"), PNG).await;
    let r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status(), 200);
    assert_eq!(header(&r, "content-type"), "image/png");
    assert_eq!(
        header(&r, "content-disposition"),
        "inline; filename=\"chart.png\""
    );
    assert_eq!(header(&r, "content-length"), PNG.len().to_string());
    assert_safe_headers(&r, &sha);
    assert_eq!(r.bytes().await.unwrap(), PNG);
    assert_eq!(sha, hex(PNG));
}

#[tokio::test]
async fn download_is_an_attachment_with_the_original_bytes() {
    let h = Harness::start(MemoryArtifacts::new()).await;
    let thread = h.thread(ALICE).await;
    let sha = h
        .keep(thread, "image/svg+xml", Some("drawing.svg"), SVG.as_bytes())
        .await;
    for flag in ["download=1", "download=true"] {
        let r = h
            .get(
                &format!("/api/threads/{thread}/artifacts/{sha}?{flag}"),
                Some(ALICE),
            )
            .await;
        assert_eq!(r.status(), 200, "{flag}");
        assert_eq!(
            header(&r, "content-disposition"),
            "attachment; filename=\"drawing.svg\""
        );
        assert_eq!(header(&r, "content-type"), "image/svg+xml");
        assert_safe_headers(&r, &sha);
        assert_eq!(
            r.bytes().await.unwrap(),
            SVG.as_bytes(),
            "{flag}: the original, script and all"
        );
    }
    // download=0 is the inline form
    let r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}?download=0"),
            Some(ALICE),
        )
        .await;
    assert!(header(&r, "content-disposition").starts_with("inline"));
    // anything else is a mistake
    let r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}?download=yes"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status(), 400);
    assert!(header(&r, "content-type").starts_with("application/problem+json"));
}

#[tokio::test]
async fn an_svg_shown_inline_is_the_sanitized_one() {
    let h = Harness::start(MemoryArtifacts::new()).await;
    let thread = h.thread(ALICE).await;
    let sha = h
        .keep(thread, "image/svg+xml", Some("drawing.svg"), SVG.as_bytes())
        .await;
    let r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status(), 200);
    assert_eq!(header(&r, "content-type"), "image/svg+xml");
    assert!(header(&r, "content-disposition").starts_with("inline"));
    assert_safe_headers(&r, &sha);
    let length: usize = header(&r, "content-length").parse().unwrap();
    let body = String::from_utf8(r.bytes().await.unwrap().to_vec()).unwrap();
    assert_eq!(body.len(), length, "the length is the cleaned body's");
    assert!(body.contains("<circle"), "{body}");
    assert!(
        !body.contains("script") && !body.contains("onload"),
        "{body}"
    );
}

#[tokio::test]
async fn an_svg_that_cannot_be_sanitized_is_an_attachment_not_inline() {
    let h = Harness::start(MemoryArtifacts::new()).await;
    let thread = h.thread(ALICE).await;
    // not well-formed
    let broken = b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</svg";
    let sha = h
        .keep(thread, "image/svg+xml", Some("broken.svg"), broken)
        .await;
    let r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status(), 200);
    assert!(header(&r, "content-disposition").starts_with("attachment"));
    assert_safe_headers(&r, &sha);
    assert_eq!(r.bytes().await.unwrap(), &broken[..]);

    // larger than the inline limit
    let mut big = String::from("<svg xmlns='http://www.w3.org/2000/svg'>");
    while big.len() as u64 <= orch_api::MAX_SVG_INLINE_BYTES {
        big.push_str("<rect width='1' height='1'/>");
    }
    big.push_str("</svg>");
    let sha = h
        .keep(thread, "image/svg+xml", Some("big.svg"), big.as_bytes())
        .await;
    let r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}"),
            Some(ALICE),
        )
        .await;
    assert!(header(&r, "content-disposition").starts_with("attachment"));
    assert_eq!(r.bytes().await.unwrap().len(), big.len());
}

#[tokio::test]
async fn only_the_preview_types_are_inline_and_everything_else_is_an_attachment() {
    let h = Harness::start(MemoryArtifacts::new()).await;
    let thread = h.thread(ALICE).await;
    let inline = [
        ("image/jpeg", "image/jpeg"),
        ("image/gif", "image/gif"),
        ("image/webp", "image/webp"),
        ("text/plain", "text/plain; charset=utf-8"),
        ("application/json", "application/json; charset=utf-8"),
    ];
    for (n, (kept, served)) in inline.into_iter().enumerate() {
        let bytes = format!("content {n}");
        let sha = h.keep(thread, kept, Some("f"), bytes.as_bytes()).await;
        let r = h
            .get(
                &format!("/api/threads/{thread}/artifacts/{sha}"),
                Some(ALICE),
            )
            .await;
        assert_eq!(header(&r, "content-type"), served);
        assert!(
            header(&r, "content-disposition").starts_with("inline"),
            "{kept}"
        );
    }
    let attachment = [
        "application/pdf",
        "application/zip",
        "application/octet-stream",
        "text/html",
        "text/markdown",
        "image/bmp",
        "image/x-icon",
        "application/javascript",
        "text/xml",
    ];
    for (n, kept) in attachment.into_iter().enumerate() {
        let bytes = format!("<script>alert({n})</script>");
        let sha = h.keep(thread, kept, Some("f.bin"), bytes.as_bytes()).await;
        let r = h
            .get(
                &format!("/api/threads/{thread}/artifacts/{sha}"),
                Some(ALICE),
            )
            .await;
        assert_eq!(r.status(), 200);
        assert!(
            header(&r, "content-disposition").starts_with("attachment"),
            "{kept}"
        );
        assert_eq!(header(&r, "content-type"), kept);
        assert_safe_headers(&r, &sha);
    }
}

#[tokio::test]
async fn a_file_name_cannot_break_the_header_and_a_name_that_is_not_ascii_has_both_forms() {
    let h = Harness::start(MemoryArtifacts::new()).await;
    let thread = h.thread(ALICE).await;
    let sha = h
        .keep(thread, "application/pdf", Some("Übersicht €.pdf"), b"pdf")
        .await;
    let r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}"),
            Some(ALICE),
        )
        .await;
    assert_eq!(
        header(&r, "content-disposition"),
        "attachment; filename=\"_bersicht _.pdf\"; filename*=UTF-8''%C3%9Cbersicht%20%E2%82%AC.pdf"
    );
    let sha = h
        .keep(
            thread,
            "application/pdf",
            Some("a\"; filename=\"../../b\\c%00"),
            b"x",
        )
        .await;
    let r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}"),
            Some(ALICE),
        )
        .await;
    let value = header(&r, "content-disposition");
    assert_eq!(value.matches("filename=").count(), 1, "{value}");
    assert!(!value.contains("..") && !value.contains('\\'), "{value}");
    // no name: named by the hash
    let sha = h.keep(thread, "application/pdf", None, b"unnamed").await;
    let r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}"),
            Some(ALICE),
        )
        .await;
    assert_eq!(
        header(&r, "content-disposition"),
        format!("attachment; filename=\"artifact-{}\"", &sha[..8])
    );
}

#[tokio::test]
async fn nobody_reaches_a_file_that_is_not_theirs_and_every_miss_is_the_same_404() {
    let h = Harness::start(MemoryArtifacts::new()).await;
    let alices = h.thread(ALICE).await;
    let other = h.thread(ALICE).await;
    let bobs = h.thread(BOB).await;
    let sha = h.keep(alices, "image/png", Some("chart.png"), PNG).await;
    let url = |thread: &dyn std::fmt::Display, sha: &str| {
        format!("/api/threads/{thread}/artifacts/{sha}")
    };
    let problem =
        |r: &reqwest::Response| header(r, "content-type").starts_with("application/problem+json");

    // the same bytes in the same thread, for the owner: 200
    assert_eq!(h.get(&url(&alices, &sha), Some(ALICE)).await.status(), 200);
    // somebody else's thread, with the right hash
    let r = h.get(&url(&alices, &sha), Some(BOB)).await;
    assert_eq!(r.status(), 404);
    assert!(problem(&r));
    let wrong_owner_body = r.text().await.unwrap();
    // a hash that is not in the thread (the file of another thread is not reachable by hash)
    let r = h.get(&url(&other, &sha), Some(ALICE)).await;
    assert_eq!(r.status(), 404);
    // bob's own thread, alice's hash
    assert_eq!(h.get(&url(&bobs, &sha), Some(BOB)).await.status(), 404);
    // unknown hash, short, long, upper case, not hex, a path trick
    let unknown = "0".repeat(64);
    let upper = sha.to_uppercase();
    for bad in [
        unknown.as_str(),
        &sha[..63],
        &format!("{sha}0"),
        upper.as_str(),
        "not-a-hash",
        "..%2F..%2Fetc%2Fpasswd",
    ] {
        let r = h.get(&url(&alices, bad), Some(ALICE)).await;
        assert_eq!(r.status(), 404, "{bad}");
        assert!(problem(&r), "{bad}");
    }
    // a thread that does not exist and one that is not an id
    let r = h
        .get(
            &url(&"0190aaaa-0000-7000-8000-000000000123", &sha),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status(), 404);
    assert_eq!(
        h.get(&url(&"not-a-uuid", &sha), Some(ALICE)).await.status(),
        404
    );
    // somebody else's thread and a missing one look the same
    let missing = h
        .get(
            &url(&"0190aaaa-0000-7000-8000-000000000123", &sha),
            Some(ALICE),
        )
        .await
        .text()
        .await
        .unwrap();
    assert_eq!(wrong_owner_body, missing);
    // no identity: refused before anything is looked up
    assert_eq!(h.get(&url(&alices, &sha), None).await.status(), 401);
}

#[tokio::test]
async fn without_an_artifact_store_every_file_is_a_404() {
    let h = Harness::start(NoArtifacts).await;
    let thread = h.thread(ALICE).await;
    let r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{}", "ab".repeat(32)),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status(), 404);
}

#[tokio::test]
async fn a_large_file_is_streamed_from_the_directory_store_whole_and_in_pieces() {
    let dir = tempfile::tempdir().unwrap();
    let store = orch_artifacts_fs::FsArtifacts::open(dir.path().join("files"))
        .await
        .unwrap();
    let h = Harness::start(store).await;
    let thread = h.thread(ALICE).await;
    // 24 MiB of non-repeating bytes
    let mut x = 0x9E37_79B9_7F4A_7C15_u64;
    let big: Vec<u8> = (0..24 * 1024 * 1024)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x.to_le_bytes()[3]
        })
        .collect();
    let sha = h
        .keep(thread, "application/octet-stream", Some("big.bin"), &big)
        .await;
    let mut r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status(), 200);
    assert_eq!(header(&r, "content-length"), big.len().to_string());
    let mut hasher = Sha256::new();
    let (mut pieces, mut total) = (0, 0);
    while let Some(piece) = r.chunk().await.unwrap() {
        pieces += 1;
        total += piece.len();
        hasher.update(&piece);
    }
    assert_eq!(total, big.len());
    assert_eq!(
        format!("{:x}", hasher.finalize()),
        sha,
        "the bytes are the file's"
    );
    assert!(
        pieces > 24,
        "streamed in many pieces, not one body: {pieces}"
    );
}

/// A store whose file arrives in two parts, the second one when the test says.
#[derive(Clone)]
struct Staged {
    inner: MemoryArtifacts,
    release: Arc<Notify>,
}

impl ArtifactStore for Staged {
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
        let Some((meta, stream)) = self.inner.get(key).await? else {
            return Ok(None);
        };
        let release = Arc::clone(&self.release);
        let mut pieces = futures_pieces(stream).await;
        let rest = pieces.split_off(1);
        let staged = futures::stream::iter(pieces.into_iter().map(Ok)).chain(
            futures::stream::once(async move {
                release.notified().await;
                Ok(Bytes::from(rest.concat()))
            }),
        );
        Ok(Some((meta, Box::pin(staged))))
    }
    async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
        self.inner.delete(key).await
    }
}

use futures::StreamExt as _;

async fn futures_pieces(mut stream: ByteStream) -> Vec<Bytes> {
    let mut out = Vec::new();
    while let Some(piece) = stream.next().await {
        out.push(piece.unwrap());
    }
    out
}

#[tokio::test]
async fn the_first_bytes_arrive_before_the_store_has_the_rest() {
    let release = Arc::new(Notify::new());
    let h = Harness::start(Staged {
        inner: MemoryArtifacts::new(),
        release: Arc::clone(&release),
    })
    .await;
    let thread = h.thread(ALICE).await;
    let big = vec![7u8; 200_000];
    let sha = h
        .keep(thread, "application/octet-stream", Some("big.bin"), &big)
        .await;
    let mut r = h
        .get(
            &format!("/api/threads/{thread}/artifacts/{sha}"),
            Some(ALICE),
        )
        .await;
    assert_eq!(
        r.status(),
        200,
        "the headers are out while the store still holds the rest"
    );
    let first = tokio::time::timeout(Duration::from_secs(5), r.chunk())
        .await
        .expect("the first piece arrives without the rest")
        .unwrap()
        .unwrap();
    assert!(!first.is_empty() && first.len() < big.len());
    release.notify_one();
    let mut total = first.len();
    while let Some(piece) = r.chunk().await.unwrap() {
        total += piece.len();
    }
    assert_eq!(total, big.len());
}

/// A store that fails in the middle of a file.
#[derive(Clone)]
struct Failing {
    inner: MemoryArtifacts,
}

impl ArtifactStore for Failing {
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
        let Some((meta, _)) = self.inner.get(key).await? else {
            return Ok(None);
        };
        let pieces: Vec<Result<Bytes, ArtifactError>> = vec![
            Ok(Bytes::from_static(b"the first half")),
            Err(ArtifactError::unavailable(
                "bucket orchestrator-secret-bucket went away",
            )),
        ];
        Ok(Some((meta, Box::pin(futures::stream::iter(pieces)))))
    }
    async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
        self.inner.delete(key).await
    }
}

#[tokio::test]
async fn a_store_that_fails_midway_does_not_send_a_prefix_as_the_file() {
    let h = Harness::start(Failing {
        inner: MemoryArtifacts::new(),
    })
    .await;
    let thread = h.thread(ALICE).await;
    let sha = h
        .keep(
            thread,
            "application/octet-stream",
            Some("f.bin"),
            &vec![1u8; 4096],
        )
        .await;
    // the response ends in an error (whether the client notices before or after the head), so a
    // client cannot take what it got for the whole file
    let sent = h
        .client
        .get(format!("{}/api/threads/{thread}/artifacts/{sha}", h.base))
        .header("X-Auth-Request-Email", ALICE)
        .send()
        .await;
    let shown = match sent {
        Err(e) => format!("{e:?}"),
        Ok(r) => {
            assert_eq!(r.status(), 200);
            let outcome = r.bytes().await;
            assert!(outcome.is_err(), "{outcome:?}");
            format!("{outcome:?}")
        }
    };
    assert!(!shown.contains("secret-bucket"), "{shown}");
}
