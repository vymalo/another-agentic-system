//! The UI catalog on a run (ADR 0023): `forwardedProps["vymalo.uiCatalog"]` is read on every run,
//! refused before the stream when it breaks a rule (nothing written, nothing sent), and applied
//! only when the run applies an input. The real router on a real port over the in-memory stack.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::catalog_digest;
use orch_ports::memory::Call;
use serde_json::{Value, json};
use support::*;

const ID: &str = "https://agents.vymalo.com/a2ui/catalogs/chat";

/// A component schema that names itself, as the web's catalog does.
fn note(tag: &str) -> Value {
    json!({
        "type": "object",
        "title": tag,
        "properties": {"id": {"type": "string"}, "component": {"const": "Note"}, "text": {"type": "string"}},
        "required": ["id", "component"],
        "additionalProperties": false,
    })
}

/// The object the web sends for a catalog of `components`, with its real digest (a dummy one when
/// the catalog cannot be canonicalised: the rule that refuses it comes first).
fn sent(version: u64, components: &Value) -> Value {
    let catalog = json!({"catalogId": ID, "components": components});
    let digest = catalog_digest(&catalog).unwrap_or_else(|_| format!("sha256:{}", "0".repeat(64)));
    json!({"catalogId": ID, "version": version, "digest": digest, "catalog": catalog})
}

/// Version `version` of a catalog with one component, which differs from another version.
fn version(version: u64) -> Value {
    sent(version, &json!({"Note": note(&format!("v{version}"))}))
}

fn with_catalog(thread: &str, run: &str, message: &str, catalog: &Value) -> Value {
    input_with(
        thread,
        run,
        &[(&format!("m-{run}"), message)],
        json!({"forwardedProps": {"vymalo.uiCatalog": catalog}}),
    )
}

fn kinds_of(events: &[Value]) -> Vec<&str> {
    events.iter().map(|e| e["kind"].as_str().unwrap()).collect()
}

/// The `thread.uiCatalog` of every `STATE_SNAPSHOT` of a response (none: `Null`).
fn snapshot_catalogs(frames: &[Frame]) -> Vec<Value> {
    frames
        .iter()
        .filter(|f| f.kind() == "STATE_SNAPSHOT")
        .map(|f| f.event["snapshot"]["thread"]["uiCatalog"].clone())
        .collect()
}

#[tokio::test]
async fn a_first_run_with_a_catalog_records_it_first_and_says_which_in_every_snapshot() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let v1 = version(1);
    let frames = h
        .run(
            "plain",
            ALICE,
            &with_catalog(&thread, "run-1", "echo hi", &v1),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    // no frame of its own, and the run opens with the message
    assert_eq!(frames[0].kind(), "RUN_STARTED");
    let reference = json!({"catalogId": ID, "version": 1, "digest": v1["digest"]});
    let catalogs = snapshot_catalogs(&frames);
    assert!(!catalogs.is_empty());
    assert!(catalogs.iter().all(|c| *c == reference), "{catalogs:?}");

    let events = h.events(ALICE, &thread).await;
    assert_eq!(kinds_of(&events)[..2], ["ui_catalog", "user_message"]);
    assert_eq!(events[0]["actor"], json!({"type": "user", "name": ALICE}));
    assert_eq!(events[0]["data"], v1, "stored as the web sent it");

    // the agent was told: the catalog inline, and the thread it is about
    let sends = h.agent.sends();
    let Call::Send {
        ui_catalog,
        thread: named,
        ..
    } = &sends[0]
    else {
        panic!("{sends:?}");
    };
    assert_eq!(
        serde_json::to_value(ui_catalog).unwrap()["inline"]["digest"],
        v1["digest"]
    );
    assert_eq!(named.map(|t| t.to_string()), Some(thread));
}

