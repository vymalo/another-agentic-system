//! `GET /api/me/usage` over real HTTP (ADR 0065, decision 9): the caller's own month from the usage
//! ledger, priced with the configured table. Another person's usage is never in an answer (the route
//! has no owner parameter), the month defaults to the current UTC one, a malformed month or any other
//! key is a 400, and the answers are checked against `docs/api/chat-api.yaml`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, Policy, UsageSettings};
use orch_core::{
    Actor, AgentId, AgentTarget, Currency, EventBody, Micros, ModelPrice, ModelTokens,
    ModelUsageData, ModelUsageTotalData, PriceTable, SandboxEntry, SandboxEnvironment,
    SandboxPrice, SandboxUsage, ThreadId, ThreadState, TokenCounts, UserId, UserMessageData,
};
use orch_ports::memory::{
    FixedClock, MemoryAuth, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{
    AgentEndpoint, Commit, NewEvent, NewThreadRecord, NoModel, PortSet, Principal, Role,
    ThreadStore,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use uuid::Uuid;

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";
/// The clock of the stack: so the current UTC month is `2026-10`.
const NOW: &str = "2026-10-28T12:00:00Z";

// ---- the contract ------------------------------------------------------------------------------

struct Contract(Value);

impl Contract {
    fn load() -> Self {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/api/chat-api.yaml"
        );
        Contract(serde_norway::from_str(&std::fs::read_to_string(path).unwrap()).unwrap())
    }

    fn validate(&self, schema: &Value, instance: &Value) {
        let mut root = schema.clone();
        let object = root.as_object_mut().unwrap();
        object.insert(
            "$schema".to_owned(),
            json!("https://json-schema.org/draft/2020-12/schema"),
        );
        object.insert("components".to_owned(), self.0["components"].clone());
        let validator = jsonschema::draft202012::options()
            .should_validate_formats(true)
            .build(&root)
            .unwrap();
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|e| format!("{e} at {}", e.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{}\n{instance}", errors.join("\n"));
    }

    fn usage(&self, instance: &Value) {
        self.validate(&json!({"$ref": "#/components/schemas/MyUsage"}), instance);
    }

    fn problem(&self, instance: &Value) {
        let schema =
            self.0["components"]["responses"]["Problem"]["content"]["application/problem+json"]
                ["schema"]
                .clone();
        self.validate(&schema, instance);
    }
}

// ---- the harness -------------------------------------------------------------------------------

struct Resp {
    status: u16,
    content_type: String,
    cache_control: String,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(&self.body)))
    }
}

struct Harness {
    base: String,
    client: reqwest::Client,
    store: MemoryStore,
    contract: Contract,
    server: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

fn table() -> PriceTable {
    PriceTable::new(
        Currency::new("EUR").unwrap(),
        vec![ModelPrice {
            provider: Some("openai".into()),
            model: "glm-5.3".into(),
            input: Micros::new(400_000),
            cached_input: Micros::new(100_000),
            output: Micros::new(1_600_000),
        }],
        Some(SandboxPrice {
            cpu_core_hour: Micros::new(40_000),
            gib_hour: Micros::new(5_000),
        }),
    )
    .unwrap()
}

impl Harness {
    async fn start(usage: UsageSettings) -> Self {
        Self::with_policy(usage, Policy::default()).await
    }

