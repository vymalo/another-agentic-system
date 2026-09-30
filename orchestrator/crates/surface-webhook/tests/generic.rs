//! `POST /webhooks/ci` over real HTTP, on the in-memory store and a clock the test holds: the
//! guard's refusals write nothing (checked by asking the store for any due row), a valid report
//! is stored once whatever the redeliveries (the key is a digest of the signed string, so a
//! replay under a new delivery id is the same delivery), a body that trickles in is cut off, the
//! identity layer of the rest of the API is unchanged, and the route never reads
//! `X-Auth-Request-Email`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_ports::{InboxItem, InboxStatus};
use orch_surface_webhook::signature::sign_generic;
use orch_surface_webhook::{GenericConfig, Secrets, generic};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const SECRET: &str = "dev-webhook-secret-0123456789abcdef0123";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const DELIVERY: &str = "0195c1a2-7b3e-7c11-8f2a-5d6e7f809a1b";
mod support;

use support::{NOW, Rig};

async fn start(secrets: &str, max_skew: u64) -> Rig {
    let cfg = GenericConfig {
        secrets: Secrets::parse(secrets).unwrap(),
        max_skew: Duration::from_secs(max_skew),
        read_timeout: Duration::from_secs(10),
    };
    Rig::start(|app| generic::routes(app, cfg)).await
}

/// The sender's side of the generic scheme, on a [`Rig`].
trait Generic {
    fn signed(
        &self,
        secret: &str,
        timestamp: i64,
        delivery: &str,
        body: &[u8],
    ) -> reqwest::RequestBuilder;
    async fn post(&self, body: &Value) -> reqwest::Response;
    /// The row stored for the delivery signed at `timestamp` with `body`.
    async fn row(&self, timestamp: i64, body: &[u8]) -> Option<InboxItem>;
}

/// The idempotency key of a delivery: the lower-case hex SHA-256 of `"<timestamp>.<body>"`.
fn key_of(timestamp: i64, body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{timestamp}.").as_bytes());
    hasher.update(body);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Generic for Rig {
    /// A signed request for `body` at `timestamp` (seconds), as a real sender builds it.
    fn signed(
        &self,
        secret: &str,
        timestamp: i64,
        delivery: &str,
        body: &[u8],
    ) -> reqwest::RequestBuilder {
        let ts = timestamp.to_string();
        let sig = sign_generic(secret, &ts, body).unwrap();
        self.client
            .post(format!("{}/webhooks/ci", self.base))
            .header("Content-Type", "application/json")
            .header("X-Vymalo-Delivery", delivery)
            .header("X-Vymalo-Timestamp", ts)
            .header("X-Vymalo-Signature-256", sig)
            .body(body.to_vec())
    }

    async fn post(&self, body: &Value) -> reqwest::Response {
        self.signed(SECRET, NOW, DELIVERY, body.to_string().as_bytes())
            .send()
            .await
            .unwrap()
    }

    async fn row(&self, timestamp: i64, body: &[u8]) -> Option<InboxItem> {
        self.find(generic::SOURCE, &key_of(timestamp, body)).await
    }
}

fn report() -> Value {
    json!({
        "version": 1,
        "repository": "https://github.com/acme/widgets",
        "sha": SHA,
        "branch": "agent/fix-flaky-test",
        "name": "ci/build",
        "conclusion": "success",
        "url": "https://ci.example.com/runs/42",
        "summary": "212 tests passed",
    })
}

#[tokio::test]
async fn a_signed_report_is_stored_and_answered_202() {
    let rig = start(SECRET, 300).await;
    let res = rig.post(&report()).await;
    assert_eq!(res.status(), 202);
    let row = rig
        .row(NOW, report().to_string().as_bytes())
        .await
        .expect("stored under the digest of the signed string");
    assert_eq!(row.status, InboxStatus::Pending);
    assert_eq!(row.kind, "ci_report");
    assert_eq!(
        row.correlation.as_deref(),
        Some(format!("ci:github.com/acme/widgets@{SHA}").as_str())
    );
    assert_eq!(
        row.payload,
        json!({
            "kind": "ci_report",
            "provider": "generic",
            "repository": "github.com/acme/widgets",
            "sha": SHA,
            "branch": "agent/fix-flaky-test",
            "name": "ci/build",
            "conclusion": "success",
            "url": "https://ci.example.com/runs/42",
            "summary": "212 tests passed",
        })
    );
}

#[tokio::test]
async fn a_redelivery_is_202_again_and_stored_once() {
    let rig = start(SECRET, 300).await;
    let body = report().to_string();
    assert_eq!(rig.post(&report()).await.status(), 202);
    let first = rig.row(NOW, body.as_bytes()).await.unwrap();
    // The same timestamp and body again: one row, kept, whatever else the request says.
    for delivery in [DELIVERY, "another-id", ""] {
        let res = rig
            .signed(SECRET, NOW, delivery, body.as_bytes())
            .send()
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            202,
            "a repeated delivery is acknowledged, not refused ({delivery:?})"
        );
    }
    let after = rig.row(NOW, body.as_bytes()).await.unwrap();
    assert_eq!(after.id, first.id);
    assert_eq!(rig.rows().await, 1, "one row for four deliveries");
}