#[tokio::test]
async fn a_thread_without_a_catalog_says_nothing_of_one() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let frames = h
        .run(
            "plain",
            ALICE,
            &input(&thread, "run-1", &[("m-1", "echo hi")]),
        )
        .await
        .all()
        .await;
    assert!(snapshot_catalogs(&frames).iter().all(Value::is_null));
    assert!(!kinds_of(&h.events(ALICE, &thread).await).contains(&"ui_catalog"));
    // `null` is no catalog
    let other = new_thread_id();
    let body = input_with(
        &other,
        "run-1",
        &[("m-1", "echo hi")],
        json!({"forwardedProps": {"vymalo.uiCatalog": null}}),
    );
    let frames = h.run("plain", ALICE, &body).await.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    assert!(!kinds_of(&h.events(ALICE, &other).await).contains(&"ui_catalog"));
}

#[tokio::test]
async fn the_same_request_again_attaches_and_records_nothing_twice() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let body = with_catalog(&thread, "run-1", "echo hi", &version(1));
    h.run("plain", ALICE, &body).await.all().await;
    h.wait_state(ALICE, &thread, "done").await;
    let before = h.events(ALICE, &thread).await;

    // a retry of the same POST: an attach, the same frames, nothing new in the log
    let again = h.run("plain", ALICE, &body).await.all().await;
    assert_eq!(again[0].kind(), "RUN_STARTED");
    assert_eq!(h.events(ALICE, &thread).await, before);
    assert_eq!(h.agent.sends().len(), 1);

    // an attach ignores the catalog it carries, even a newer one
    let newer = with_catalog(&thread, "run-1", "echo hi", &version(2));
    h.run("plain", ALICE, &newer).await.all().await;
    assert_eq!(h.events(ALICE, &thread).await, before);
}

