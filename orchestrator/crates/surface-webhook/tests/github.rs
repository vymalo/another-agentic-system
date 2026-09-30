//! `POST /webhooks/github` over real HTTP, on the in-memory store and a clock the test holds, with
//! the fixtures of `testdata/github` (synthetic, shaped after GitHub's documented schemas; see the
//! README there): the two completed events become the report they should, every other event or
//! action (`check_suite` included) is acknowledged and not stored, so are the reports that must
//! not count (an unnamed workflow, a fork's code, an event older than the maximum age), `ping` is
//! 204, the idempotency key comes from the signed body and not from the delivery id, a body that
//! stalls is cut off, the guard's refusals write nothing, and the route never reads
//! `X-Auth-Request-Email`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use orch_ports::{InboxItem, InboxStatus};
use orch_surface_webhook::signature::{sign_generic, sign_github};
use orch_surface_webhook::{GithubConfig, Secrets, github};
use serde_json::{Value, json};
use support::Rig;

const SECRET: &str = "dev-webhook-secret-0123456789abcdef0123";
const DELIVERY: &str = "72d3162e-cc78-11e3-81ab-4c9367dc0958";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

async fn start(secrets: &str) -> Rig {
    let cfg = GithubConfig::new(Secrets::parse(secrets).unwrap());
    Rig::start(|app| github::routes(app, cfg)).await
}