/// The delivery id is not signed, so it cannot be what makes a delivery new: a captured request
/// replayed under a fresh id is the same delivery, and a delivery with no id at all is accepted.
#[tokio::test]
async fn a_replay_under_a_new_delivery_id_is_the_same_delivery() {
    let rig = start(SECRET, 300).await;
    let body = report().to_string();
    let ts = NOW.to_string();
    let sig = sign_generic(SECRET, &ts, body.as_bytes()).unwrap();
    let url = format!("{}/webhooks/ci", rig.base);
    // No delivery header at all.
    let res = rig
        .client
        .post(&url)
        .header("X-Vymalo-Timestamp", &ts)
        .header("X-Vymalo-Signature-256", &sig)
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202, "the delivery id is optional");
    // The captured request, replayed under ten fresh ids (some of them no UUID at all).
    for n in 0..10 {
        let res = rig
            .client
            .post(&url)
            .header("X-Vymalo-Delivery", format!("fresh-{n}"))
            .header("X-Vymalo-Timestamp", &ts)
            .header("X-Vymalo-Signature-256", &sig)
            .body(body.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 202, "replay {n}");
    }
    assert_eq!(rig.rows().await, 1, "the replays were the one delivery");
    // A genuinely new delivery (its own timestamp, signed) is a new row, even with the same body.
    rig.clock.advance(Duration::from_secs(5));
    let res = rig
        .signed(SECRET, NOW + 5, DELIVERY, body.as_bytes())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202);
    assert_eq!(rig.rows().await, 2);
}