#[tokio::test]
async fn a_newer_catalog_on_a_later_run_is_recorded_and_an_older_one_never_becomes_current() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let (v1, v2) = (version(1), version(2));
    h.run(
        "plain",
        ALICE,
        &with_catalog(&thread, "run-1", "echo one", &v1),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;

    // version 2: a second event, first in its commit, and the next job's snapshot names it
    let frames = h
        .run(
            "plain",
            ALICE,
            &with_catalog(&thread, "run-2", "echo two", &v2),
        )
        .await
        .all()
        .await;
    h.wait_state(ALICE, &thread, "done").await;
    let reference_2 = json!({"catalogId": ID, "version": 2, "digest": v2["digest"]});
    assert!(
        snapshot_catalogs(&frames).iter().all(|c| *c == reference_2),
        "{:?}",
        snapshot_catalogs(&frames)
    );
    let events = h.events(ALICE, &thread).await;
    assert_eq!(
        kinds_of(&events)
            .iter()
            .filter(|k| **k == "ui_catalog")
            .count(),
        2
    );
    let at = events
        .iter()
        .rposition(|e| e["kind"] == "ui_catalog")
        .unwrap();
    assert_eq!(events[at + 1]["kind"], "user_message");
    assert_eq!(events[at]["data"]["version"], 2);

    // the older screen comes back: its digest is known, so nothing is written, and the thread
    // keeps the newest
    let frames = h
        .run(
            "plain",
            ALICE,
            &with_catalog(&thread, "run-3", "echo three", &v1),
        )
        .await
        .all()
        .await;
    h.wait_state(ALICE, &thread, "done").await;
    assert!(snapshot_catalogs(&frames).iter().all(|c| *c == reference_2));
    let events = h.events(ALICE, &thread).await;
    assert_eq!(
        kinds_of(&events)
            .iter()
            .filter(|k| **k == "ui_catalog")
            .count(),
        2
    );

    // what each message told the agent: inline, inline, then a reference to the newest
    let told: Vec<Value> = h
        .agent
        .sends()
        .into_iter()
        .map(|call| match call {
            Call::Send { ui_catalog, .. } => serde_json::to_value(ui_catalog).unwrap(),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(told[0]["inline"]["version"], 1);
    assert_eq!(told[1]["inline"]["version"], 2);
    assert_eq!(
        told[2],
        json!({"ref": {"catalogId": ID, "version": 2, "digest": v2["digest"]}})
    );
}

#[tokio::test]
async fn an_answer_to_a_question_can_carry_a_newer_catalog() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let (v1, v2) = (version(1), version(2));
    let mut first = h
        .run(
            "plain",
            ALICE,
            &with_catalog(&thread, "run-1", "ask which branch", &v1),
        )
        .await;
    let frames = first.through_run().await;
    let interrupt = frames.last().unwrap().event["outcome"]["interrupts"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.wait_state(ALICE, &thread, "blocked").await;

    let answer = input_with(
        &thread,
        "run-2",
        &[],
        json!({
            "resume": [{"interruptId": interrupt, "status": "resolved", "payload": {"text": "main"}}],
            "forwardedProps": {"vymalo.uiCatalog": v2},
        }),
    );
    h.run("plain", ALICE, &answer).await.all().await;
    h.wait_state(ALICE, &thread, "done").await;
    let events = h.events(ALICE, &thread).await;
    let catalog_seqs: Vec<(i64, i64)> = events
        .iter()
        .filter(|e| e["kind"] == "ui_catalog")
        .map(|e| {
            (
                e["seq"].as_i64().unwrap(),
                e["data"]["version"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(catalog_seqs.len(), 2);
    // the newer catalog sits right before the answer it came with
    let answered = events
        .iter()
        .position(|e| e["kind"] == "user_message" && e["data"]["text"] == "main")
        .unwrap();
    assert_eq!(events[answered - 1]["kind"], "ui_catalog");
    assert_eq!(events[answered - 1]["data"]["version"], 2);
}

/// Every refusal: the status, nothing streamed, and nothing created or sent.
async fn refused(h: &Harness, catalog: &Value, status: u16) -> Value {
    let thread = new_thread_id();
    let body = with_catalog(&thread, "run-1", "echo hi", catalog);
    let problem = h.refused("plain", Some(ALICE), &body).await.problem(status);
    let r = h.get(&format!("/api/threads/{thread}"), Some(ALICE)).await;
    assert_eq!(r.status, 404, "the refused request created a thread");
    assert!(h.agent.sends().is_empty(), "nothing was sent to the agent");
    problem
}

#[tokio::test]
async fn a_catalog_that_breaks_a_rule_is_a_400_with_the_rule_and_nothing_is_written() {
    let h = Harness::start().await;
    let ok = || sent(1, &json!({"Note": note("a")}));
    let mut cases: Vec<(&str, Value, &str)> = Vec::new();

    // the envelope
    cases.push(("a string", json!("catalog"), "object"));
    cases.push(("an array", json!([]), "object"));
    cases.push(("an empty object", json!({}), "missing field"));
    let mut missing = ok();
    missing.as_object_mut().unwrap().remove("digest");
    cases.push(("no digest", missing, "digest"));
    let mut extra = ok();
    extra["extra"] = json!(1);
    cases.push(("an unknown member", extra, "extra"));
    for (why, id) in [
        ("an http id", "http://agents.vymalo.com/c"),
        ("a relative id", "a2ui/catalogs/chat"),
        ("an id with a space", "https://agents.vymalo.com/a b"),
    ] {
        let mut v = ok();
        v["catalogId"] = json!(id);
        cases.push((why, v, "catalogId"));
    }
    for bad in [
        json!(0),
        json!(1_000_001),
        json!(-3),
        json!("1"),
        json!(1.5),
    ] {
        let mut v = ok();
        v["version"] = bad;
        cases.push(("a version out of range", v, "version"));
    }
    for bad in ["sha256:", "sha256:ABC", "md5:0123", "0".repeat(64).as_str()] {
        let mut v = ok();
        v["digest"] = json!(bad);
        cases.push(("a malformed digest", v, "digest"));
    }
    let mut wrong = ok();
    wrong["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
    cases.push((
        "a digest that is not the catalog's",
        wrong,
        "does not match",
    ));
    let mut other_id = ok();
    other_id["catalog"]["catalogId"] = json!("https://agents.vymalo.com/a2ui/catalogs/other");
    cases.push(("another id inside", other_id, "catalogId"));
    let mut not_object = ok();
    not_object["catalog"] = json!("nope");
    cases.push((
        "a catalog that is not an object",
        not_object,
        "catalog must be an object",
    ));

    // what a catalog may hold
    for member in ["functions", "theme"] {
        let mut v = json!({"catalogId": ID, "components": {"Note": note("a")}});
        v[member] = json!([]);
        let envelope = json!({"catalogId": ID, "version": 1,
            "digest": catalog_digest(&v).unwrap(), "catalog": v});
        cases.push((
            "a member of the inline catalog we do not accept",
            envelope,
            member,
        ));
    }
    cases.push(("no components", sent(1, &json!({})), "components"));
    let many: serde_json::Map<String, Value> =
        (0..65).map(|i| (format!("C{i}"), note("many"))).collect();
    cases.push((
        "65 components",
        sent(1, &Value::Object(many)),
        "65 components",
    ));
    cases.push((
        "a lowercase name",
        sent(1, &json!({"note": note("a")})),
        "component name",
    ));
    for keyword in ["$ref", "$id", "$anchor", "$schema", "$dynamicRef"] {
        let mut schema = note("a");
        schema["properties"]["text"] = json!({ keyword: "x" });
        cases.push((
            "a reference or an identity",
            sent(1, &json!({"Note": schema})),
            keyword,
        ));
    }
    let mut fractional = note("a");
    fractional["properties"]["text"] = json!({"type": "number", "multipleOf": 0.5});
    cases.push((
        "a fractional number",
        sent(1, &json!({"Note": fractional})),
        "integers",
    ));
    let mut accented = note("a");
    accented["properties"]["clé"] = json!({"type": "string"});
    cases.push((
        "a key that is not ASCII",
        sent(1, &json!({"Note": accented})),
        "ASCII",
    ));
    let mut deep = json!(1);
    for _ in 0..40 {
        deep = json!({"a": deep});
    }
    let mut nested = note("a");
    nested["properties"]["text"] = deep;
    cases.push((
        "nesting past 32 levels",
        sent(1, &json!({"Note": nested})),
        "deeper",
    ));

    // the schemas
    let mut not_schema = note("a");
    not_schema["type"] = json!("nonsense");
    cases.push((
        "a schema that is not JSON Schema",
        sent(1, &json!({"Note": not_schema})),
        "JSON Schema",
    ));
    cases.push((
        "a schema that does not name its component",
        sent(
            1,
            &json!({"Note": {"type": "object", "properties": {"component": {"const": "Other"}}}}),
        ),
        "properties.component.const",
    ));
    cases.push((
        "a schema with no component member",
        sent(1, &json!({"Note": {"type": "object"}})),
        "properties.component.const",
    ));

    for (why, catalog, rule) in &cases {
        let problem = refused(&h, catalog, 400).await;
        let detail = problem["detail"].as_str().unwrap();
        assert!(detail.contains("vymalo.uiCatalog"), "{why}: {detail}");
        assert!(
            detail.contains(rule),
            "{why}: the detail does not say '{rule}': {detail}"
        );
    }
}

#[tokio::test]
async fn a_catalog_over_64_kib_is_a_413() {
    let h = Harness::start().await;
    let mut schema = note("big");
    schema["description"] = json!("x".repeat(70_000));
    let big = sent(1, &json!({"Note": schema}));
    let problem = refused(&h, &big, 413).await;
    assert!(
        problem["detail"].as_str().unwrap().contains("65536"),
        "{problem}"
    );
}

#[tokio::test]
async fn a_bad_catalog_is_refused_on_a_run_that_continues_a_thread_too_and_changes_nothing() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    h.run(
        "plain",
        ALICE,
        &with_catalog(&thread, "run-1", "echo one", &version(1)),
    )
    .await
    .all()
    .await;
    h.wait_state(ALICE, &thread, "done").await;
    let before = h.events(ALICE, &thread).await;
    let sends = h.agent.sends().len();

    let mut forged = version(2);
    forged["digest"] = json!(format!("sha256:{}", "f".repeat(64)));
    let body = with_catalog(&thread, "run-2", "echo two", &forged);
    h.refused("plain", Some(ALICE), &body).await.problem(400);
    // even a retry of a run the log holds (an attach) is read: a malformed member is refused
    let retry = with_catalog(&thread, "run-1", "echo one", &forged);
    h.refused("plain", Some(ALICE), &retry).await.problem(400);

    assert_eq!(h.events(ALICE, &thread).await, before);
    assert_eq!(h.agent.sends().len(), sends);
    assert_eq!(h.state(ALICE, &thread).await, "done");
}
