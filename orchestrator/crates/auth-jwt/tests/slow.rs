//! An issuer that is slow, or never answers: nobody waits for it who need not, a fetch is not lost with the
//! request that started it, and every request to the issuer is bounded by the timeout. Over real HTTP, against
//! servers on local ports whose delays a test sets.
//!
//! The real sockets need real time, so the margins are wide: an issuer is made slow by seconds and a request that
//! must not wait for it is given a bound well under that, and the tests never assert a short sleep.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::routing::get;
use common::{SETTLE, eventually, exactly};
use orch_auth_jwt::testkit::TestIdp;
use orch_auth_jwt::{JwtAuth, JwtConfig};
use orch_ports::testkit::bearer::{Alg, Signing};
use orch_ports::{AuthError, Authenticator, Credentials, Principal};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

const AUDIENCE: &str = "orchestrator-web";
const ALICE: &str = "alice@example.com";

// ---- the servers ------------------------------------------------------------------------------------------------

struct Slow {
    upstream: String,
    http: reqwest::Client,
    delay_ms: AtomicU64,
    hang: AtomicBool,
    started: AtomicUsize,
    answered: AtomicUsize,
}

async fn slow_keys(State(slow): State<Arc<Slow>>) -> impl axum::response::IntoResponse {
    slow.started.fetch_add(1, Ordering::SeqCst);
    if slow.hang.load(Ordering::SeqCst) {
        std::future::pending::<()>().await;
    }
    tokio::time::sleep(Duration::from_millis(slow.delay_ms.load(Ordering::SeqCst))).await;
    let body = slow
        .http
        .get(&slow.upstream)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    slow.answered.fetch_add(1, Ordering::SeqCst);
    ([(CONTENT_TYPE, "application/json")], body)
}

/// The key endpoint of an issuer in front of a `TestIdp`'s: it serves what the test issuer publishes at that
/// moment, after a delay a test sets, or never.
struct SlowKeys {
    slow: Arc<Slow>,
    url: String,
    server: JoinHandle<()>,
}

impl Drop for SlowKeys {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl SlowKeys {
    async fn start(idp: &TestIdp) -> Self {
        let slow = Arc::new(Slow {
            upstream: idp.jwks_url(),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            delay_ms: AtomicU64::new(0),
            hang: AtomicBool::new(false),
            started: AtomicUsize::new(0),
            answered: AtomicUsize::new(0),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/jwks", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/jwks", get(slow_keys))
            .with_state(Arc::clone(&slow));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { slow, url, server }
    }

    fn set_delay(&self, delay: Duration) {
        self.slow
            .delay_ms
            .store(delay.as_millis() as u64, Ordering::SeqCst);
    }

    fn set_hang(&self, hang: bool) {
        self.slow.hang.store(hang, Ordering::SeqCst);
    }

    /// Requests that reached the server.
    fn started(&self) -> usize {
        self.slow.started.load(Ordering::SeqCst)
    }

    /// Requests the server has answered.
    fn answered(&self) -> usize {
        self.slow.answered.load(Ordering::SeqCst)
    }
}

/// A test issuer, the slow place its authenticator reads the keys from, and the authenticator.
struct Setup {
    idp: TestIdp,
    keys: SlowKeys,
    auth: JwtAuth,
}

impl Setup {
    async fn new(adjust: impl FnOnce(&mut JwtConfig)) -> Self {
        let idp = TestIdp::start().await;
        let keys = SlowKeys::start(&idp).await;
        let mut config = JwtConfig::new(idp.issuer(), [AUDIENCE])
            .with_jwks_url(keys.url.clone())
            .without_system_proxy();
        adjust(&mut config);
        let auth = JwtAuth::new(config).unwrap();
        Self { idp, keys, auth }
    }

    fn good(&self) -> String {
        self.idp.token(AUDIENCE, ALICE)
    }

    /// A token signed by the key the test issuer has not published, so that the copy lacks its `kid`.
    fn unknown(&self) -> String {
        self.idp.mint(
            &self.idp.claims(AUDIENCE, ALICE),
            Signing::Unpublished(Alg::Rs256),
        )
    }

    async fn check(&self, token: &str) -> Result<Principal, AuthError> {
        check(&self.auth, token).await
    }