    async fn with_policy(usage: UsageSettings, policy: Policy) -> Self {
        let directory = AgentDirectory::new(vec![AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new("coder"),
                "https://coder.example.com/.well-known/agent-card.json".to_owned(),
                None,
            ),
            name: "coder".to_owned(),
        }]);
        let auth = MemoryAuth::new();
        auth.allow("alice", principal(ALICE, &["user"]));
        auth.allow("bob", principal(BOB, &["user"]));
        // a person whose roles grant nothing (the deny-all policy below makes it so for everybody)
        auth.allow("roleless", principal("roleless@example.com", &[]));
        let store = MemoryStore::new();
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: orch_ports::NoArtifacts,
                    store: store.clone(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: FixedClock::new(NOW.parse().unwrap()),
                    ids: SeqIds::default(),
                    model: NoModel,
                    auth,
                    registry: directory.fixed_registry(),
                    user_tools: orch_ports::NoUserTools::off(),
                },
                directory,
                AppConfig {
                    stream_poll: Duration::from_millis(100),
                    policy,
                    usage,
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = orch_api::router(app, ApiConfig::default());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            store,
            contract: Contract::load(),
            server,
        }
    }

    async fn get(&self, path: &str, token: Option<&str>) -> Resp {
        let mut request = self.client.get(format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.unwrap();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned()
        };
        Resp {
            status: response.status().as_u16(),
            content_type: header("content-type"),
            cache_control: header("cache-control"),
            body: response.bytes().await.unwrap().to_vec(),
        }
    }

    /// The answer for `path` as `token`, a 200 that fits the contract.
    async fn usage(&self, path: &str, token: &str) -> Value {
        let r = self.get(path, Some(token)).await;
        assert_eq!(
            r.status,
            200,
            "{path}: {}",
            String::from_utf8_lossy(&r.body)
        );
        assert!(r.content_type.starts_with("application/json"));
        assert_eq!(r.cache_control, "no-store");
        let body = r.json();
        self.contract.usage(&body);
        body
    }

    /// A 400 problem of the contract.
    async fn bad_request(&self, path: &str, token: &str) {
        let r = self.get(path, Some(token)).await;
        assert_eq!(
            r.status,
            400,
            "{path}: {}",
            String::from_utf8_lossy(&r.body)
        );
        assert!(
            r.content_type.starts_with("application/problem+json"),
            "{path}: {}",
            r.content_type
        );
        self.contract.problem(&r.json());
    }

    /// A thread of `owner` whose log holds `events`.
    async fn thread(&self, owner: &str, events: Vec<NewEvent>) {
        let owner = UserId::new(owner);
        let mut all = vec![NewEvent {
            at: NOW.parse().unwrap(),
            actor: Actor::user(&owner),
            body: EventBody::UserMessage(UserMessageData::new("hi")),
            idempotency_key: None,
        }];
        all.extend(events);
        self.store
            .create_thread(
                NewThreadRecord {
                    id: ThreadId(Uuid::now_v7()),
                    owner,
                    title: "t".into(),
                    description: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("coder"),
                        release: None,
                    },
                    context_id: None,
                    rail_parent: None,
                    now: NOW.parse().unwrap(),
                },
                Commit {
                    new_state: ThreadState::Working,
                    job: None,
                    events: all,
                    outbox: Vec::new(),
                    binding: None,
                    now: NOW.parse().unwrap(),
                    lease: None,
                    watches: Vec::new(),
                    timers: Vec::new(),
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
    }
}

fn counts(input: u64, output: u64) -> TokenCounts {
    TokenCounts {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        ..TokenCounts::default()
    }
}

fn total(
    when: &str,
    task: &str,
    totals: Vec<ModelTokens>,
    sandbox: Option<SandboxUsage>,
) -> NewEvent {
    NewEvent {
        at: when.parse::<Timestamp>().unwrap(),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body: EventBody::ModelUsageTotal(ModelUsageTotalData {
            job: 1,
            agent: "coder".into(),
            task: task.into(),
            path: Vec::new(),
            totals,
            sandbox,
        }),
        idempotency_key: None,
    }
}

fn call(when: &str, task: &str, model: &str, tokens: TokenCounts) -> NewEvent {
    NewEvent {
        at: when.parse::<Timestamp>().unwrap(),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body: EventBody::ModelUsage(Box::new(ModelUsageData {
            job: 1,
            agent: "coder".into(),
            task: task.into(),
            call: format!("c-{task}-{model}"),
            path: Vec::new(),
            provider: Some("openai".into()),
            model: model.into(),
            tokens,
            context_window: None,
        })),
        idempotency_key: None,
    }
}

fn line(model: &str, tokens: TokenCounts) -> ModelTokens {
    ModelTokens {
        provider: Some("openai".into()),
        model: model.into(),
        tokens,
    }
}

fn priced() -> UsageSettings {
    UsageSettings {
        prices: Some(table()),
        ..UsageSettings::default()
    }
}

// ---- the tests ---------------------------------------------------------------------------------