#[tokio::test]
async fn a_refused_delivery_writes_nothing() {
    let rig = start(SECRET, 300).await;
    let body = report().to_string();
    let body = body.as_bytes();
    let ts = NOW.to_string();
    let good_sig = sign_generic(SECRET, &ts, body).unwrap();

    let url = format!("{}/webhooks/ci", rig.base);
    // Each case: the headers as sent (None leaves the header out).
    type Headers<'a> = [(&'a str, Option<String>); 3];
    let cases: Vec<(&str, Headers)> = vec![
        (
            "no signature",
            [
                ("X-Vymalo-Signature-256", None),
                ("X-Vymalo-Timestamp", Some(ts.clone())),
                ("X-Vymalo-Delivery", Some(DELIVERY.into())),
            ],
        ),
        (
            "no timestamp",
            [
                ("X-Vymalo-Signature-256", Some(good_sig.clone())),
                ("X-Vymalo-Timestamp", None),
                ("X-Vymalo-Delivery", Some(DELIVERY.into())),
            ],
        ),
        (
            "a signature of another body",
            [
                (
                    "X-Vymalo-Signature-256",
                    Some(sign_generic(SECRET, &ts, b"{}").unwrap()),
                ),
                ("X-Vymalo-Timestamp", Some(ts.clone())),
                ("X-Vymalo-Delivery", Some(DELIVERY.into())),
            ],
        ),
        (
            "a wrong secret",
            [
                (
                    "X-Vymalo-Signature-256",
                    Some(sign_generic("nope", &ts, body).unwrap()),
                ),
                ("X-Vymalo-Timestamp", Some(ts.clone())),
                ("X-Vymalo-Delivery", Some(DELIVERY.into())),
            ],
        ),
        (
            "a signature without its prefix",
            [
                (
                    "X-Vymalo-Signature-256",
                    Some(good_sig.trim_start_matches("sha256=").to_owned()),
                ),
                ("X-Vymalo-Timestamp", Some(ts.clone())),
                ("X-Vymalo-Delivery", Some(DELIVERY.into())),
            ],
        ),
        (
            "a signature over the body alone",
            [
                (
                    "X-Vymalo-Signature-256",
                    Some(orch_surface_webhook::signature::sign_generic(SECRET, "", body).unwrap()),
                ),
                ("X-Vymalo-Timestamp", Some(ts.clone())),
                ("X-Vymalo-Delivery", Some(DELIVERY.into())),
            ],
        ),
        (
            "a timestamp that was not signed",
            [
                ("X-Vymalo-Signature-256", Some(good_sig.clone())),
                ("X-Vymalo-Timestamp", Some((NOW + 1).to_string())),
                ("X-Vymalo-Delivery", Some(DELIVERY.into())),
            ],
        ),
        (
            "a signed but stale timestamp",
            [
                (
                    "X-Vymalo-Signature-256",
                    Some(sign_generic(SECRET, &(NOW - 301).to_string(), body).unwrap()),
                ),
                ("X-Vymalo-Timestamp", Some((NOW - 301).to_string())),
                ("X-Vymalo-Delivery", Some(DELIVERY.into())),
            ],
        ),
        (
            "a signed timestamp from the future",
            [
                (
                    "X-Vymalo-Signature-256",
                    Some(sign_generic(SECRET, &(NOW + 301).to_string(), body).unwrap()),
                ),
                ("X-Vymalo-Timestamp", Some((NOW + 301).to_string())),
                ("X-Vymalo-Delivery", Some(DELIVERY.into())),
            ],
        ),
    ];
    for (why, headers) in cases {
        let mut req = rig.client.post(&url).body(body.to_vec());
        for (name, value) in headers {
            if let Some(value) = value {
                req = req.header(name, value);
            }
        }
        let res = req.send().await.unwrap();
        assert_eq!(res.status(), 401, "{why}");
        assert_eq!(
            res.headers()["content-type"],
            "application/problem+json",
            "{why}: an RFC 9457 problem"
        );
    }
    // A timestamp that is not plain digits is refused even when it is signed as written.
    for odd in [
        "+1790769600",
        " 1790769600",
        "1790769600.0",
        "-1",
        "",
        "1e9",
        "0x6ac0a8c0",
        "17907696000000",
    ] {
        let sig = sign_generic(SECRET, odd, body).unwrap();
        let res = rig
            .client
            .post(&url)
            .header("X-Vymalo-Delivery", DELIVERY)
            .header("X-Vymalo-Timestamp", odd)
            .header("X-Vymalo-Signature-256", sig)
            .body(body.to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401, "timestamp {odd:?}");
    }
    // A GET is guarded like the POST: 401, not a hint about the route.
    let res = rig.client.get(&url).send().await.unwrap();
    assert_eq!(res.status(), 401);

    assert!(!rig.holds_a_row().await, "not one of them wrote a row");
}

