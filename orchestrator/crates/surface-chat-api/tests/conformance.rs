//! Contract conformance: drives every operation of `docs/api/chat-api.yaml` over real HTTP
//! against the in-memory stack and validates every response body against the spec's schemas.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::*;

const RANDOM: &str = "0190aaaa-0000-7000-8000-000000000123";

#[tokio::test]
async fn every_operation_conforms_to_the_contract() {
    let h = Harness::start().await;
    let mut c = Conformance::new();

    // getHealth / getReady
    for (op, path) in [("getHealth", "/healthz"), ("getReady", "/readyz")] {
        let r = h.get(path, None).await;
        assert_eq!(r.status, 200);
        c.check(op, &r);
    }

    // 401 on every operation that requires identity.
    let auth_ops: [(&str, reqwest::Method, String); 8] = [
        ("listAgents", reqwest::Method::GET, "/api/agents".into()),
        ("listThreads", reqwest::Method::GET, "/api/threads".into()),
        ("createThread", reqwest::Method::POST, "/api/threads".into()),
        (
            "getThread",
            reqwest::Method::GET,
            format!("/api/threads/{RANDOM}"),
        ),
        (
            "listEvents",
            reqwest::Method::GET,
            format!("/api/threads/{RANDOM}/events"),
        ),
        (
            "streamEvents",
            reqwest::Method::GET,
            format!("/api/threads/{RANDOM}/stream"),
        ),
        (
            "postMessage",
            reqwest::Method::POST,
            format!("/api/threads/{RANDOM}/messages"),
        ),
        (
            "cancelThread",
            reqwest::Method::POST,
            format!("/api/threads/{RANDOM}/cancel"),
        ),
    ];
    for (op, method, path) in auth_ops {
        let r = h.send(method, &path, None, Some(json!({}))).await;
        assert_eq!(r.status, 401, "{op}");
        c.check(op, &r);
    }

    // listAgents
    let r = h.get("/api/agents", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    c.check("listAgents", &r);
    let agents = r.json();
    let coder = agents
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "coder")
        .unwrap();
    assert_eq!(coder["releases"]["defaultChannel"], "stable");
    assert_eq!(coder["releases"]["channels"]["staging"], "rev-2");
    let plain = agents
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "plain")
        .unwrap();
    assert!(plain.get("releases").is_none());
    // `cardUrl` is optional in the contract: an in-process agent has none, and the key is absent
    // (not null); every A2A agent keeps it.
    let helper = agents
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "helper")
        .unwrap();
    assert!(helper.get("cardUrl").is_none(), "{helper}");
    assert_eq!(helper["name"], "Helper");
    assert_eq!(
        plain["cardUrl"],
        "https://plain.example.com/.well-known/agent-card.json"
    );

    // createThread: 201 and the 400s
    let r = h
        .post(
            "/api/threads",
            Some(ALICE),
            json!({"title": "Contract", "target": {"agentId": "coder", "release": "staging"}, "text": "echo contract"}),
        )
        .await;
    assert_eq!(r.status, 201);
    c.check("createThread", &r);
    let created = r.json();
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(created["title"], "Contract");
    assert_eq!(
        created["target"],
        json!({"agentId": "coder", "release": "staging"})
    );
    assert_eq!(created["lastSeq"], 1);
    for bad in [
        json!({"target": {"agentId": "coder"}, "text": ""}),
        json!({"target": {"agentId": "nope"}, "text": "hi"}),
        json!({"target": {"agentId": "plain", "release": "stable"}, "text": "hi"}),
        json!({"target": {"agentId": "coder", "release": "nightly"}, "text": "hi"}),
        json!({"target": {"agentId": "plain"}}),
        json!({"text": "no target"}),
        json!({"target": {"agentId": "plain"}, "text": "x", "title": "t".repeat(201)}),
        json!({"target": {"agentId": "plain"}, "text": "x".repeat(100_001)}),
    ] {
        let r = h.post("/api/threads", Some(ALICE), bad.clone()).await;
        assert_eq!(r.status, 400, "{bad}");
        c.check("createThread", &r);
    }
    // 502: the release cannot be validated because the agent's card is down. 503: storage
    // fails. Both are documented on createThread.
    h.agent.set_card_down("coder", true);
    let r = h
        .post(
            "/api/threads",
            Some(ALICE),
            json!({"target": {"agentId": "coder", "release": "staging"}, "text": "x"}),
        )
        .await;
    assert_eq!(r.status, 502);
    c.check("createThread", &r);
    h.agent.set_card_down("coder", false);
    h.store.fail_next_creates(1, || {
        orch_ports::StoreError::unavailable(std::io::Error::other("pool timed out"))
    });
    let r = h
        .post(
            "/api/threads",
            Some(ALICE),
            json!({"target": {"agentId": "plain"}, "text": "x"}),
        )
        .await;
    assert_eq!(r.status, 503);
    c.check("createThread", &r);
    assert_eq!(r.json()["detail"], "storage is unavailable");

    // Malformed JSON and a wrong content type are 400 problems as well.
    let r = h
        .client
        .post(h.url("/api/threads"))
        .header("X-Auth-Request-Email", ALICE)
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);
    let r = h
        .client
        .post(h.url("/api/threads"))
        .header("X-Auth-Request-Email", ALICE)
        .header("content-type", "text/plain")
        .body("hello")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 400);

    // getThread
    h.wait_state(ALICE, &id, "done").await;
    let r = h.get(&format!("/api/threads/{id}"), Some(ALICE)).await;
    assert_eq!(r.status, 200);
    c.check("getThread", &r);
    assert_eq!(r.json()["state"], "done");
    for path in [
        format!("/api/threads/{RANDOM}"),
        "/api/threads/not-a-uuid".to_owned(),
    ] {
        let r = h.get(&path, Some(ALICE)).await;
        assert_eq!(r.status, 404);
        c.check("getThread", &r);
    }

    // listThreads
    for i in 0..3 {
        h.create(ALICE, "plain", &format!("echo list {i}")).await;
    }
    let r = h.get("/api/threads", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    c.check("listThreads", &r);
    let all = r.json();
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 4);
    let ids: Vec<&str> = all.iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids[3], id, "newest first: the first thread is last");
    let r = h.get("/api/threads?limit=2", Some(ALICE)).await;
    c.check("listThreads", &r);
    assert_eq!(r.json().as_array().unwrap().len(), 2);
    let r = h
        .get(
            &format!("/api/threads?limit=2&before={}", ids[1]),
            Some(ALICE),
        )
        .await;
    c.check("listThreads", &r);
    let page: Vec<String> = r
        .json()
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(page, [ids[2], ids[3]]);
    for bad in ["limit=0", "limit=101", "limit=abc", "before=not-a-uuid"] {
        let r = h.get(&format!("/api/threads?{bad}"), Some(ALICE)).await;
        assert_eq!(r.status, 400, "{bad}");
        c.check("listThreads", &r);
    }
    assert!(
        h.get("/api/threads", Some(BOB))
            .await
            .json()
            .as_array()
            .unwrap()
            .is_empty()
    );

    // listEvents
    let r = h
        .get(&format!("/api/threads/{id}/events"), Some(ALICE))
        .await;
    assert_eq!(r.status, 200);
    c.check("listEvents", &r);
    let events = r.json();
    assert_eq!(
        shape(events.as_array().unwrap()),
        [
            "user_message",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    let seqs: Vec<i64> = events
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["seq"].as_i64().unwrap())
        .collect();
    assert_eq!(seqs, [1, 2, 3, 4, 5]);
    // The release echo: agent events name the revision that served them.
    assert_eq!(
        events[1]["actor"],
        json!({"type": "agent", "name": "coder", "revision": "rev-2"})
    );
    assert_eq!(events[0]["actor"], json!({"type": "user", "name": ALICE}));
    assert_eq!(
        events[4]["actor"],
        json!({"type": "system", "name": "orchestrator"})
    );
    let r = h
        .get(
            &format!("/api/threads/{id}/events?after=3&limit=1"),
            Some(ALICE),
        )
        .await;
    c.check("listEvents", &r);
    assert_eq!(r.json().as_array().unwrap().len(), 1);
    assert_eq!(r.json()[0]["seq"], 4);
    let r = h
        .get(&format!("/api/threads/{RANDOM}/events"), Some(ALICE))
        .await;
    assert_eq!(r.status, 404);
    c.check("listEvents", &r);
    for bad in ["after=-1", "limit=0", "limit=501", "after=x"] {
        let r = h
            .get(&format!("/api/threads/{id}/events?{bad}"), Some(ALICE))
            .await;
        assert_eq!(r.status, 400, "{bad}");
        c.check("listEvents", &r);
    }

    // streamEvents: frames are `id: seq`, `event: kind`, `data: Event`.
    let mut sse = h.stream(ALICE, &id, None).await;
    assert_eq!(sse.status.as_u16(), 200);
    assert!(
        sse.headers["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let mut streamed = Vec::new();
    for _ in 0..5 {
        let (seq, kind, data) = sse.next_event(Duration::from_secs(5)).await.expect("event");
        assert_eq!(data["seq"], seq);
        assert_eq!(data["kind"], kind.as_str());
        c.contract.validate_component("Event", &data);
        streamed.push(data);
    }
    assert_eq!(
        &streamed,
        events.as_array().unwrap(),
        "SSE and /events agree"
    );
    let r = h
        .get(&format!("/api/threads/{RANDOM}/stream"), Some(ALICE))
        .await;
    assert_eq!(r.status, 404);
    c.check("streamEvents", &r);
    let r = h.get(&format!("/api/threads/{id}/stream"), Some(BOB)).await;
    assert_eq!(r.status, 404);
    c.check("streamEvents", &r);
    // The 200 response itself: streamEvents is documented as text/event-stream.
    let contract_ct = c.contract.response("streamEvents", 200).unwrap().unwrap().0;
    assert_eq!(contract_ct, "text/event-stream");
    c.check(
        "streamEvents",
        &Resp {
            status: 200,
            content_type: "text/event-stream".into(),
            deprecation: sse
                .headers
                .get("deprecation")
                .map(|v| v.to_str().unwrap().to_owned()),
            body: vec![],
        },
    );

    // postMessage: blocked -> 202 Event; 409 on a finished thread; 404; 400.
    let ask = h.create(ALICE, "plain", "ask something").await;
    h.wait_state(ALICE, &ask, "blocked").await;
    let r = h
        .post(
            &format!("/api/threads/{ask}/messages"),
            Some(ALICE),
            json!({"text": "main"}),
        )
        .await;
    assert_eq!(r.status, 202);
    c.check("postMessage", &r);
    assert_eq!(r.json()["kind"], "user_message");
    assert_eq!(r.json()["data"], json!({"text": "main"}));
    h.wait_state(ALICE, &ask, "done").await;
    let r = h
        .post(
            &format!("/api/threads/{ask}/messages"),
            Some(ALICE),
            json!({"text": "late"}),
        )
        .await;
    assert_eq!(r.status, 409);
    c.check("postMessage", &r);
    assert_eq!(r.json()["detail"], "Thread is finished; start a new one");
    let r = h
        .post(
            &format!("/api/threads/{RANDOM}/messages"),
            Some(ALICE),
            json!({"text": "x"}),
        )
        .await;
    assert_eq!(r.status, 404);
    c.check("postMessage", &r);
    for bad in [
        json!({"text": ""}),
        json!({}),
        json!({"text": "x".repeat(100_001)}),
    ] {
        let r = h
            .post(&format!("/api/threads/{id}/messages"), Some(ALICE), bad)
            .await;
        assert_eq!(r.status, 400);
        c.check("postMessage", &r);
    }

    // cancelThread: 202 while running; 404.
    let slow = h.create(ALICE, "plain", "slow please").await;
    h.wait_state(ALICE, &slow, "working").await;
    let r = h
        .post_empty(&format!("/api/threads/{slow}/cancel"), Some(ALICE))
        .await;
    assert_eq!(r.status, 202);
    c.check("cancelThread", &r);
    assert!(r.body.is_empty());
    h.wait_state(ALICE, &slow, "cancelled").await;
    let r = h
        .post_empty(&format!("/api/threads/{slow}/cancel"), Some(ALICE))
        .await;
    assert_eq!(r.status, 202, "cancelling a finished thread is a no-op");
    c.check("cancelThread", &r);
    let r = h
        .post_empty(&format!("/api/threads/{RANDOM}/cancel"), Some(ALICE))
        .await;
    assert_eq!(r.status, 404);
    c.check("cancelThread", &r);

    c.assert_all_operations_exercised();
    // Known gaps of the contract (see the report): 400s that the spec does not document.
    let want: std::collections::BTreeSet<(String, u16)> = [
        ("listThreads", 400),
        ("listEvents", 400),
        ("postMessage", 400),
    ]
    .into_iter()
    .map(|(o, s)| (o.to_owned(), s))
    .collect();
    assert_eq!(c.undocumented, want);
    // Problems carry `about:blank`, a title and the status.
    let r = h.get("/api/threads?limit=0", Some(ALICE)).await;
    let p: Value = r.json();
    assert_eq!(p["type"], "about:blank");
    assert_eq!(p["status"], 400);
    assert_eq!(p["title"], "Bad Request");
}

/// The validator must actually bite: these instances violate the contract on purpose.
mod validator_bites {
    use serde_json::json;

    use super::support::contract::Contract;

    fn good_thread() -> serde_json::Value {
        json!({
            "id": "0190aaaa-0000-7000-8000-000000000123",
            "title": "t",
            "target": {"agentId": "a"},
            "state": "working",
            "createdAt": "2026-09-29T10:00:00Z",
            "updatedAt": "2026-09-29T10:00:00Z",
            "lastSeq": 1
        })
    }

    #[test]
    fn a_valid_thread_passes() {
        Contract::load().validate_component("Thread", &good_thread());
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn unknown_thread_state_is_rejected() {
        let mut t = good_thread();
        t["state"] = json!("canceled");
        Contract::load().validate_component("Thread", &t);
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn non_uuid_id_is_rejected() {
        let mut t = good_thread();
        t["id"] = json!("123");
        Contract::load().validate_component("Thread", &t);
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn bad_timestamp_is_rejected() {
        let mut t = good_thread();
        t["createdAt"] = json!("yesterday");
        Contract::load().validate_component("Thread", &t);
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn missing_required_field_is_rejected() {
        let mut t = good_thread();
        t.as_object_mut().unwrap().remove("lastSeq");
        Contract::load().validate_component("Thread", &t);
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn null_for_an_optional_field_is_rejected() {
        let mut t = good_thread();
        t["target"]["release"] = json!(null);
        Contract::load().validate_component("Thread", &t);
    }

    #[test]
    #[should_panic(expected = "does not match the contract")]
    fn event_with_a_bad_kind_is_rejected() {
        Contract::load().validate_component(
            "Event",
            &json!({
                "seq": 1, "threadId": "0190aaaa-0000-7000-8000-000000000123",
                "at": "2026-09-29T10:00:00Z", "kind": "banana",
                "actor": {"type": "user", "name": "a"}, "data": {}
            }),
        );
    }

    #[test]
    fn nullable_is_normalised() {
        let mut v = json!({"type": "string", "nullable": true});
        super::support::contract::normalize_nullable(&mut v);
        assert_eq!(v, json!({"type": ["string", "null"]}));
    }
}