/// The answer of the contract's own example: a priced model, a model with no price, a sandbox shape
/// and a task whose sandbox was not measured.
#[tokio::test]
async fn a_priced_month_with_an_unpriced_model_a_sandbox_and_an_unmeasured_task() {
    let h = Harness::start(priced()).await;
    let kube = SandboxUsage {
        environment: SandboxEnvironment::Kubernetes,
        usage: Some(vec![SandboxEntry {
            cpu_millis: Some(250),
            memory_bytes: Some(1 << 29),
            commands: 12,
            milliseconds: 81_234,
        }]),
    };
    let local = SandboxUsage {
        environment: SandboxEnvironment::Local,
        usage: None,
    };
    let mut parts = counts(1000, 500);
    parts.cached_input_tokens = Some(400);
    parts.reasoning_tokens = Some(120);
    h.thread(
        ALICE,
        vec![
            total(
                "2026-10-03T09:00:00Z",
                "t1",
                vec![line("glm-5.3", parts), line("glm-5.3-mini", counts(10, 5))],
                Some(kube),
            ),
            total(
                "2026-10-04T09:00:00Z",
                "t2",
                vec![line("glm-5.3", counts(1000, 0))],
                Some(local),
            ),
        ],
    )
    .await;

    let body = h.usage("/api/me/usage?month=2026-10", "alice").await;
    assert_eq!(
        body,
        json!({
            "month": "2026-10",
            "currency": "EUR",
            "prices": table().id().to_string(),
            "tasks": 2,
            "models": [
                {
                    "provider": "openai", "model": "glm-5.3",
                    "inputTokens": 2000, "outputTokens": 500, "totalTokens": 2500,
                    "reasoningTokens": 120, "cachedInputTokens": 400,
                    // 1 600 uncached * 0.40 + 400 cached * 0.10 + 500 out * 1.60, per million
                    // = 640 + 40 + 800 = 1480 millionths of a unit, i.e. 0.001480
                    "price": "0.001480"
                },
                {
                    "provider": "openai", "model": "glm-5.3-mini",
                    "inputTokens": 10, "outputTokens": 5, "totalTokens": 15,
                    "price": null
                }
            ],
            "sandbox": [
                {
                    "environment": "kubernetes",
                    "cpuMillis": 250, "memoryBytes": 536_870_912,
                    "commands": 12, "milliseconds": 81_234,
                    // 81234 / 3.6e6 h * (0.25 * 0.04 + 0.5 * 0.005) units = 282.0625 millionths, rounded: 0.000282
                    "price": "0.000282"
                }
            ],
            "unmeasured": [{"environment": "local", "tasks": 1}],
            "total": {
                "amount": "0.001762",
                "currency": "EUR",
                "complete": false,
                "unpriced": {
                    "models": [{"provider": "openai", "model": "glm-5.3-mini"}],
                    "sandbox": 0,
                    "unmeasured": 1
                }
            }
        })
    );
}

/// Amounts are strings with exactly six decimals, never JSON numbers, wherever they are.
#[tokio::test]
async fn money_is_a_string_with_six_decimals_and_nothing_else_is() {
    let h = Harness::start(priced()).await;
    h.thread(
        ALICE,
        vec![total(
            "2026-10-03T09:00:00Z",
            "t1",
            vec![line("glm-5.3", counts(1_000_000, 0))],
            None,
        )],
    )
    .await;
    let body = h.usage("/api/me/usage", "alice").await;
    assert_eq!(body["models"][0]["price"], "0.400000");
    assert_eq!(body["total"]["amount"], "0.400000");
    assert!(body["total"]["complete"].as_bool().unwrap());
    fn walk(path: &str, v: &Value) {
        match v {
            Value::Object(map) => map.iter().for_each(|(k, v)| {
                if matches!(k.as_str(), "price" | "amount") {
                    assert!(v.is_string() || v.is_null(), "{path}.{k}: {v}");
                }
                walk(&format!("{path}.{k}"), v);
            }),
            Value::Array(items) => items.iter().enumerate().for_each(|(i, v)| {
                walk(&format!("{path}[{i}]"), v);
            }),
            _ => {}
        }
    }
    walk("$", &body);
}

#[tokio::test]
async fn the_answer_is_the_callers_own_and_nobody_elses() {
    let h = Harness::start(priced()).await;
    h.thread(
        ALICE,
        vec![total(
            "2026-10-03T09:00:00Z",
            "t1",
            vec![line("glm-5.3", counts(100, 10))],
            None,
        )],
    )
    .await;
    h.thread(
        BOB,
        vec![total(
            "2026-10-05T09:00:00Z",
            "t1",
            vec![line("glm-5.3", counts(7, 1))],
            None,
        )],
    )
    .await;
    let alice = h.usage("/api/me/usage", "alice").await;
    let bob = h.usage("/api/me/usage", "bob").await;
    assert_eq!(alice["models"][0]["inputTokens"], 100);
    assert_eq!(bob["models"][0]["inputTokens"], 7);
    assert_eq!(
        (alice["tasks"].clone(), bob["tasks"].clone()),
        (json!(1), json!(1))
    );
}

