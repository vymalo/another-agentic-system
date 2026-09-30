//! `POST /webhooks/github` over real HTTP, on the in-memory store, with the fixtures of
//! `testdata/github` (synthetic, shaped after GitHub's documented schemas; see the README there):
//! the three completed events become the report they should, every other event or action is
//! acknowledged and not stored, `ping` is 204, the guard's refusals write nothing, and the route
//! never reads `X-Auth-Request-Email`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::path::PathBuf;

use orch_ports::{InboxItem, InboxStatus};
use orch_surface_webhook::signature::{sign_generic, sign_github};
use orch_surface_webhook::{GithubConfig, Secrets, github};
use serde_json::{Value, json};
use support::Rig;

const SECRET: &str = "dev-webhook-secret";
const DELIVERY: &str = "72d3162e-cc78-11e3-81ab-4c9367dc0958";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

async fn start(secrets: &str) -> Rig {
    let cfg = GithubConfig::new(Secrets::parse(secrets).unwrap());
    Rig::start(|app| github::routes(app, cfg)).await
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

async fn row(rig: &Rig, id: &str) -> Option<InboxItem> {
    rig.find(github::SOURCE, &format!("github:{id}")).await
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
        file: "check_suite.completed.success",
        event: "check_suite",
        name: "github-actions",
        conclusion: "success",
        branch: Some("agent/fix-flaky-test"),
        url: None,
        summary: None,
    },
    Accepted {
        file: "check_suite.completed.failure_no_branch",
        event: "check_suite",
        name: "github-actions",
        conclusion: "failure",
        branch: None,
        url: None,
        summary: None,
    },
    Accepted {
        file: "check_suite.completed.unknown_conclusion",
        event: "check_suite",
        name: "github-actions",
        conclusion: "failure",
        branch: Some("agent/fix-flaky-test"),
        url: None,
        summary: None,
    },
    Accepted {
        file: "check_suite.completed.null_conclusion",
        event: "check_suite",
        name: "github-actions",
        conclusion: "failure",
        branch: Some("agent/fix-flaky-test"),
        url: None,
        summary: None,
    },
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
    Accepted {
        file: "workflow_run.completed.unnamed",
        event: "workflow_run",
        name: "workflow",
        conclusion: "cancelled",
        branch: Some("agent/fix-flaky-test"),
        url: Some("https://github.com/Acme/Widgets/actions/runs/31415926"),
        summary: None,
    },
];

/// Acknowledged, not stored: the same events with another action, and another event.
const IGNORED: &[(&str, &str)] = &[
    ("check_suite.requested", "check_suite"),
    ("check_run.created", "check_run"),
    ("check_run.rerequested", "check_run"),
    ("workflow_run.requested", "workflow_run"),
    ("push", "push"),
];