#[tokio::test]
async fn the_skew_window_is_inclusive_and_follows_the_clock() {
    let rig = start(SECRET, 60).await;
    let body = report().to_string();
    for (offset, want) in [(-60, 202), (60, 202), (-61, 401), (61, 401), (0, 202)] {
        let delivery = format!("0195c1a2-7b3e-7c11-8f2a-5d6e7f8{:05}", offset + 100);
        let res = rig
            .signed(SECRET, NOW + offset, &delivery, body.as_bytes())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), want, "offset {offset}");
    }
    // The clock moves, and so does the window: what was fresh is now stale.
    rig.clock.advance(Duration::from_secs(61));
    let res = rig
        .signed(
            SECRET,
            NOW,
            "0195c1a2-7b3e-7c11-8f2a-5d6e7f809a99",
            body.as_bytes(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn either_secret_of_a_rotation_is_accepted() {
    let new = "new-secret-new-secret-new-secret-new";
    let old = "old-secret-old-secret-old-secret-old";
    let rig = start(&format!("{new},{old}"), 300).await;
    for (n, secret) in [new, old].into_iter().enumerate() {
        let delivery = format!("0195c1a2-7b3e-7c11-8f2a-5d6e7f809a0{n}");
        let res = rig
            .signed(secret, NOW, &delivery, report().to_string().as_bytes())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 202, "{secret}");
    }
    let res = rig
        .signed(
            "retired-secret-retired-secret-retired",
            NOW,
            "0195c1a2-7b3e-7c11-8f2a-5d6e7f809a05",
            report().to_string().as_bytes(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn a_body_over_256_kib_is_413_before_and_after_the_signature() {
    let rig = start(SECRET, 300).await;
    let mut big = report();
    big["summary"] = json!("x".repeat(generic::MAX_BODY_BYTES));
    let body = big.to_string();
    assert!(body.len() > generic::MAX_BODY_BYTES);

    // Declared: refused without reading it.
    let res = rig
        .signed(SECRET, NOW, DELIVERY, body.as_bytes())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 413);
    assert_eq!(res.headers()["content-type"], "application/problem+json");

    // Undeclared (chunked): the read itself is bounded.
    let ts = NOW.to_string();
    let sig = sign_generic(SECRET, &ts, body.as_bytes()).unwrap();
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> = body
        .as_bytes()
        .chunks(16 * 1024)
        .map(|c| Ok(c.to_vec()))
        .collect();
    let res = rig
        .client
        .post(format!("{}/webhooks/ci", rig.base))
        .header("X-Vymalo-Delivery", DELIVERY)
        .header("X-Vymalo-Timestamp", ts)
        .header("X-Vymalo-Signature-256", sig)
        .body(reqwest::Body::wrap_stream(futures::stream::iter(chunks)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 413);

    // Exactly at the limit is read (and is then a 400 for what it says, not a 413).
    let mut at_limit = vec![b' '; generic::MAX_BODY_BYTES];
    at_limit[0] = b'{';
    at_limit[generic::MAX_BODY_BYTES - 1] = b'}';
    let res = rig
        .signed(SECRET, NOW, DELIVERY, &at_limit)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    assert!(!rig.holds_a_row().await);
}

#[tokio::test]
async fn a_signed_body_that_is_not_a_report_is_400_and_writes_nothing() {
    let rig = start(SECRET, 300).await;
    let with = |edit: &dyn Fn(&mut Value)| {
        let mut v = report();
        edit(&mut v);
        v
    };
    let bad: Vec<(&str, Vec<u8>)> = vec![
        ("not JSON", b"not json".to_vec()),
        ("an array", b"[]".to_vec()),
        ("empty", Vec::new()),
        (
            "no sha",
            with(&|v| {
                v.as_object_mut().unwrap().remove("sha");
            })
            .to_string()
            .into_bytes(),
        ),
        (
            "no name",
            with(&|v| {
                v.as_object_mut().unwrap().remove("name");
            })
            .to_string()
            .into_bytes(),
        ),
        (
            "no repository",
            with(&|v| {
                v.as_object_mut().unwrap().remove("repository");
            })
            .to_string()
            .into_bytes(),
        ),
        (
            "no version",
            with(&|v| {
                v.as_object_mut().unwrap().remove("version");
            })
            .to_string()
            .into_bytes(),
        ),
        (
            "no conclusion",
            with(&|v| {
                v.as_object_mut().unwrap().remove("conclusion");
            })
            .to_string()
            .into_bytes(),
        ),
        (
            "a mistyped name",
            with(&|v| v["name"] = json!(7)).to_string().into_bytes(),
        ),
        (
            "a string version",
            with(&|v| v["version"] = json!("1"))
                .to_string()
                .into_bytes(),
        ),
        (
            "version 2",
            with(&|v| v["version"] = json!(2)).to_string().into_bytes(),
        ),
        (
            "version 0",
            with(&|v| v["version"] = json!(0)).to_string().into_bytes(),
        ),
        (
            "a short sha",
            with(&|v| v["sha"] = json!("abc123"))
                .to_string()
                .into_bytes(),
        ),
        (
            "a sha that is not hex",
            with(&|v| v["sha"] = json!("g".repeat(40)))
                .to_string()
                .into_bytes(),
        ),
        (
            "a repository that is no address",
            with(&|v| v["repository"] = json!("nonsense"))
                .to_string()
                .into_bytes(),
        ),
        (
            "an unknown conclusion",
            with(&|v| v["conclusion"] = json!("passed"))
                .to_string()
                .into_bytes(),
        ),
        (
            "a conclusion of another case",
            with(&|v| v["conclusion"] = json!("Success"))
                .to_string()
                .into_bytes(),
        ),
        (
            "startup_failure, which is GitHub's alone",
            with(&|v| v["conclusion"] = json!("startup_failure"))
                .to_string()
                .into_bytes(),
        ),
        (
            "a blank name",
            with(&|v| v["name"] = json!("  ")).to_string().into_bytes(),
        ),
    ];
    for (why, body) in bad {
        let res = rig
            .signed(SECRET, NOW, DELIVERY, &body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400, "{why}");
        assert_eq!(
            res.headers()["content-type"],
            "application/problem+json",
            "{why}"
        );
    }
    assert!(!rig.holds_a_row().await, "not one of them wrote a row");
}

#[tokio::test]
async fn the_optional_members_are_optional_and_unknown_ones_are_ignored() {
    let rig = start(SECRET, 300).await;
    let minimal = json!({
        "version": 1,
        "repository": "git@github.com:Acme/Widgets.git",
        "sha": SHA.to_uppercase(),
        "name": "ci/build",
        "conclusion": "failure",
        "surprise": {"nested": true},
    });
    assert_eq!(rig.post(&minimal).await.status(), 202);
    let row = rig.row(NOW, minimal.to_string().as_bytes()).await.unwrap();
    assert_eq!(row.payload["repository"], "github.com/acme/widgets");
    assert_eq!(row.payload["sha"], SHA, "stored lower case");
    assert_eq!(row.payload["conclusion"], "failure");
    for absent in ["branch", "url", "summary"] {
        assert!(row.payload.get(absent).is_none(), "{absent}");
    }
}

#[tokio::test]
async fn only_http_links_survive_and_a_long_summary_is_cut() {
    let rig = start(SECRET, 300).await;
    for (n, (url, kept)) in [
        ("https://ci.example.com/r/1", true),
        ("http://ci.example.com/r/1", true),
        (" HTTPS://CI.Example.com/a b ", true),
        ("javascript:alert(1)", false),
        ("file:///etc/passwd", false),
        ("data:text/html,x", false),
        ("not a url", false),
    ]
    .into_iter()
    .enumerate()
    {
        let mut body = report();
        body["url"] = json!(url);
        let delivery = format!("0195c1a2-7b3e-7c11-8f2a-5d6e7f8000{n:02}");
        let res = rig
            .signed(SECRET, NOW, &delivery, body.to_string().as_bytes())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 202, "{url}");
        let row = rig.row(NOW, body.to_string().as_bytes()).await.unwrap();
        assert_eq!(row.payload.get("url").is_some(), kept, "{url}");
    }
    // What is kept is the URL as parsed, not the text that was sent.
    let mut body = report();
    body["url"] = json!(" HTTPS://CI.Example.com/a b ");
    let row = rig.row(NOW, body.to_string().as_bytes()).await.unwrap();
    assert_eq!(row.payload["url"], "https://ci.example.com/a%20b");
    // 16 KiB of two-byte characters, cut on a character boundary at 16 KiB.
    let mut body = report();
    body["summary"] = json!("é".repeat(20_000));
    assert_eq!(rig.post(&body).await.status(), 202);
    let summary = rig
        .row(NOW, body.to_string().as_bytes())
        .await
        .unwrap()
        .payload["summary"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(summary.len(), generic::MAX_SUMMARY_BYTES);
    assert!(summary.chars().all(|c| c == 'é'));
}

#[tokio::test]
async fn the_route_needs_no_identity_and_the_rest_of_the_api_still_does() {
    let rig = start(SECRET, 300).await;
    // No X-Auth-Request-Email: the webhook is a machine route.
    assert_eq!(rig.post(&report()).await.status(), 202);
    // An identity header changes nothing (it is not read) ...
    let delivery = "0195c1a2-7b3e-7c11-8f2a-5d6e7f809a02";
    let res = rig
        .signed(SECRET, NOW, delivery, report().to_string().as_bytes())
        .header("X-Auth-Request-Email", "mallory@example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202);
    // ... and it does not stand for a signature either.
    let res = rig
        .client
        .post(format!("{}/webhooks/ci", rig.base))
        .header("X-Auth-Request-Email", "alice@example.com")
        .body(report().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    // Every other route is still behind the identity layer.
    for path in ["/api/agents", "/api/threads", "/webhooks/other"] {
        let res = rig
            .client
            .get(format!("{}{path}", rig.base))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401, "{path}");
    }
    let res = rig
        .client
        .get(format!("{}/api/agents", rig.base))
        .header("X-Auth-Request-Email", "alice@example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    // And health stays open.
    let res = rig
        .client
        .get(format!("{}/healthz", rig.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}

#[tokio::test]
async fn a_name_over_256_bytes_is_400() {
    let rig = start(SECRET, 300).await;
    let mut body = report();
    body["name"] = json!("n".repeat(generic::MAX_NAME_BYTES + 1));
    assert_eq!(rig.post(&body).await.status(), 400);
    body["name"] = json!("n".repeat(generic::MAX_NAME_BYTES));
    assert_eq!(rig.post(&body).await.status(), 202);
}

/// A body that declares 200 KB (or nothing) and then trickles or stalls holds neither a
/// connection nor a buffer for long: the route cuts it off with 408, and writes nothing. (The
/// buffer is not sized from the declared length; that is the code's, this test is the clock's.)
#[tokio::test]
async fn a_body_that_stalls_is_cut_off_with_408_and_writes_nothing() {
    let cfg = GenericConfig {
        secrets: Secrets::parse(SECRET).unwrap(),
        max_skew: Duration::from_secs(300),
        read_timeout: Duration::from_millis(300),
    };
    let rig = Rig::start(|app| generic::routes(app, cfg)).await;
    let ts = NOW.to_string();
    let sig = sign_generic(SECRET, &ts, b"{}").unwrap();
    for declared in [Some("200000"), None] {
        let stalled =
            futures::stream::once(async { Ok::<_, std::io::Error>(b"{\"version\":".to_vec()) })
                .chain(futures::stream::pending());
        let mut req = rig
            .client
            .post(format!("{}/webhooks/ci", rig.base))
            .header("X-Vymalo-Timestamp", &ts)
            .header("X-Vymalo-Signature-256", &sig)
            .body(reqwest::Body::wrap_stream(stalled));
        if let Some(length) = declared {
            req = req.header("Content-Length", length);
        }
        let started = std::time::Instant::now();
        let res = tokio::time::timeout(Duration::from_secs(5), req.send())
            .await
            .expect("the route answers instead of waiting for the body")
            .unwrap();
        assert_eq!(res.status(), 408, "declared {declared:?}");
        assert!(started.elapsed() < Duration::from_secs(4), "{declared:?}");
    }
    assert!(!rig.holds_a_row().await);
    // A prompt delivery on the same route is unaffected.
    assert_eq!(rig.post(&report()).await.status(), 202);
}