#[tokio::test]
async fn there_is_no_owner_parameter_so_nobody_asks_for_another_persons_usage() {
    let h = Harness::start(priced()).await;
    h.thread(
        BOB,
        vec![total(
            "2026-10-05T09:00:00Z",
            "t1",
            vec![line("glm-5.3", counts(7, 1))],
            None,
        )],
    )
    .await;
    for query in [
        "?owner=bob@example.com",
        "?user=bob@example.com",
        "?month=2026-10&owner=bob@example.com",
        "?email=bob@example.com",
        "?owner=",
    ] {
        h.bad_request(&format!("/api/me/usage{query}"), "alice")
            .await;
    }
    // and without the parameter alice sees an empty month, not bob's
    let alice = h.usage("/api/me/usage", "alice").await;
    assert_eq!(
        (alice["tasks"].clone(), alice["models"].clone()),
        (json!(0), json!([]))
    );
}

#[tokio::test]
async fn the_month_is_the_current_utc_month_unless_one_is_named() {
    let h = Harness::start(priced()).await;
    h.thread(
        ALICE,
        vec![
            total(
                "2026-10-03T09:00:00Z",
                "oct",
                vec![line("glm-5.3", counts(10, 1))],
                None,
            ),
            total(
                "2026-09-30T23:59:59Z",
                "sep",
                vec![line("glm-5.3", counts(20, 2))],
                None,
            ),
            call("2026-08-15T09:00:00Z", "aug", "glm-5.3", counts(30, 3)),
        ],
    )
    .await;
    let default = h.usage("/api/me/usage", "alice").await;
    assert_eq!(default["month"], "2026-10");
    assert_eq!(default["models"][0]["inputTokens"], 10);
    let september = h.usage("/api/me/usage?month=2026-09", "alice").await;
    assert_eq!(september["month"], "2026-09");
    assert_eq!(september["models"][0]["inputTokens"], 20);
    // calls of a task with no total are counted until its total comes
    let august = h.usage("/api/me/usage?month=2026-08", "alice").await;
    assert_eq!(august["models"][0]["inputTokens"], 30);
    // a month with nothing in it, past, future or far from either, is an empty complete answer
    for month in ["2026-07", "2027-01", "1999-12", "0001-01", "9999-12"] {
        let empty = h
            .usage(&format!("/api/me/usage?month={month}"), "alice")
            .await;
        assert_eq!(empty["month"], month);
        assert_eq!(
            (
                empty["tasks"].clone(),
                empty["models"].clone(),
                empty["sandbox"].clone(),
                empty["unmeasured"].clone()
            ),
            (json!(0), json!([]), json!([]), json!([])),
            "{month}"
        );
        assert_eq!(empty["total"]["amount"], "0.000000");
        assert_eq!(empty["total"]["complete"], true);
    }
}

#[tokio::test]
async fn a_malformed_month_and_nothing_else_is_a_400() {
    let h = Harness::start(priced()).await;
    for bad in [
        "month=2026-13",
        "month=2026-00",
        "month=2026-1",
        "month=26-10",
        "month=202610",
        "month=2026-10-01",
        "month=2026/10",
        "month=abcd-ef",
        "month=",
        "month=%202026-10",
        "month=2026-10%20",
        "month=%2B2026-10",
        "month=2026-10&month=2026-09",
        "month=0000-01",
        "month=2026-10&other=1",
        "other=1",
    ] {
        h.bad_request(&format!("/api/me/usage?{bad}"), "alice")
            .await;
    }
    // the unknown query key and the month are both fine on their own
    h.usage("/api/me/usage?month=2026-10", "alice").await;
    h.usage("/api/me/usage", "alice").await;
}