#[tokio::test]
async fn the_three_completed_events_become_the_report_they_should() {
    for case in ACCEPTED {
        let rig = start(SECRET).await;
        let status = send(&rig, case.event, &fixture(case.file)).await;
        assert_eq!(status, 202, "{}", case.file);
        let stored = row(&rig, DELIVERY)
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
    for event in ["issues", "star", "deployment_status", "check_suite_unknown"] {
        assert_eq!(send(&rig, event, b"not even json").await, 202, "{event}");
    }
    assert!(!rig.holds_a_row().await);
}

#[tokio::test]
async fn an_unsigned_delivery_is_never_acknowledged_whatever_the_event() {
    let rig = start(SECRET).await;
    for event in ["push", "ping", "check_suite", "issues"] {
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
    let first = row(&rig, DELIVERY).await.unwrap();
    assert_eq!(send(&rig, "check_run", &body).await, 202);
    // The same delivery id with another body is the same delivery: the first is kept.
    assert_eq!(
        send(&rig, "check_run", &fixture("check_run.completed.timed_out")).await,
        202
    );
    assert_eq!(row(&rig, DELIVERY).await.unwrap().id, first.id);
    assert_eq!(row(&rig, DELIVERY).await.unwrap().payload, first.payload);
    // A new delivery id is a new report.
    let res = delivery(
        &rig,
        SECRET,
        "check_run",
        "0a1b2c3d-0000-0000-0000-000000000002",
        &body,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(res.status(), 202);
    assert_eq!(rig.rows().await, 2);
}

#[tokio::test]
async fn a_refused_delivery_writes_nothing() {
    let rig = start(SECRET).await;
    let body = fixture("check_suite.completed.success");
    let url = format!("{}/webhooks/github", rig.base);
    let post = |signature: Option<String>| {
        let mut req = rig
            .client
            .post(&url)
            .header("X-GitHub-Event", "check_suite")
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
        ("not JSON", "check_suite", b"{".to_vec()),
        ("not an object", "check_run", b"[]".to_vec()),
        (
            "no action",
            "check_suite",
            edit("check_suite.completed.success", &|v| remove(v, "/action")),
        ),
        (
            "no repository",
            "check_suite",
            edit("check_suite.completed.success", &|v| {
                remove(v, "/repository/html_url")
            }),
        ),
        (
            "a repository that is no address",
            "check_suite",
            edit("check_suite.completed.success", &|v| {
                v["repository"]["html_url"] = json!("nonsense")
            }),
        ),
        (
            "no commit (suite)",
            "check_suite",
            edit("check_suite.completed.success", &|v| {
                remove(v, "/check_suite/head_sha")
            }),
        ),
        (
            "a short commit",
            "check_suite",
            edit("check_suite.completed.success", &|v| {
                v["check_suite"]["head_sha"] = json!("abc123")
            }),
        ),
        (
            "no app slug",
            "check_suite",
            edit("check_suite.completed.success", &|v| {
                remove(v, "/check_suite/app/slug")
            }),
        ),
        (
            "a blank app slug",
            "check_suite",
            edit("check_suite.completed.success", &|v| {
                v["check_suite"]["app"]["slug"] = json!(" ")
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
            "a mistyped commit (run)",
            "check_run",
            edit("check_run.completed.failure", &|v| {
                v["check_run"]["head_sha"] = json!(7)
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
    // No event header, and no delivery id for an event that is stored.
    let body = fixture("check_suite.completed.success");
    let res = rig
        .client
        .post(format!("{}/webhooks/github", rig.base))
        .header("X-Hub-Signature-256", sign_github(SECRET, &body).unwrap())
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400, "no X-GitHub-Event");
    let res = rig
        .client
        .post(format!("{}/webhooks/github", rig.base))
        .header("X-GitHub-Event", "check_suite")
        .header("X-Hub-Signature-256", sign_github(SECRET, &body).unwrap())
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400, "no X-GitHub-Delivery");
    assert!(!rig.holds_a_row().await, "not one of them wrote a row");
}

#[tokio::test]
async fn text_from_outside_is_kept_as_text_and_only_http_links_survive() {
    let rig = start(SECRET).await;
    let mut n = 0;
    let mut stored = |v: Value| {
        n += 1;
        (
            format!("c0ffee00-0000-0000-0000-{n:012}"),
            v.to_string().into_bytes(),
        )
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
        let (id, body) = stored(edit(&|v| v["check_run"]["html_url"] = json!(link)));
        let res = delivery(&rig, SECRET, "check_run", &id, &body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 202);
        let payload = row(&rig, &id).await.unwrap().payload;
        assert_eq!(payload.get("url").is_some(), kept, "{link}");
    }
    // A summary of two-byte characters is cut on a boundary at 16 KiB; a blank one is absent.
    let (id, body) = stored(edit(&|v| {
        v["check_run"]["output"]["summary"] = json!("é".repeat(20_000))
    }));
    assert_eq!(
        delivery(&rig, SECRET, "check_run", &id, &body)
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    let summary = row(&rig, &id).await.unwrap().payload["summary"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(summary.len(), github::MAX_SUMMARY_BYTES);
    let (id, body) = stored(edit(&|v| {
        v["check_run"]["output"]["summary"] = json!("  \n")
    }));
    assert_eq!(
        delivery(&rig, SECRET, "check_run", &id, &body)
            .send()
            .await
            .unwrap()
            .status(),
        202
    );
    assert!(
        row(&rig, &id)
            .await
            .unwrap()
            .payload
            .get("summary")
            .is_none()
    );
}

#[tokio::test]
async fn either_secret_of_a_rotation_is_accepted() {
    let rig = start("new-secret,old-secret").await;
    let body = fixture("check_suite.completed.success");
    for (n, secret) in ["new-secret", "old-secret"].into_iter().enumerate() {
        let id = format!("0a1b2c3d-0000-0000-0000-00000000000{n}");
        let res = delivery(&rig, secret, "check_suite", &id, &body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 202, "{secret}");
    }
    let res = delivery(
        &rig,
        "retired",
        "check_suite",
        "0a1b2c3d-0000-0000-0000-000000000009",
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