/// The key the route stores a delivery under, read off the signed body as the docs say:
/// `check_run:<id>:<completed_at>` or `workflow_run:<id>:<run_attempt>`.
fn key_of(event: &str, body: &[u8]) -> String {
    let v: Value = serde_json::from_slice(body).unwrap();
    match event {
        "check_run" => format!(
            "check_run:{}:{}",
            v["check_run"]["id"],
            v["check_run"]["completed_at"].as_str().unwrap()
        ),
        _ => format!(
            "workflow_run:{}:{}",
            v["workflow_run"]["id"], v["workflow_run"]["run_attempt"]
        ),
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("testdata/github")
        .join(format!("{name}.json"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// What GitHub sends: the event, the delivery id and the signature of the raw body.
fn delivery(
    rig: &Rig,
    secret: &str,
    event: &str,
    id: &str,
    body: &[u8],
) -> reqwest::RequestBuilder {
    rig.client
        .post(format!("{}/webhooks/github", rig.base))
        .header("Content-Type", "application/json")
        .header("User-Agent", "GitHub-Hookshot/044aadd")
        .header("X-GitHub-Event", event)
        .header("X-GitHub-Delivery", id)
        .header("X-Hub-Signature-256", sign_github(secret, body).unwrap())
        .body(body.to_vec())
}

async fn send(rig: &Rig, event: &str, body: &[u8]) -> u16 {
    delivery(rig, SECRET, event, DELIVERY, body)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn row(rig: &Rig, event: &str, body: &[u8]) -> Option<InboxItem> {
    rig.find(github::SOURCE, &key_of(event, body)).await
}

/// `fixture(file)` edited by `f`, as the bytes GitHub would sign.
fn edited(file: &str, f: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut v: Value = serde_json::from_slice(&fixture(file)).unwrap();
    f(&mut v);
    v.to_string().into_bytes()
}

/// `(file, event, what the report must be)`.
struct Accepted {
    file: &'static str,
    event: &'static str,
    name: &'static str,
    conclusion: &'static str,
    branch: Option<&'static str>,
    url: Option<&'static str>,
    summary: Option<&'static str>,
}

const ACCEPTED: &[Accepted] = &[
    Accepted {
        file: "check_run.completed.failure",
        event: "check_run",
        name: "build",
        conclusion: "failure",
        branch: Some("agent/fix-flaky-test"),
        url: Some("https://github.com/Acme/Widgets/runs/128620228"),
        summary: Some(
            "2 tests failed: `parser::empty`, `parser::utf8`\n\n<script>alert(1)</script> [details](javascript:alert(1))",
        ),
    },
    Accepted {
        file: "check_run.completed.failure_no_branch",
        event: "check_run",
        name: "build",
        conclusion: "failure",
        branch: None,
        url: Some("https://github.com/Acme/Widgets/runs/128620228"),
        summary: Some(
            "2 tests failed: `parser::empty`, `parser::utf8`\n\n<script>alert(1)</script> [details](javascript:alert(1))",
        ),
    },
    Accepted {
        file: "check_run.completed.unknown_conclusion",
        event: "check_run",
        name: "build",
        conclusion: "failure",
        branch: Some("agent/fix-flaky-test"),
        url: Some("https://github.com/Acme/Widgets/runs/128620228"),
        summary: Some(
            "2 tests failed: `parser::empty`, `parser::utf8`\n\n<script>alert(1)</script> [details](javascript:alert(1))",
        ),
    },
    Accepted {
        file: "check_run.completed.null_conclusion",
        event: "check_run",
        name: "build",
        conclusion: "failure",
        branch: Some("agent/fix-flaky-test"),
        url: Some("https://github.com/Acme/Widgets/runs/128620228"),
        summary: Some(
            "2 tests failed: `parser::empty`, `parser::utf8`\n\n<script>alert(1)</script> [details](javascript:alert(1))",
        ),
    },
    Accepted {
        file: "check_run.completed.same_repository_pull_request",
        event: "check_run",
        name: "build",
        conclusion: "failure",
        branch: Some("agent/fix-flaky-test"),
        url: Some("https://github.com/Acme/Widgets/runs/128620228"),
        summary: Some(
            "2 tests failed: `parser::empty`, `parser::utf8`\n\n<script>alert(1)</script> [details](javascript:alert(1))",
        ),
    },
    Accepted {
        file: "check_run.completed.success_no_summary",
        event: "check_run",
        name: "lint",
        conclusion: "success",
        branch: Some("agent/fix-flaky-test"),
        url: Some("https://github.com/Acme/Widgets/runs/128620228"),
        summary: None,
    },
    Accepted {
        file: "check_run.completed.timed_out",
        event: "check_run",
        name: "e2e",
        conclusion: "timed_out",
        branch: Some("agent/fix-flaky-test"),
        url: Some("https://github.com/Acme/Widgets/runs/128620228"),
        summary: Some("The job exceeded the maximum execution time of 360 minutes"),
    },
    Accepted {
        file: "workflow_run.completed.success",
        event: "workflow_run",
        name: "CI",
        conclusion: "success",
        branch: Some("agent/fix-flaky-test"),
        url: Some("https://github.com/Acme/Widgets/actions/runs/31415926"),
        summary: None,
    },
    Accepted {
        file: "workflow_run.completed.startup_failure",
        event: "workflow_run",
        name: "CI",
        conclusion: "startup_failure",
        branch: Some("agent/fix-flaky-test"),
        url: Some("https://github.com/Acme/Widgets/actions/runs/31415926"),
        summary: None,
    },
];

/// Acknowledged, not stored: the same events with another action, another event (`check_suite`,
/// which names the app and not a check), and the reports that must not count.
const IGNORED: &[(&str, &str)] = &[
    ("check_suite.completed.success", "check_suite"),
    ("check_run.created", "check_run"),
    ("check_run.rerequested", "check_run"),
    ("workflow_run.requested", "workflow_run"),
    ("push", "push"),
    // A workflow with no name: a required check is named, and "workflow" would name every
    // unnamed one alike.
    ("workflow_run.completed.unnamed", "workflow_run"),
    // A run of a fork's code; its workflow names are the fork's to choose.
    ("workflow_run.completed.fork", "workflow_run"),
    ("check_run.completed.fork_pull_request", "check_run"),
];

#[tokio::test]
async fn the_two_completed_events_become_the_report_they_should() {
    for case in ACCEPTED {
        let rig = start(SECRET).await;
        let status = send(&rig, case.event, &fixture(case.file)).await;
        assert_eq!(status, 202, "{}", case.file);
        let stored = row(&rig, case.event, &fixture(case.file))
            .await
            .unwrap_or_else(|| panic!("{}: not stored", case.file));
        assert_eq!(stored.status, InboxStatus::Pending);
        assert_eq!(stored.kind, "ci_report");
        assert_eq!(
            stored.correlation.as_deref(),
            Some(format!("ci:github.com/acme/widgets@{SHA}").as_str()),
            "{}: the watch key is the lower-cased repository and the commit",
            case.file
        );
        let mut want = json!({
            "kind": "ci_report",
            "provider": "github",
            "repository": "github.com/acme/widgets",
            "sha": SHA,
            "name": case.name,
            "conclusion": case.conclusion,
        });
        for (key, value) in [
            ("branch", case.branch),
            ("url", case.url),
            ("summary", case.summary),
        ] {
            if let Some(value) = value {
                want[key] = json!(value);
            }
        }
        assert_eq!(stored.payload, want, "{}", case.file);
    }
}

#[tokio::test]
async fn every_other_event_and_action_is_acknowledged_and_not_stored() {
    let rig = start(SECRET).await;
    for (file, event) in IGNORED {
        assert_eq!(send(&rig, event, &fixture(file)).await, 202, "{file}");
    }
    // Anything else GitHub might send, even what is not JSON at all.
    for event in [
        "issues",
        "star",
        "deployment_status",
        "check_suite",
        "check_suite_unknown",
    ] {
        assert_eq!(send(&rig, event, b"not even json").await, 202, "{event}");
    }
    assert!(!rig.holds_a_row().await);
}

#[tokio::test]
async fn an_unsigned_delivery_is_never_acknowledged_whatever_the_event() {
    let rig = start(SECRET).await;
    for event in ["push", "ping", "check_run", "issues"] {
        let res = rig
            .client
            .post(format!("{}/webhooks/github", rig.base))
            .header("X-GitHub-Event", event)
            .header("X-GitHub-Delivery", DELIVERY)
            .body(fixture("push"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401, "{event}");
    }
}

#[tokio::test]
async fn ping_is_204_and_stores_nothing() {
    let rig = start(SECRET).await;
    let res = delivery(&rig, SECRET, "ping", DELIVERY, &fixture("ping"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert!(res.bytes().await.unwrap().is_empty());
    // A ping that is not signed is refused like any delivery.
    let res = delivery(&rig, "guess", "ping", DELIVERY, &fixture("ping"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    assert!(!rig.holds_a_row().await);
}

#[tokio::test]
async fn a_redelivery_is_202_again_and_stored_once() {
    let rig = start(SECRET).await;
    let body = fixture("check_run.completed.failure");
    assert_eq!(send(&rig, "check_run", &body).await, 202);
    let first = row(&rig, "check_run", &body).await.unwrap();
    assert_eq!(send(&rig, "check_run", &body).await, 202);
    // The same report under any delivery id (GitHub's "Redeliver" makes a new one, and an
    // attacker with a captured request can make any): the key is the signed body's.
    for id in ["0a1b2c3d-0000-0000-0000-000000000002", "fresh", ""] {
        let res = delivery(&rig, SECRET, "check_run", id, &body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 202, "{id:?}");
    }
    assert_eq!(rig.rows().await, 1, "one row for five deliveries");
    assert_eq!(
        row(&rig, "check_run", &body).await.unwrap().id,
        first.id,
        "the first is kept"
    );
    // The fixture of a timed-out run has the same id and completion time: the same report, the
    // first kept. The same run completing again later is another report.
    assert_eq!(
        send(&rig, "check_run", &fixture("check_run.completed.timed_out")).await,
        202
    );
    let rerun = edited("check_run.completed.failure", |v| {
        v["check_run"]["completed_at"] = json!("2026-09-30T12:07:00Z")
    });
    assert_eq!(send(&rig, "check_run", &rerun).await, 202);
    assert_eq!(
        rig.rows().await,
        1,
        "the re-run; the first was claimed above"
    );
    // A workflow run is keyed by its id and attempt.
    let run = fixture("workflow_run.completed.success");
    assert_eq!(send(&rig, "workflow_run", &run).await, 202);
    assert_eq!(send(&rig, "workflow_run", &run).await, 202);
    let second_attempt = edited("workflow_run.completed.success", |v| {
        v["workflow_run"]["run_attempt"] = json!(2)
    });
    assert_eq!(send(&rig, "workflow_run", &second_attempt).await, 202);
    assert_eq!(rig.rows().await, 2, "the run and its second attempt");
    assert!(
        row(&rig, "workflow_run", &run).await.is_some()
            && row(&rig, "workflow_run", &second_attempt).await.is_some()
    );
}

/// An event dated before `now - WEBHOOK_GITHUB_MAX_AGE_SECS` is a replay of something old.
#[tokio::test]
async fn an_event_older_than_the_maximum_age_is_acknowledged_and_not_stored() {
    let cfg = GithubConfig {
        max_age: Duration::from_secs(3600),
        ..GithubConfig::new(Secrets::parse(SECRET).unwrap())
    };
    let rig = Rig::start(|app| github::routes(app, cfg)).await;
    let run = fixture("check_run.completed.failure"); // completed 2026-09-30T12:01:40Z
    let workflow = fixture("workflow_run.completed.success"); // updated 2026-09-30T12:01:44Z
    // The clock is at 12:00:00Z: both are fresh (an event slightly ahead of us is not old).
    assert_eq!(send(&rig, "check_run", &run).await, 202);
    assert_eq!(send(&rig, "workflow_run", &workflow).await, 202);
    assert_eq!(rig.rows().await, 2);
    // A day later the same, redelivered by an attacker under new ids, is acknowledged and
    // stored nowhere (a row is there only if it was stored before: it is not stored again).
    let other_run = edited("check_run.completed.failure", |v| {
        v["check_run"]["id"] = json!(7)
    });
    let other_workflow = edited("workflow_run.completed.success", |v| {
        v["workflow_run"]["id"] = json!(8)
    });
    rig.clock.advance(Duration::from_secs(3600 + 200));
    assert_eq!(send(&rig, "check_run", &other_run).await, 202);
    assert_eq!(send(&rig, "workflow_run", &other_workflow).await, 202);
    assert!(row(&rig, "check_run", &other_run).await.is_none());
    assert!(row(&rig, "workflow_run", &other_workflow).await.is_none());
    // Just inside the window is stored.
    let fresh = edited("check_run.completed.failure", |v| {
        v["check_run"]["id"] = json!(9);
        v["check_run"]["completed_at"] = json!("2026-09-30T12:20:00Z");
    });
    assert_eq!(send(&rig, "check_run", &fresh).await, 202);
    assert!(row(&rig, "check_run", &fresh).await.is_some());
    // The default is a day.
    assert_eq!(github::DEFAULT_MAX_AGE_SECS, 86_400);
    assert_eq!(
        GithubConfig::new(Secrets::parse(SECRET).unwrap()).max_age,
        Duration::from_secs(86_400)
    );
}

/// A body that stalls is cut off with 408 (the buffer is not sized from what it declares).
#[tokio::test]
async fn a_body_that_stalls_is_cut_off_with_408_and_writes_nothing() {
    let cfg = GithubConfig {
        read_timeout: Duration::from_millis(300),
        ..GithubConfig::new(Secrets::parse(SECRET).unwrap())
    };
    let rig = Rig::start(|app| github::routes(app, cfg)).await;
    for declared in [Some("5000000"), None] {
        let stalled =
            futures::stream::once(async { Ok::<_, std::io::Error>(b"{\"action\":".to_vec()) })
                .chain(futures::stream::pending());
        let mut req = rig
            .client
            .post(format!("{}/webhooks/github", rig.base))
            .header("X-GitHub-Event", "check_run")
            .header("X-Hub-Signature-256", sign_github(SECRET, b"{}").unwrap())
            .body(reqwest::Body::wrap_stream(stalled));
        if let Some(length) = declared {
            req = req.header("Content-Length", length);
        }
        let res = tokio::time::timeout(Duration::from_secs(5), req.send())
            .await
            .expect("the route answers instead of waiting for the body")
            .unwrap();
        assert_eq!(res.status(), 408, "declared {declared:?}");
    }
    assert!(!rig.holds_a_row().await);
    assert_eq!(
        send(&rig, "check_run", &fixture("check_run.completed.failure")).await,
        202,
        "a prompt delivery is unaffected"
    );
}

#[tokio::test]
async fn a_refused_delivery_writes_nothing() {
    let rig = start(SECRET).await;
    let body = fixture("check_run.completed.failure");
    let url = format!("{}/webhooks/github", rig.base);
    let post = |signature: Option<String>| {
        let mut req = rig
            .client
            .post(&url)
            .header("X-GitHub-Event", "check_run")
            .header("X-GitHub-Delivery", DELIVERY)
            .body(body.clone());
        if let Some(signature) = signature {
            req = req.header("X-Hub-Signature-256", signature);
        }
        async move { req.send().await.unwrap().status().as_u16() }
    };
    let good = sign_github(SECRET, &body).unwrap();
    let hex = good.trim_start_matches("sha256=").to_owned();
    let cases: Vec<(&str, Option<String>)> = vec![
        ("no signature", None),
        ("a wrong secret", sign_github("guess", &body)),
        ("the signature of another body", sign_github(SECRET, b"{}")),
        ("no prefix", Some(hex.clone())),
        ("the old sha1 header's prefix", Some(format!("sha1={hex}"))),
        ("a short signature", Some(good[..good.len() - 2].to_owned())),
        (
            "a generic signature (timestamp and body)",
            sign_generic(SECRET, "1", &body),
        ),
        ("empty", Some(String::new())),
    ];
    for (why, signature) in cases {
        assert_eq!(post(signature).await, 401, "{why}");
    }
    // A GET is guarded like the POST.
    assert_eq!(rig.client.get(&url).send().await.unwrap().status(), 401);
    assert!(!rig.holds_a_row().await, "not one of them wrote a row");
    // And the right signature works on the same request.
    assert_eq!(post(Some(good)).await, 202);
}

#[tokio::test]
async fn a_body_over_5_mib_is_413_declared_and_chunked() {
    let rig = start(SECRET).await;
    let body = vec![b'x'; github::MAX_BODY_BYTES + 1];
    let res = delivery(&rig, SECRET, "push", DELIVERY, &body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 413);
    assert_eq!(res.headers()["content-type"], "application/problem+json");
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
        body.chunks(64 * 1024).map(|c| Ok(c.to_vec())).collect();
    let res = rig
        .client
        .post(format!("{}/webhooks/github", rig.base))
        .header("X-GitHub-Event", "push")
        .header("X-GitHub-Delivery", DELIVERY)
        .header("X-Hub-Signature-256", sign_github(SECRET, &body).unwrap())
        .body(reqwest::Body::wrap_stream(futures::stream::iter(chunks)))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 413);
    // Exactly at the limit is read: an event this route ignores is acknowledged.
    let at_limit = vec![b'x'; github::MAX_BODY_BYTES];
    assert_eq!(send(&rig, "push", &at_limit).await, 202);
    assert!(!rig.holds_a_row().await);
}

/// A signed delivery of an accepted event that does not say what it is read from is a 400.
#[tokio::test]
async fn a_signed_payload_that_lacks_what_it_is_read_from_is_400_and_writes_nothing() {
    let rig = start(SECRET).await;
    let edit = |file: &str, f: &dyn Fn(&mut Value)| {
        let mut v: Value = serde_json::from_slice(&fixture(file)).unwrap();
        f(&mut v);
        v.to_string().into_bytes()
    };
    let remove = |v: &mut Value, pointer: &str| {
        let (parent, key) = pointer.rsplit_once('/').unwrap();
        v.pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(key);
    };
    let cases: Vec<(&str, &str, Vec<u8>)> = vec![
        ("not JSON", "check_run", b"{".to_vec()),
        ("not an object", "check_run", b"[]".to_vec()),
        (
            "no action",
            "check_run",
            edit("check_run.completed.failure", &|v| remove(v, "/action")),
        ),
        (
            "no repository",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                remove(v, "/repository/html_url")
            }),
        ),
        (
            "a repository that is no address",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                v["repository"]["html_url"] = json!("nonsense")
            }),
        ),
        (
            "no check_run",
            "check_run",
            edit("check_run.completed.failure", &|v| remove(v, "/check_run")),
        ),
        (
            "no check name",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                remove(v, "/check_run/name")
            }),
        ),
        (
            "a blank check name",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                v["check_run"]["name"] = json!(" ")
            }),
        ),
        (
            "no commit (run)",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                remove(v, "/check_run/head_sha")
            }),
        ),
        (
            "a short commit",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                v["check_run"]["head_sha"] = json!("abc123")
            }),
        ),
        (
            "a mistyped commit (run)",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                v["check_run"]["head_sha"] = json!(7)
            }),
        ),
        (
            "no check run id, which the key is made of",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                remove(v, "/check_run/id")
            }),
        ),
        (
            "a check run id that is no number",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                v["check_run"]["id"] = json!("128620228")
            }),
        ),
        (
            "no completion time, which the key and the age are made of",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                remove(v, "/check_run/completed_at")
            }),
        ),
        (
            "a completion time that is no time",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                v["check_run"]["completed_at"] = json!("yesterday")
            }),
        ),
        (
            "no workflow_run",
            "workflow_run",
            edit("workflow_run.completed.success", &|v| {
                remove(v, "/workflow_run")
            }),
        ),
        (
            "no commit (workflow)",
            "workflow_run",
            edit("workflow_run.completed.success", &|v| {
                remove(v, "/workflow_run/head_sha")
            }),
        ),
        (
            "no workflow run id",
            "workflow_run",
            edit("workflow_run.completed.success", &|v| {
                remove(v, "/workflow_run/id")
            }),
        ),
        (
            "no update time",
            "workflow_run",
            edit("workflow_run.completed.success", &|v| {
                remove(v, "/workflow_run/updated_at")
            }),
        ),
        (
            "a run attempt that is no number",
            "workflow_run",
            edit("workflow_run.completed.success", &|v| {
                v["workflow_run"]["run_attempt"] = json!("one")
            }),
        ),
        (
            "a check_run event that carries a workflow_run body",
            "check_run",
            fixture("workflow_run.completed.success"),
        ),
        (
            "a workflow_run event that carries a check_run body",
            "workflow_run",
            fixture("check_run.completed.failure"),
        ),
    ];
    for (why, event, body) in cases {
        let res = delivery(&rig, SECRET, event, DELIVERY, &body)
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
    // No event header.
    let body = fixture("check_run.completed.failure");
    let res = rig
        .client
        .post(format!("{}/webhooks/github", rig.base))
        .header("X-Hub-Signature-256", sign_github(SECRET, &body).unwrap())
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400, "no X-GitHub-Event");
    assert!(!rig.holds_a_row().await, "not one of them wrote a row");
    // The delivery id is only for the log: without it the same event is stored.
    let res = rig
        .client
        .post(format!("{}/webhooks/github", rig.base))
        .header("X-GitHub-Event", "check_run")
        .header("X-Hub-Signature-256", sign_github(SECRET, &body).unwrap())
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 202, "no X-GitHub-Delivery");
    assert_eq!(rig.rows().await, 1);
}