    /// The request, running on its own.
    fn spawn(&self, token: String) -> JoinHandle<Result<Principal, AuthError>> {
        let auth = self.auth.clone();
        tokio::spawn(async move { check(&auth, &token).await })
    }
}

async fn check(auth: &JwtAuth, token: &str) -> Result<Principal, AuthError> {
    auth.authenticate(&Credentials {
        bearer: Some(token),
        ..Credentials::default()
    })
    .await
}

#[track_caller]
fn assert_accepted(result: Result<Principal, AuthError>, what: &str) {
    if let Err(error) = result {
        panic!("{what}: expected the token to be accepted, got {error:?}");
    }
}

#[track_caller]
fn assert_unavailable<T: std::fmt::Debug>(result: Result<T, AuthError>, what: &str) {
    assert!(
        matches!(result, Err(AuthError::Unavailable { .. })),
        "{what}: expected Unavailable, got {result:?}"
    );
}

/// The refusal a token that names a key the issuer has not published ends with, once the keys are fetched.
#[track_caller]
fn assert_unknown_key(result: Result<Principal, AuthError>, what: &str) {
    match result {
        Err(AuthError::Invalid { detail, .. }) => assert!(
            detail.contains("signing key is not the issuer's"),
            "{what}: {detail}"
        ),
        other => panic!("{what}: expected the refusal of an unknown key, got {other:?}"),
    }
}

/// A listener that accepts connections and does what `serve` says with each, keeping them open. Returns the
/// issuer's base URL.
async fn raw_server<F, Fut>(serve: F) -> String
where
    F: Fn(tokio::net::TcpStream) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(serve(stream));
        }
    });
    base
}

/// Reads the request line and headers of `stream`.
async fn read_request(stream: &mut tokio::net::TcpStream) {
    let mut seen = Vec::new();
    let mut buf = [0u8; 1024];
    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).await.unwrap();
        if n == 0 {
            return;
        }
        seen.extend_from_slice(&buf[..n]);
    }
}

// ---- nobody waits who need not -----------------------------------------------------------------------------------

