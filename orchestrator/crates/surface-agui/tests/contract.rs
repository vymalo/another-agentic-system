//! `docs/api/chat-api.yaml` against this surface: every `/agui/*` operation of the contract is
//! driven over real HTTP, and the statuses the contract documents for it are exactly the ones the
//! surface returns. Bodies are validated against the contract's schemas, which reference the
//! vendored AG-UI 1.0 JSON Schema by file; every stream frame is validated against the schema the
//! contract names for it (`AgUiSseFrame`, whose `data` carries an `AgUiEvent`).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};

use jsonschema::Validator;
use orch_agui_proto::SCHEMA_1_0;
use orch_ports::StoreError;
use serde_json::{Value, json};
use support::*;

/// How the contract points at the vendored schema, and what that resolves to.
const SCHEMA_REF: &str = "../../orchestrator/crates/agui-proto/schema/ag-ui-1.0.schema.json";
const CONTRACT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../docs/api/chat-api.yaml"
);
const RELEASES: &str = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";

struct Contract {
    doc: Value,
}

/// Points every reference to the vendored schema at `/agui` in the same document, where the
/// schema itself is embedded, so a validator needs no retrieval.
fn localize(v: &mut Value, inside_schema: bool) {
    match v {
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get_mut("$ref") {
                if let Some(rest) = r.strip_prefix(SCHEMA_REF) {
                    *r = format!("#/agui{}", rest.trim_start_matches('#'));
                } else if inside_schema && r.starts_with("#/$defs/") {
                    *r = format!("#/agui/{}", &r[2..]);
                }
            }
            for child in map.values_mut() {
                localize(child, inside_schema);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|c| localize(c, inside_schema)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

impl Contract {
    fn load() -> Self {
        let text = std::fs::read_to_string(CONTRACT).expect("contract file");
        let doc: Value = serde_norway::from_str(&text).expect("the contract is valid YAML");
        assert!(
            doc["openapi"]
                .as_str()
                .is_some_and(|v| v.starts_with("3.1.")),
            "OpenAPI 3.1 is JSON Schema 2020-12, which is what makes the external $ref valid"
        );
        Contract { doc }
    }

    /// The file the contract's `$ref`s name is the vendored schema this crate is tested against.
    fn assert_the_reference_resolves_to_the_vendored_schema(&self) {
        let path = std::path::Path::new(CONTRACT)
            .parent()
            .unwrap()
            .join(SCHEMA_REF);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} does not resolve: {e}", path.display()));
        assert_eq!(text, SCHEMA_1_0, "{}", path.display());
        let refs = self.doc["components"]["schemas"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(name, _)| name.starts_with("AgUi") && *name != "AgUiSseFrame")
            .map(|(name, schema)| (name.clone(), schema["$ref"].as_str().unwrap().to_owned()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            refs,
            BTreeMap::from([
                (
                    "AgUiAgentCapabilities".to_owned(),
                    format!("{SCHEMA_REF}#/$defs/AgentCapabilities")
                ),
                ("AgUiEvent".to_owned(), format!("{SCHEMA_REF}#/$defs/Event")),
                (
                    "AgUiRunAgentInput".to_owned(),
                    format!("{SCHEMA_REF}#/$defs/RunAgentInput")
                ),
            ])
        );
    }

    /// `(operationId, path, method)` of every `/agui/*` operation.
    fn agui_operations(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for (path, item) in self.doc["paths"].as_object().unwrap() {
            if !path.starts_with("/agui/") {
                continue;
            }
            for (method, op) in item.as_object().unwrap() {
                if let Some(id) = op.get("operationId").and_then(Value::as_str) {
                    out.push((id.to_owned(), path.clone(), method.clone()));
                }
            }
        }
        out
    }

    fn operation(&self, id: &str) -> &Value {
        let (_, path, method) = self
            .agui_operations()
            .into_iter()
            .find(|(op, _, _)| op == id)
            .unwrap_or_else(|| panic!("{id} is not an /agui operation of the contract"));
        &self.doc["paths"][path][method]
    }

    fn resolve<'a>(&'a self, node: &'a Value) -> &'a Value {
        match node.get("$ref").and_then(Value::as_str) {
            Some(r) => self
                .doc
                .pointer(r.strip_prefix('#').expect("a local reference"))
                .expect("dangling $ref"),
            None => node,
        }
    }

    /// The statuses the contract documents for `id`.
    fn statuses(&self, id: &str) -> BTreeSet<u16> {
        self.operation(id)["responses"]
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.parse().unwrap())
            .collect()
    }

    /// The `(content type, schema)` the contract documents for a status.
    fn body(&self, id: &str, status: u16) -> (String, Value, Option<Value>) {
        let response = self.resolve(&self.operation(id)["responses"][status.to_string()]);
        let (ct, media) = response["content"]
            .as_object()
            .unwrap_or_else(|| panic!("{id} {status} documents no body"))
            .iter()
            .next()
            .unwrap();
        (
            ct.clone(),
            media["schema"].clone(),
            media.get("x-itemSchema").cloned(),
        )
    }

    /// A validator for `schema`, a fragment of the contract, with the vendored schema embedded.
    fn validator(&self, schema: &Value) -> Validator {
        let mut root = schema.clone();
        let object = root.as_object_mut().expect("schema object");
        object.insert(
            "$schema".to_owned(),
            json!("https://json-schema.org/draft/2020-12/schema"),
        );
        object.insert("components".to_owned(), self.doc["components"].clone());
        let mut vendored: Value = serde_json::from_str(SCHEMA_1_0).unwrap();
        let vendored_object = vendored.as_object_mut().unwrap();
        vendored_object.remove("$id");
        vendored_object.remove("$schema");
        vendored_object.remove("$ref");
        object.insert("agui".to_owned(), vendored);
        localize(&mut root, false);
        let root_agui = root.get_mut("agui").unwrap();
        localize(root_agui, true);
        jsonschema::draft202012::options()
            .should_validate_formats(true)
            .build(&root)
            .expect("the contract's schema compiles")
    }

    fn validate(&self, schema: &Value, instance: &Value, what: &str) {
        let errors: Vec<String> = self
            .validator(schema)
            .iter_errors(instance)
            .map(|e| format!("{e} at {}", e.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "{what} does not match the contract:\n{}\ninstance: {instance}",
            errors.join("\n")
        );
    }

    fn component(&self, name: &str) -> Value {
        json!({"$ref": format!("#/components/schemas/{name}")})
    }
}

/// What the surface answered, per operation and status, against what the contract says.
struct Seen<'a> {
    contract: &'a Contract,
    seen: BTreeMap<String, BTreeSet<u16>>,
}

impl<'a> Seen<'a> {
    fn new(contract: &'a Contract) -> Self {
        Seen {
            contract,
            seen: BTreeMap::new(),
        }
    }

    fn note(&mut self, op: &str, status: u16) {
        let documented = self.contract.statuses(op);
        assert!(
            documented.contains(&status),
            "{op} answered {status}, which the contract does not document ({documented:?})"
        );
        self.seen.entry(op.to_owned()).or_default().insert(status);
    }

    /// A refusal: documented, and a problem.
    fn problem(&mut self, op: &str, status: u16, r: &Resp) {
        self.note(op, status);
        let p = r.problem(status);
        let (ct, schema, _) = self.contract.body(op, status);
        assert_eq!(ct, "application/problem+json");
        self.contract
            .validate(&schema, &p, &format!("{op} {status}"));
    }

    /// A stream: documented as `text/event-stream`, every frame an `AgUiSseFrame`.
    fn frames(&mut self, op: &str, stream: &Stream, frames: &[Frame]) {
        self.note(op, 200);
        let (ct, _, item) = self.contract.body(op, 200);
        assert_eq!(ct, "text/event-stream");
        assert!(
            stream.headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with(&ct)
        );
        let item = item.expect("the stream documents its items (x-itemSchema)");
        assert!(!frames.is_empty(), "{op}: an empty stream proves nothing");
        for frame in frames {
            // The frame as the SSE message says it: `data` is the event as a JSON string.
            let mut message = json!({"data": frame.event.to_string()});
            if let Some(id) = frame.id {
                message["id"] = json!(id.to_string());
            }
            self.contract
                .validate(&item, &message, &format!("{op} frame"));
            // The contract's own claim about `data`, checked, since `contentSchema` is an
            // annotation and a validator does not assert it.
            let event = self.contract.component("AgUiEvent");
            self.contract
                .validate(&event, &frame.event, &format!("{op} event"));
        }
    }
}

async fn raw_run(h: &Harness, agent: &str, headers: &[(&str, &str)], body: Vec<u8>) -> Resp {
    let mut req = h.client.post(h.url(&format!("/agui/agents/{agent}")));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    resp_of(req.body(body).send().await.unwrap()).await
}

#[tokio::test]
async fn the_agui_operations_answer_what_the_contract_documents() {
    let contract = Contract::load();
    contract.assert_the_reference_resolves_to_the_vendored_schema();
    let h = Harness::start().await;
    let mut seen = Seen::new(&contract);
    let json_headers = [
        ("X-Auth-Request-Email", ALICE),
        ("Content-Type", "application/json"),
        ("Accept", "text/event-stream"),
    ];

    // runAgent 200: a thread created by the run, request and response.
    let thread = new_thread_id();
    let body = input(&thread, "run-a", &[("m1", "echo contract")]);
    contract.validate(
        &contract.component("AgUiRunAgentInput"),
        &body,
        "the request",
    );
    let resp = h.post("plain", Some(ALICE), &body).await;
    assert_eq!(resp.status().as_u16(), 200);
    let mut stream = Stream::new(resp);
    let frames = stream.through_run().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    seen.frames("runAgent", &stream, &frames);
    h.wait_state(ALICE, &thread, "done").await;

    // runAgent 401, 404, 400, 406, 409, 413, 415, 422, 502, 503.
    let fresh = |run: &str| input(&new_thread_id(), run, &[("m", "echo hi")]);
    let r = h.refused("plain", None, &fresh("r")).await;
    seen.problem("runAgent", 401, &r);
    let r = h.refused("nobody", Some(ALICE), &fresh("r")).await;
    seen.problem("runAgent", 404, &r);
    let r = h
        .refused(
            "plain",
            Some(ALICE),
            &input("nope", "r", &[("m", "echo hi")]),
        )
        .await;
    seen.problem("runAgent", 400, &r);
    let r = raw_run(
        &h,
        "plain",
        &[
            ("X-Auth-Request-Email", ALICE),
            ("Content-Type", "application/json"),
            ("Accept", "application/json"),
        ],
        fresh("r").to_string().into_bytes(),
    )
    .await;
    seen.problem("runAgent", 406, &r);
    // A message on the finished thread is served (it starts the next job, ADR 0020); what is
    // refused with 409 is a run while another is open.
    let busy = new_thread_id();
    let mut open = h
        .run(
            "plain",
            ALICE,
            &input(&busy, "run-b", &[("m2", "gate wait")]),
        )
        .await;
    assert_eq!(
        open.next(std::time::Duration::from_secs(10))
            .await
            .unwrap()
            .kind(),
        "RUN_STARTED"
    );
    h.wait_state(ALICE, &busy, "working").await;
    let r = h
        .refused(
            "plain",
            Some(ALICE),
            &input(&busy, "run-c", &[("m2", "gate wait"), ("m3", "hurry")]),
        )
        .await;
    seen.problem("runAgent", 409, &r);
    h.agent.release_gate();
    open.all().await;
    let huge = "x".repeat(orch_surface_agui::MAX_BODY_BYTES + 1);
    let r = raw_run(
        &h,
        "plain",
        &json_headers,
        json!({"threadId": new_thread_id(), "runId": "r", "messages": [], "state": huge})
            .to_string()
            .into_bytes(),
    )
    .await;
    seen.problem("runAgent", 413, &r);
    let r = raw_run(
        &h,
        "plain",
        &[
            ("X-Auth-Request-Email", ALICE),
            ("Content-Type", "text/plain"),
            ("Accept", "text/event-stream"),
        ],
        fresh("r").to_string().into_bytes(),
    )
    .await;
    seen.problem("runAgent", 415, &r);
    let r = h
        .refused(
            "plain",
            Some(ALICE),
            &input(&new_thread_id(), "r", &[("a", "echo 1"), ("b", "echo 2")]),
        )
        .await;
    seen.problem("runAgent", 422, &r);
    // An A2UI action: for a surface the thread does not have, 422; too large, 413.
    let action = |name: &str| {
        input_with(
            &new_thread_id(),
            "r",
            &[],
            json!({"forwardedProps": {"a2uiAction": {"userAction": {
                "name": name, "surfaceId": "s1", "sourceComponentId": "b"}}}}),
        )
    };
    let r = h.refused("plain", Some(ALICE), &action("go")).await;
    seen.problem("runAgent", 422, &r);
    let r = h
        .refused("plain", Some(ALICE), &action(&"n".repeat(300)))
        .await;
    seen.problem("runAgent", 413, &r);
    h.agent.set_card_down("coder", true);
    let r = h
        .refused(
            "coder",
            Some(ALICE),
            &input_with(
                &new_thread_id(),
                "r",
                &[("m", "echo hi")],
                json!({"forwardedProps": {RELEASES: {"release": "staging"}}}),
            ),
        )
        .await;
    seen.problem("runAgent", 502, &r);
    h.agent.set_card_down("coder", false);
    h.store.fail_next_creates(1, || {
        StoreError::unavailable(std::io::Error::other("pool timed out"))
    });
    let r = h.refused("plain", Some(ALICE), &fresh("r")).await;
    seen.problem("runAgent", 503, &r);
    assert!(r.headers.contains_key("retry-after"));

    // connectThread 200 (a replay that ends: `mode=run` on a finished thread), 401, 404, 400, 406.
    let mut stream = h.connect_run(&thread, ALICE, None).await;
    let frames = stream.through_run().await;
    seen.frames("connectThread", &stream, &frames);
    let resp = h.connect_raw(&thread, None, None, None).await;
    seen.problem("connectThread", 401, &resp_of(resp).await);
    for other in [new_thread_id(), "not-a-uuid".to_owned()] {
        let resp = h.connect_raw(&other, Some(ALICE), None, None).await;
        seen.problem("connectThread", 404, &resp_of(resp).await);
    }
    let resp = h.connect_raw(&thread, Some(BOB), None, None).await;
    seen.problem("connectThread", 404, &resp_of(resp).await);
    for (last, query) in [
        (Some("abc"), None),
        (Some("-1"), None),
        (None, Some("mode=live")),
    ] {
        let resp = h.connect_raw(&thread, Some(ALICE), last, query).await;
        seen.problem("connectThread", 400, &resp_of(resp).await);
    }
    let resp = h
        .client
        .get(h.url(&format!("/agui/threads/{thread}/connect")))
        .header("X-Auth-Request-Email", ALICE)
        .header("Accept", "application/json")
        .send()
        .await
        .unwrap();
    seen.problem("connectThread", 406, &resp_of(resp).await);

    // getAgentCapabilities 200, 401, 404.
    let r = h.get("/agui/agents/coder/capabilities", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    let (ct, schema, _) = contract.body("getAgentCapabilities", 200);
    assert!(r.content_type.starts_with(&ct));
    assert_eq!(r.headers["cache-control"], "no-store");
    contract.validate(&schema, &r.json(), "getAgentCapabilities 200");
    seen.note("getAgentCapabilities", 200);
    let r = h.get("/agui/agents/plain/capabilities", None).await;
    seen.problem("getAgentCapabilities", 401, &r);
    let r = h.get("/agui/agents/nobody/capabilities", Some(ALICE)).await;
    seen.problem("getAgentCapabilities", 404, &r);

    // The documented statuses are the answered ones, operation by operation. A store that fails
    // to read a thread (a 503 of connectThread, `App::get_thread` through `problem_for`) cannot be
    // injected into the in-memory store, which only fails commits and creates; the status is
    // documented from the code path shared with every other route and named here so that it is
    // the one exemption and cannot grow unnoticed.
    let not_driven: BTreeMap<&str, BTreeSet<u16>> =
        BTreeMap::from([("connectThread", BTreeSet::from([503]))]);
    let operations: BTreeSet<String> = contract
        .agui_operations()
        .into_iter()
        .map(|(id, _, _)| id)
        .collect();
    assert_eq!(
        operations,
        BTreeSet::from([
            "runAgent".to_owned(),
            "connectThread".to_owned(),
            "getAgentCapabilities".to_owned()
        ])
    );
    for op in &operations {
        let mut answered = seen.seen.get(op).cloned().unwrap_or_default();
        answered.extend(not_driven.get(op.as_str()).into_iter().flatten());
        assert_eq!(
            answered,
            contract.statuses(op),
            "{op}: answered (plus the named exemption) and documented statuses differ"
        );
    }
}

/// The validator must bite: through the contract's references, the vendored schema is what
/// judges these instances.
mod validator_bites {
    use serde_json::json;

    use super::Contract;

    fn rejects(component: &str, instance: &serde_json::Value) -> bool {
        let contract = Contract::load();
        contract
            .validator(&contract.component(component))
            .iter_errors(instance)
            .next()
            .is_some()
    }

    #[test]
    fn events_run_inputs_and_capabilities_are_judged_by_the_vendored_schema() {
        let started = json!({"type": "RUN_STARTED", "threadId": "t", "runId": "r"});
        assert!(!rejects("AgUiEvent", &started));
        assert!(rejects("AgUiEvent", &json!({"type": "RUN_STARTED"})));
        assert!(rejects("AgUiEvent", &json!({"type": "BANANA"})));
        let input = json!({"threadId": "t", "runId": "r", "messages": []});
        assert!(!rejects("AgUiRunAgentInput", &input));
        assert!(rejects(
            "AgUiRunAgentInput",
            &json!({"threadId": "t", "messages": []})
        ));
        assert!(!rejects(
            "AgUiAgentCapabilities",
            &json!({"transport": {"streaming": true}})
        ));
        assert!(rejects(
            "AgUiAgentCapabilities",
            &json!({"transport": {"streaming": "yes"}})
        ));
    }

    #[test]
    fn a_frame_needs_data_and_a_numeric_id() {
        let contract = Contract::load();
        let (_, _, item) = contract.body("runAgent", 200);
        let item = item.unwrap();
        let validator = contract.validator(&item);
        let data = json!({"type": "RUN_FINISHED", "threadId": "t", "runId": "r"}).to_string();
        assert!(
            validator
                .iter_errors(&json!({"data": data, "id": "4"}))
                .next()
                .is_none()
        );
        assert!(validator.iter_errors(&json!({"id": "4"})).next().is_some());
        assert!(
            validator
                .iter_errors(&json!({"data": data, "id": "x"}))
                .next()
                .is_some()
        );
    }
}