#[tokio::test]
async fn text_from_outside_is_kept_as_text_and_only_http_links_survive() {
    let rig = start(SECRET).await;
    let mut n = 0;
    let mut stored = |v: Value| {
        n += 1;
        let mut v = v;
        v["check_run"]["id"] = json!(9000 + n);
        v.to_string().into_bytes()
    };
    let edit = |f: &dyn Fn(&mut Value)| {
        let mut v: Value = serde_json::from_slice(&fixture("check_run.completed.failure")).unwrap();
        f(&mut v);
        v
    };
    for (link, kept) in [
        ("https://github.com/Acme/Widgets/runs/1", true),
        ("javascript:alert(1)", false),
        ("data:text/html,x", false),
        ("not a url", false),
    ] {
        let body = stored(edit(&|v| v["check_run"]["html_url"] = json!(link)));
        assert_eq!(send(&rig, "check_run", &body).await, 202);
        let payload = row(&rig, "check_run", &body).await.unwrap().payload;
        assert_eq!(payload.get("url").is_some(), kept, "{link}");
    }
    // A summary of two-byte characters is cut on a boundary at 16 KiB; a blank one is absent.
    let body = stored(edit(&|v| {
        v["check_run"]["output"]["summary"] = json!("é".repeat(20_000))
    }));
    assert_eq!(send(&rig, "check_run", &body).await, 202);
    let summary = row(&rig, "check_run", &body).await.unwrap().payload["summary"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(summary.len(), github::MAX_SUMMARY_BYTES);
    let body = stored(edit(&|v| {
        v["check_run"]["output"]["summary"] = json!("  \n")
    }));
    assert_eq!(send(&rig, "check_run", &body).await, 202);
    assert!(
        row(&rig, "check_run", &body)
            .await
            .unwrap()
            .payload
            .get("summary")
            .is_none()
    );
    // A name over the limit is cut, which makes it match nothing required.
    let body = stored(edit(&|v| {
        v["check_run"]["name"] = json!("n".repeat(github::MAX_NAME_BYTES + 50))
    }));
    assert_eq!(send(&rig, "check_run", &body).await, 202);
    assert_eq!(
        row(&rig, "check_run", &body).await.unwrap().payload["name"]
            .as_str()
            .unwrap()
            .len(),
        github::MAX_NAME_BYTES
    );
}

#[tokio::test]
async fn either_secret_of_a_rotation_is_accepted() {
    let new = "new-secret-new-secret-new-secret-new";
    let old = "old-secret-old-secret-old-secret-old";
    let rig = start(&format!("{new},{old}")).await;
    let body = fixture("check_run.completed.failure");
    for secret in [new, old] {
        let res = delivery(&rig, secret, "check_run", DELIVERY, &body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 202, "{secret}");
    }
    let res = delivery(
        &rig,
        "retired-retired-retired-retired-retired",
        "check_run",
        DELIVERY,
        &body,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn the_route_needs_no_identity_and_the_rest_of_the_api_still_does() {
    let rig = start(SECRET).await;
    let body = fixture("workflow_run.completed.success");
    assert_eq!(send(&rig, "workflow_run", &body).await, 202);
    // An identity header is not read, and does not stand for a signature.
    let res = delivery(
        &rig,
        SECRET,
        "workflow_run",
        "0a1b2c3d-0000-0000-0000-000000000003",
        &body,
    )
    .header("X-Auth-Request-Email", "mallory@example.com")
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202);
    let res = rig
        .client
        .post(format!("{}/webhooks/github", rig.base))
        .header("X-Auth-Request-Email", "alice@example.com")
        .header("X-GitHub-Event", "workflow_run")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let res = rig
        .client
        .get(format!("{}/api/agents", rig.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let res = rig
        .client
        .get(format!("{}/healthz", rig.base))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
}