#[tokio::test]
async fn a_good_token_is_answered_while_an_unknown_kid_fetch_to_a_slow_issuer_is_running() {
    let s = Setup::new(|c| c.kid_refetch_interval = Duration::from_millis(50)).await;
    let good = s.good();
    assert_accepted(s.check(&good).await, "the first fetch is quick");
    assert_eq!(s.keys.started(), 1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The issuer is slow from now on, and a token names a key it does not have: one fetch, which takes 3 s.
    s.keys.set_delay(Duration::from_secs(3));
    let unknown = s.spawn(s.unknown());
    eventually("the fetch to be running", || s.keys.started() == 2).await;

    // Everybody else goes on with the copy they have.
    let began = Instant::now();
    for _ in 0..5 {
        assert_accepted(s.check(&good).await, "a good token meanwhile");
    }
    let waited = began.elapsed();
    assert!(
        !unknown.is_finished() && waited < Duration::from_millis(1500),
        "five good tokens took {waited:?}: they waited for the fetch an unknown kid started"
    );

    // Only the request that named the unknown key waits for the fetch, and is then told what it is.
    assert_unknown_key(unknown.await.unwrap(), "the unknown kid, after the fetch");
    assert_eq!(s.keys.started(), 2);
}

#[tokio::test]
async fn unknown_kids_that_arrive_while_a_fetch_is_running_join_it() {
    let s = Setup::new(|c| c.kid_refetch_interval = Duration::from_millis(50)).await;
    assert_accepted(s.check(&s.good()).await, "the first fetch");
    tokio::time::sleep(Duration::from_millis(100)).await;

    s.keys.set_delay(Duration::from_millis(600));
    // The issuer rotates; sixteen requests name its new key at the same moment, then a few more while the
    // fetch is running.
    s.idp.publish_unpublished_keys();
    let mut requests = Vec::new();
    for _ in 0..16 {
        requests.push(s.spawn(s.unknown()));
    }
    eventually("the fetch to be running", || s.keys.started() == 2).await;
    for _ in 0..4 {
        requests.push(s.spawn(s.unknown()));
    }
    for request in requests {
        assert_accepted(
            request.await.unwrap(),
            "the rotated key, found by the fetch",
        );
    }
    assert_eq!(s.keys.started(), 2, "one fetch for all of them");
}

#[tokio::test]
async fn a_stale_copy_is_served_at_once_and_refreshed_in_the_background_once() {
    let s = Setup::new(|c| {
        c.refresh_interval = Duration::from_millis(200);
        c.stale_grace = Duration::from_secs(60);
        c.timeout = Duration::from_secs(3);
    })
    .await;
    let good = s.good();
    assert_accepted(s.check(&good).await, "the first fetch");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The copy is older than the refresh interval, and the issuer takes the request and never answers.
    s.keys.set_hang(true);
    let began = Instant::now();
    for _ in 0..8 {
        assert_accepted(s.check(&good).await, "the stale copy, within its grace");
    }
    let waited = began.elapsed();
    assert!(
        waited < Duration::from_millis(1500),
        "eight tokens took {waited:?} with the issuer hanging: the stale copy was not served at once"
    );
    exactly(
        "one refresh, however many requests",
        || s.keys.started(),
        2,
        SETTLE,
    )
    .await;
}

#[tokio::test]
async fn ready_returns_at_once_while_a_copy_can_verify_and_waits_when_there_is_none() {
    let s = Setup::new(|c| {
        c.refresh_interval = Duration::from_millis(200);
        c.stale_grace = Duration::from_secs(60);
        c.timeout = Duration::from_secs(3);
    })
    .await;
    s.auth.ready().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.keys.set_hang(true);

    // Stale, inside the grace: ready, and a refresh is started in the background.
    let began = Instant::now();
    s.auth.ready().await.unwrap();
    assert!(
        began.elapsed() < Duration::from_millis(1500),
        "ready waited {:?} for a refresh while a copy could verify",
        began.elapsed()
    );
    eventually("the refresh to be running", || s.keys.started() == 2).await;

    // No copy at all: ready has to wait for the fetch, which gives up at the timeout.
    let down = Setup::new(|c| c.timeout = Duration::from_millis(800)).await;
    down.keys.set_hang(true);
    let began = Instant::now();
    assert_unavailable(down.auth.ready().await, "ready with no copy");
    assert!(
        began.elapsed() >= Duration::from_millis(700),
        "ready did not wait for the only fetch: {:?}",
        began.elapsed()
    );
}

#[tokio::test]
async fn the_first_requests_wait_for_the_one_fetch_and_are_unavailable_when_it_times_out() {
    let s = Setup::new(|c| c.timeout = Duration::from_millis(600)).await;
    s.keys.set_delay(Duration::from_millis(300));
    let good = s.good();
    // Six first requests at once: one fetch, which they all wait for.
    let began = Instant::now();
    let requests: Vec<_> = (0..6).map(|_| s.spawn(good.clone())).collect();
    for request in requests {
        assert_accepted(request.await.unwrap(), "after the one fetch");
    }
    assert!(began.elapsed() >= Duration::from_millis(250));
    assert_eq!(s.keys.started(), 1);

    // And when the issuer never answers, they all fail closed, after the timeout, and not before.
    let down = Setup::new(|c| c.timeout = Duration::from_millis(600)).await;
    down.keys.set_hang(true);
    let good = down.good();
    let began = Instant::now();
    let requests: Vec<_> = (0..4).map(|_| down.spawn(good.clone())).collect();
    for request in requests {
        assert_unavailable(request.await.unwrap(), "no copy, and the fetch timed out");
    }
    assert!(began.elapsed() >= Duration::from_millis(550));
    assert_eq!(down.keys.started(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_on_many_threads_make_one_fetch_and_one_refresh() {
    let s = Setup::new(|c| {
        c.refresh_interval = Duration::from_millis(400);
        c.stale_grace = Duration::from_secs(60);
    })
    .await;
    let good = s.good();

    // Thirty-two first requests on four threads: one fetch, which they all wait for.
    s.keys.set_delay(Duration::from_millis(200));
    let requests: Vec<_> = (0..32).map(|_| s.spawn(good.clone())).collect();
    for request in requests {
        assert_accepted(request.await.unwrap(), "after the one fetch");
    }
    assert_eq!(s.keys.started(), 1);

    // The copy grows old, the issuer is slow, and thirty-two requests come at once: all are answered by the
    // stale copy, and one refresh starts.
    tokio::time::sleep(Duration::from_millis(500)).await;
    s.keys.set_delay(Duration::from_millis(1500));
    let began = Instant::now();
    let requests: Vec<_> = (0..32).map(|_| s.spawn(good.clone())).collect();
    for request in requests {
        assert_accepted(request.await.unwrap(), "the stale copy");
    }
    assert!(
        began.elapsed() < Duration::from_millis(1000),
        "thirty-two tokens took {:?} against a stale copy: they waited for the refresh",
        began.elapsed()
    );
    eventually("the refresh to be answered", || s.keys.answered() >= 2).await;
    exactly(
        "one refresh, however many requests",
        || s.keys.started(),
        2,
        SETTLE,
    )
    .await;
}

// ---- a request that gives up does not take the fetch with it ------------------------------------------------------

#[tokio::test]
async fn a_request_cancelled_during_a_slow_fetch_does_not_lose_the_keys_it_asked_for() {
    // An unknown kid earns one fetch per interval. If the fetch died with the request that started it, the
    // interval would be spent and the rotated key refused until it was over.
    let interval = Duration::from_millis(1500);
    let s = Setup::new(|c| c.kid_refetch_interval = interval).await;
    assert_accepted(s.check(&s.good()).await, "the first fetch");
    tokio::time::sleep(interval + Duration::from_millis(100)).await;

    // The issuer rotates, and answers slowly. The first request to name the new key gives up after 100 ms.
    s.idp.publish_unpublished_keys();
    s.keys.set_delay(Duration::from_millis(400));
    let rotated = s.unknown();
    let cancelled = tokio::time::timeout(Duration::from_millis(100), s.check(&rotated)).await;
    assert!(cancelled.is_err(), "the request was cancelled");
    // The fetch is a task of its own: it reaches the issuer whether or not the request is still there to see it.
    eventually("the fetch to reach the issuer", || s.keys.started() == 2).await;

    // Once the fetch is over, the keys it brought are the copy: the next request with that key is accepted,
    // well inside the interval of the fetch that brought them.
    eventually("the fetch to be answered", || s.keys.answered() == 2).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_accepted(
        s.check(&rotated).await,
        "the rotated key, from the fetch of a request that is gone",
    );
    assert_eq!(s.keys.started(), 2, "and nothing was fetched again");
}

#[tokio::test]
async fn a_first_request_cancelled_during_the_fetch_does_not_lose_the_keys() {
    let s = Setup::new(|_| {}).await;
    s.keys.set_delay(Duration::from_millis(400));
    let good = s.good();
    let cancelled = tokio::time::timeout(Duration::from_millis(100), s.check(&good)).await;
    assert!(cancelled.is_err(), "the first request was cancelled");
    eventually("the fetch to reach the issuer", || s.keys.started() == 1).await;

    // The fetch goes on, and its keys are the copy: the next request is answered from it, with no fetch of its own.
    eventually("the fetch to be answered", || s.keys.answered() == 1).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_accepted(s.check(&good).await, "the copy of the cancelled fetch");
    assert_eq!(s.keys.started(), 1, "and nothing was fetched again");
}

#[tokio::test]
async fn an_unknown_kid_joins_the_refresh_of_a_stale_copy_that_is_already_running() {
    let s = Setup::new(|c| {
        c.refresh_interval = Duration::from_millis(200);
        c.stale_grace = Duration::from_secs(60);
    })
    .await;
    let good = s.good();
    assert_accepted(s.check(&good).await, "the first fetch");
    tokio::time::sleep(Duration::from_millis(300)).await;

    s.idp.publish_unpublished_keys();
    s.keys.set_delay(Duration::from_millis(500));
    // A good token: answered by the stale copy, and the refresh starts.
    assert_accepted(s.check(&good).await, "the stale copy");
    eventually("the refresh to be running", || s.keys.started() == 2).await;
    // A token of the rotated key: it waits for that refresh rather than asking again.
    assert_accepted(
        s.check(&s.unknown()).await,
        "the rotated key, from the refresh already running",
    );
    assert_eq!(s.keys.started(), 2);
}

#[tokio::test]
async fn a_fetch_ends_at_the_timeout_when_the_authenticator_that_started_it_is_gone() {
    // The fetch is a task of its own that holds what it needs: it is not tied to any request, and it is not
    // immortal either. The issuer reads the request and waits for the connection to close.
    let connected = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicUsize::new(0));
    let (opened, ended) = (Arc::clone(&connected), Arc::clone(&closed));
    let base = raw_server(move |mut stream| {
        let (opened, ended) = (Arc::clone(&opened), Arc::clone(&ended));
        async move {
            read_request(&mut stream).await;
            opened.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 64];
            while stream.read(&mut buf).await.is_ok_and(|n| n > 0) {}
            ended.fetch_add(1, Ordering::SeqCst);
        }
    })
    .await;
    let mut config = JwtConfig::new(base.clone(), [AUDIENCE])
        .with_jwks_url(format!("{base}/jwks"))
        .without_system_proxy();
    config.timeout = Duration::from_millis(600);
    let auth = JwtAuth::new(config).unwrap();
    let asking = {
        let auth = auth.clone();
        tokio::spawn(async move { auth.ready().await })
    };
    eventually("the issuer to be asked", || {
        connected.load(Ordering::SeqCst) == 1
    })
    .await;

    // The request and the authenticator are gone; the fetch gives up at the timeout, and the issuer sees it.
    asking.abort();
    let _ = asking.await;
    drop(auth);
    let gone = Instant::now();
    eventually("the fetch to end", || closed.load(Ordering::SeqCst) == 1).await;
    assert!(
        gone.elapsed() < Duration::from_secs(5),
        "the fetch of a dropped authenticator went on for {:?}",
        gone.elapsed()
    );
}

// ---- every request to the issuer is bounded -----------------------------------------------------------------------

/// The failure of `ready()` for an issuer, and how long it took, with a guard so that a request that is not
/// bounded fails the test instead of hanging it.
async fn timed_out(auth: &JwtAuth) -> (AuthError, Duration) {
    let began = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(8), auth.ready())
        .await
        .expect("the request to the issuer is bounded by the timeout");
    (result.unwrap_err(), began.elapsed())
}

fn config(base: &str) -> JwtConfig {
    let mut config = JwtConfig::new(base, [AUDIENCE]).without_system_proxy();
    config.timeout = Duration::from_millis(500);
    config
}

#[tokio::test]
async fn a_discovery_request_that_is_never_answered_times_out() {
    // The server accepts the connection, reads the request, and says nothing.
    let base = raw_server(|mut stream| async move {
        read_request(&mut stream).await;
        std::future::pending::<()>().await;
    })
    .await;
    let auth = JwtAuth::new(config(&base)).unwrap();
    let (error, took) = timed_out(&auth).await;
    assert!(matches!(error, AuthError::Unavailable { .. }), "{error:?}");
    assert!(
        took >= Duration::from_millis(450) && took < Duration::from_secs(3),
        "{took:?}"
    );
}

#[tokio::test]
async fn a_jwks_request_that_is_never_answered_times_out_after_a_good_discovery() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let document = json!({"issuer": base, "jwks_uri": format!("{base}/jwks")});
    let router = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || {
                let document = document.clone();
                async move { axum::Json(document) }
            }),
        )
        .route(
            "/jwks",
            get(|| async {
                std::future::pending::<()>().await;
                "never"
            }),
        );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let auth = JwtAuth::new(config(&base)).unwrap();
    let (error, took) = timed_out(&auth).await;
    assert!(matches!(error, AuthError::Unavailable { .. }), "{error:?}");
    assert!(
        took >= Duration::from_millis(450) && took < Duration::from_secs(3),
        "{took:?}"
    );
}

#[tokio::test]
async fn a_body_that_drips_in_times_out() {
    // The headers come at once and promise a lot; the body arrives a byte at a time, forever.
    let base = raw_server(|mut stream| async move {
        read_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100000\r\n\r\n",
            )
            .await
            .unwrap();
        loop {
            if stream.write_all(b" ").await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let auth = JwtAuth::new(config(&base).with_jwks_url(format!("{base}/jwks"))).unwrap();
    let (error, took) = timed_out(&auth).await;
    assert!(matches!(error, AuthError::Unavailable { .. }), "{error:?}");
    assert!(
        took >= Duration::from_millis(450) && took < Duration::from_secs(3),
        "the drip went on {took:?}: the timeout is not on the whole request"
    );
}