#[tokio::test]
async fn without_a_table_nothing_is_priced_and_no_currency_or_table_is_named() {
    let h = Harness::start(UsageSettings::default()).await;
    let kube = SandboxUsage {
        environment: SandboxEnvironment::Kubernetes,
        usage: Some(vec![SandboxEntry {
            cpu_millis: Some(250),
            memory_bytes: Some(1 << 29),
            commands: 1,
            milliseconds: 1000,
        }]),
    };
    h.thread(
        ALICE,
        vec![total(
            "2026-10-03T09:00:00Z",
            "t1",
            vec![line("glm-5.3", counts(100, 10))],
            Some(kube),
        )],
    )
    .await;
    let body = h.usage("/api/me/usage", "alice").await;
    assert_eq!(body["currency"], Value::Null);
    assert_eq!(body["prices"], Value::Null);
    assert_eq!(body["models"][0]["price"], Value::Null);
    assert_eq!(body["sandbox"][0]["price"], Value::Null);
    assert_eq!(body["total"]["amount"], "0.000000");
    assert_eq!(body["total"]["currency"], Value::Null);
    assert_eq!(body["total"]["complete"], false);
    assert_eq!(
        body["total"]["unpriced"],
        json!({"models": [{"provider": "openai", "model": "glm-5.3"}], "sandbox": 1, "unmeasured": 0})
    );
    // with no usage at all it is complete, with or without a table
    let empty = h.usage("/api/me/usage?month=2026-01", "alice").await;
    assert_eq!(empty["currency"], Value::Null);
    assert_eq!(empty["total"]["complete"], true);
}

#[tokio::test]
async fn a_model_with_no_provider_and_a_sandbox_with_no_request_read_as_they_are() {
    let h = Harness::start(priced()).await;
    let kube = SandboxUsage {
        environment: SandboxEnvironment::Kubernetes,
        usage: Some(vec![SandboxEntry {
            cpu_millis: None,
            memory_bytes: None,
            commands: 2,
            milliseconds: 5,
        }]),
    };
    h.thread(
        ALICE,
        vec![total(
            "2026-10-03T09:00:00Z",
            "t1",
            vec![ModelTokens {
                provider: None,
                model: "bare".into(),
                tokens: counts(3, 4),
            }],
            Some(kube),
        )],
    )
    .await;
    let body = h.usage("/api/me/usage", "alice").await;
    let model = &body["models"][0];
    assert!(model.get("provider").is_none(), "{model}");
    assert_eq!(
        (model["model"].clone(), model["price"].clone()),
        (json!("bare"), Value::Null)
    );
    let sandbox = &body["sandbox"][0];
    assert!(sandbox.get("cpuMillis").is_none() && sandbox.get("memoryBytes").is_none());
    assert_eq!(sandbox["price"], Value::Null);
    assert_eq!(
        body["total"]["unpriced"],
        json!({"models": [{"model": "bare"}], "sandbox": 1, "unmeasured": 0})
    );
}

#[tokio::test]
async fn it_asks_for_a_credential_as_the_other_routes_do() {
    let h = Harness::start(priced()).await;
    for path in ["/api/me/usage", "/api/me/usage?month=2026-10"] {
        let none = h.get(path, None).await;
        assert_eq!(none.status, 401, "{path}");
        assert!(none.content_type.starts_with("application/problem+json"));
        h.contract.problem(&none.json());
        let wrong = h.get(path, Some("not-a-token")).await;
        assert_eq!(wrong.status, 401, "{path}");
    }
    // a malformed month with no credential is still a 401: identity comes first
    let none = h.get("/api/me/usage?month=nonsense", None).await;
    assert_eq!(none.status, 401);
}

/// As `GET /api/me`, the route answers a person whose roles grant nothing: it is their own data, and
/// it changes nothing.
#[tokio::test]
async fn a_person_whose_roles_grant_nothing_reads_their_own_usage() {
    let h = Harness::with_policy(priced(), Policy::deny_all()).await;
    // every other route is a 403 for them
    assert_eq!(h.get("/api/threads", Some("roleless")).await.status, 403);
    assert_eq!(h.get("/api/me", Some("roleless")).await.status, 200);
    let body = h.usage("/api/me/usage", "roleless").await;
    assert_eq!(body["tasks"], 0);
    h.thread(
        "roleless@example.com",
        vec![total(
            "2026-10-03T09:00:00Z",
            "t1",
            vec![line("glm-5.3", counts(5, 5))],
            None,
        )],
    )
    .await;
    let body = h.usage("/api/me/usage", "roleless").await;
    assert_eq!(body["models"][0]["inputTokens"], 5);
}

#[tokio::test]
async fn other_methods_are_not_allowed() {
    let h = Harness::start(priced()).await;
    for method in [
        reqwest::Method::POST,
        reqwest::Method::PUT,
        reqwest::Method::PATCH,
        reqwest::Method::DELETE,
    ] {
        let r = h
            .client
            .request(method.clone(), format!("{}/api/me/usage", h.base))
            .bearer_auth("alice")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status().as_u16(), 405, "{method}");
    }
}
