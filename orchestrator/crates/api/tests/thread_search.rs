//! `POST /api/search/threads` (`searchThreads`, ADR 0068) over real HTTP against the in-memory stack: the
//! answers are validated against `docs/api/chat-api.yaml`, the corpus and queries of
//! `docs/api/examples/thread-search/` are asked of it (the vectors the web's mock server passes too), and
//! the rules that make search safe are held: only a person's own threads, what `listThreads` lists,
//! nothing of the query in any log line, words that look like syntax are words.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use jiff::Timestamp;
use orch_api::{ApiConfig, AuthorizeLimits};
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, Permission, Policy, RoleGrant};
use orch_core::{
    Actor, AgentId, AgentMessageData, AgentTarget, EventBody, ForkKind, MessagePurpose, ThreadId,
    ThreadState, UserId, UserMessageData,
};
use orch_ports::memory::{
    MemoryArtifacts, MemoryAuth, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{
    AgentEndpoint, Arrangement, Commit, FixedRegistry, ForkOrigin, NewEvent, NewThreadRecord,
    NoModel, PortSet, Principal, Role, SystemClock, ThreadStore,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";
const ROOT: &str = "root@example.com";

type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    MemoryAuth,
    MemoryArtifacts,
>;

const CORPUS: &str = include_str!("../../../../docs/api/examples/thread-search/corpus.json");
const REQUESTS: &str = include_str!("../../../../docs/api/examples/thread-search/requests.json");
/// Every file of `queries/`, read from the directory (the web's mock reads the directory too, so a new file
/// can never be asked of one and not of the other).
fn queries() -> Vec<(String, String)> {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/api/examples/thread-search/queries"
    );
    let mut files: Vec<(String, String)> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(&path).unwrap(),
            )
        })
        .filter(|(name, _)| name.ends_with(".json"))
        .collect();
    files.sort();
    files
}

// ---- the contract ---------------------------------------------------------------------------

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

    fn page(&self, instance: &Value) {
        self.validate(
            &json!({"$ref": "#/components/schemas/ThreadSearchPage"}),
            instance,
        );
    }

    fn problem(&self, status: u16, instance: &Value) {
        let schema =
            self.0["components"]["responses"]["Problem"]["content"]["application/problem+json"]
                ["schema"]
                .clone();
        self.validate(&schema, instance);
        assert!(
            self.0["paths"]["/api/search/threads"]["post"]["responses"]
                .get(status.to_string())
                .is_some(),
            "the contract documents no {status} for searchThreads"
        );
    }
}

// ---- the harness -----------------------------------------------------------------------------

struct Resp {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn code(&self) -> Option<String> {
        self.json()["code"].as_str().map(str::to_owned)
    }
}

struct Harness {
    base: String,
    client: reqwest::Client,
    app: Arc<App<Stack>>,
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

fn t0() -> Timestamp {
    "2026-11-14T10:00:00Z".parse().unwrap()
}

impl Harness {
    async fn start() -> Self {
        Self::with(ApiConfig::default(), Policy::default()).await
    }

    async fn with(cfg: ApiConfig, policy: Policy) -> Self {
        let entry = |id: &str| AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new(id),
                format!("https://{id}.example.com/.well-known/agent-card.json"),
                None,
            ),
            name: id.to_owned(),
        };
        let directory = AgentDirectory::new(vec![entry("plain")]);
        let auth = MemoryAuth::new();
        auth.allow("alice", principal(ALICE, &["user"]));
        auth.allow("bob", principal(BOB, &["user"]));
        auth.allow("root", principal(ROOT, &["admin"]));
        auth.allow("chat", principal("chat@example.com", &["chat-only"]));
        auth.allow("nobody", principal("nobody@example.com", &["no-read"]));
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: MemoryArtifacts::new(),
                    store: MemoryStore::new(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: SystemClock,
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
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = orch_api::router_with_surfaces(Arc::clone(&app), cfg, Vec::new());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            app,
            contract: Contract::load(),
            server,
        }
    }

    async fn post_raw(&self, token: &str, content_type: Option<&str>, body: Vec<u8>) -> Resp {
        let mut req = self
            .client
            .post(format!("{}/api/search/threads", self.base))
            .bearer_auth(token);
        if let Some(ct) = content_type {
            req = req.header("Content-Type", ct);
        }
        let resp = req.body(body).send().await.unwrap();
        Resp {
            status: resp.status().as_u16(),
            headers: resp.headers().clone(),
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    async fn search(&self, token: &str, body: &Value) -> Resp {
        self.post_raw(
            token,
            Some("application/json"),
            serde_json::to_vec(body).unwrap(),
        )
        .await
    }

    /// A search that is a 200 and matches the contract.
    async fn found(&self, token: &str, body: &Value) -> Value {
        let r = self.search(token, body).await;
        assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
        assert_eq!(r.headers["cache-control"], "no-store");
        let page = r.json();
        self.contract.page(&page);
        page
    }

    /// A refusal: `status`, the `code`, a problem of the contract that the contract documents.
    async fn refused(&self, r: Resp, status: u16, code: Option<&str>) -> Value {
        assert_eq!(r.status, status, "{}", String::from_utf8_lossy(&r.body));
        assert_eq!(r.code().as_deref(), code);
        let body = r.json();
        self.contract.problem(status, &body);
        body
    }

    async fn get_threads(&self, token: &str, query: &str) -> Vec<String> {
        let r = self
            .client
            .get(format!("{}/api/threads?limit=100{query}", self.base))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        r.json::<Vec<Value>>()
            .await
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect()
    }

    fn store(&self) -> &MemoryStore {
        &self.app.ports().store
    }

    /// A thread of `owner` with these messages (a person's, in order).
    async fn thread(&self, owner: &str, n: u128, title: &str, texts: &[&str]) -> ThreadId {
        let id = ThreadId(uuid::Uuid::from_u128(
            0x0190_aaaa_0000_7000_8000_0000_0000_0000 + n,
        ));
        let events = texts.iter().map(|t| say(owner, t)).collect();
        self.store()
            .create_thread(new_thread(id, owner, title, None), commit(events))
            .await
            .unwrap();
        id
    }
}

fn new_thread(
    id: ThreadId,
    owner: &str,
    title: &str,
    description: Option<&str>,
) -> NewThreadRecord {
    NewThreadRecord {
        id,
        owner: UserId::new(owner),
        title: title.to_owned(),
        description: description.map(Into::into),
        target: AgentTarget {
            agent_id: AgentId::new("plain"),
            release: None,
        },
        context_id: None,
        rail_parent: None,
        now: t0(),
    }
}

fn say(owner: &str, text: &str) -> NewEvent {
    NewEvent {
        at: t0(),
        actor: Actor::user(&UserId::new(owner)),
        body: EventBody::UserMessage(UserMessageData::new(text)),
        idempotency_key: None,
    }
}

fn commit(events: Vec<NewEvent>) -> Commit {
    Commit {
        new_state: ThreadState::Done,
        job: None,
        events,
        outbox: Vec::new(),
        binding: None,
        now: t0(),
        lease: None,
        watches: Vec::new(),
        timers: Vec::new(),
        inbox: None,
        finishes_outbox: None,
        title: None,
        description: None,
        sharing: None,
        skip_unsent_delegates: false,
    }
}

fn result_ids(page: &Value) -> Vec<String> {
    page["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["thread"]["id"].as_str().unwrap().to_owned())
        .collect()
}

// ---- the corpus -------------------------------------------------------------------------------

/// Plays the corpus of the vectors into the store, as ALICE's: a thread with `copiedThrough` is a fork of
/// the thread that says the same words (`parent`), cut where it says; an `edit` is a fork of kind edit.
async fn play_corpus(h: &Harness) {
    let corpus: Value = serde_json::from_str(CORPUS).unwrap();
    let threads = corpus["threads"].as_array().unwrap();
    for t in threads {
        let id: ThreadId = t["id"].as_str().unwrap().parse().unwrap();
        let title = &text_of(&t["title"]);
        let description = (!t["description"].is_null()).then(|| text_of(&t["description"]));
        let parent: ThreadId = t["parent"]
            .as_str()
            .unwrap_or("0190aaaa-0000-7000-8000-000000000003")
            .parse()
            .unwrap();
        let events: Vec<&Value> = t["events"].as_array().unwrap().iter().collect();
        let own = |from: i64| -> Vec<NewEvent> {
            let mut out = Vec::new();
            let mut skipped = false;
            for e in events.iter().filter(|e| e["seq"].as_i64().unwrap() > from) {
                let at: Timestamp = e["at"].as_str().unwrap().parse().unwrap();
                let body = match e["kind"].as_str().unwrap() {
                    "user_message" => {
                        EventBody::UserMessage(UserMessageData::new(text_of(&e["text"])))
                    }
                    "agent_message" => EventBody::AgentMessage(AgentMessageData {
                        text: text_of(&e["text"]),
                        message_id: e["messageId"].as_str().unwrap().to_owned(),
                        is_final: e["final"].as_bool().unwrap(),
                        purpose: match e["purpose"].as_str() {
                            Some("working") => Some(MessagePurpose::Working),
                            Some("answer") => Some(MessagePurpose::Answer),
                            _ => None,
                        },
                        via: None,
                    }),
                    _ => {
                        skipped = true; // an event nothing searches: only ever the last
                        continue;
                    }
                };
                assert!(
                    !skipped,
                    "an event that is not played comes before one that is"
                );
                out.push(NewEvent {
                    at,
                    actor: Actor::user(&UserId::new(ALICE)),
                    body,
                    idempotency_key: None,
                });
            }
            out
        };
        let record = new_thread(id, ALICE, title, description.as_deref());
        if t["edit"].as_bool().unwrap() {
            // a branch of the first thread cut at nothing: its log is its own
            h.store()
                .fork_thread(
                    record,
                    ForkOrigin {
                        parent,
                        cut: 0,
                        kind: ForkKind::Edit,
                    },
                    commit(own(0)),
                )
                .await
                .unwrap();
        } else if let Some(cut) = t["copiedThrough"].as_i64() {
            h.store()
                .fork_thread(
                    record,
                    ForkOrigin {
                        parent,
                        cut,
                        kind: ForkKind::Fork,
                    },
                    commit(own(cut)),
                )
                .await
                .unwrap();
        } else {
            h.store()
                .create_thread(record, commit(own(0)))
                .await
                .unwrap();
        }
        if t["archived"].as_bool().unwrap() {
            h.store()
                .arrange_thread(
                    &UserId::new(ALICE),
                    id,
                    Arrangement {
                        archived: Some(true),
                        ..Arrangement::default()
                    },
                    t0(),
                )
                .await
                .unwrap();
        }
    }
}

/// A text of the corpus: a string, or `{repeat, times, suffix?}`.
fn text_of(v: &Value) -> String {
    match expand(v) {
        Value::String(s) => s,
        other => panic!("not a text: {other}"),
    }
}

fn expand(v: &Value) -> Value {
    match v {
        Value::Object(o) if o.contains_key("repeat") => {
            let unit = o["repeat"].as_str().unwrap();
            let times = usize::try_from(o["times"].as_u64().unwrap()).unwrap();
            let suffix = o.get("suffix").and_then(Value::as_str).unwrap_or("");
            Value::String(format!("{}{suffix}", unit.repeat(times)))
        }
        other => other.clone(),
    }
}

/// Every request of every query file, answered exactly as the vectors say (the web's mock server answers
/// the same): the terms, the threads in order, the ranks, the counts, the hits and their snippets with
/// UTF-16 offsets, whether a page follows, and the refusals' codes.
#[tokio::test]
async fn the_corpus_answers_every_vector() {
    // more requests than a person's burst: the limiter is not what is asked here
    let h = Harness::with(
        ApiConfig {
            search_limits: AuthorizeLimits {
                burst: 1000,
                refill: Duration::from_millis(1),
            },
            ..ApiConfig::default()
        },
        Policy::default(),
    )
    .await;
    play_corpus(&h).await;
    let files = queries();
    assert!(files.len() >= 25, "{} query files", files.len());
    for (file, text) in &files {
        let query: Value = serde_json::from_str(text).unwrap();
        let mut cursor: Option<String> = None;
        for (n, step) in query["steps"].as_array().unwrap().iter().enumerate() {
            let label = format!("{file} step {n}");
            let mut request = step["request"].clone();
            request["q"] = expand(&request["q"]);
            if request["cursor"] == "$previous" {
                request["cursor"] = cursor.clone().map_or(Value::Null, Value::String);
            }
            let r = h.search("alice", &request).await;
            let expect = &step["expect"];
            if let Some(code) = expect["error"].as_str() {
                // the contract's `bad_request` is the problem's code for every other malformed request
                assert_eq!(r.status, 400, "{label}");
                assert_eq!(r.code().as_deref(), Some(code), "{label}");
                h.contract.problem(400, &r.json());
                cursor = None;
                continue;
            }
            assert_eq!(
                r.status,
                200,
                "{label}: {}",
                String::from_utf8_lossy(&r.body)
            );
            let page = r.json();
            h.contract.page(&page);
            assert_eq!(page["query"]["terms"], expect["terms"], "{label}");
            let want: Vec<Value> = expect["results"].as_array().unwrap().clone();
            let got = page["results"].as_array().unwrap();
            assert_eq!(got.len(), want.len(), "{label}: {page}");
            for (g, w) in got.iter().zip(&want) {
                assert_eq!(g["thread"]["id"], w["id"], "{label}");
                assert_eq!(g["rank"], w["rank"], "{label}");
                assert_eq!(g["messageHits"], w["messageHits"], "{label}");
                assert_eq!(g["hits"], w["hits"], "{label}: {}", w["id"]);
            }
            assert_eq!(
                page.get("next").is_some(),
                expect["hasNext"].as_bool().unwrap(),
                "{label}"
            );
            cursor = page["next"].as_str().map(str::to_owned);
        }
    }
}

/// Every raw request of `requests.json` is answered with the status and code it says (the web's mock server
/// is held to the same file): the media type, the shape of the body, the query, and which error wins.
#[tokio::test]
async fn every_request_vector_is_answered_as_it_says() {
    let h = Harness::with(
        ApiConfig {
            search_limits: AuthorizeLimits {
                burst: 10_000,
                refill: Duration::from_millis(1),
            },
            ..ApiConfig::default()
        },
        Policy::default(),
    )
    .await;
    let file: Value = serde_json::from_str(REQUESTS).unwrap();
    let cases = file["cases"].as_array().unwrap();
    assert!(cases.len() >= 60, "{}", cases.len());
    let mut failures = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let content_type = match case.get("headers") {
            None => Some("application/json".to_owned()),
            Some(headers) => headers["content-type"].as_str().map(str::to_owned),
        };
        let body = case["body"].as_str().unwrap().as_bytes().to_vec();
        let r = h.post_raw("alice", content_type.as_deref(), body).await;
        let want = &case["expect"];
        let want_status = u16::try_from(want["status"].as_u64().unwrap()).unwrap();
        let want_code = want["code"].as_str().map(str::to_owned);
        if r.status != want_status || r.code() != want_code {
            failures.push(format!(
                "{name}: wanted {want_status} {want_code:?}, got {} {:?}",
                r.status,
                r.code()
            ));
            continue;
        }
        if want_status == 200 {
            h.contract.page(&r.json());
            assert_eq!(r.json()["query"]["terms"], want["terms"], "{name}");
        } else {
            h.contract.problem(want_status, &r.json());
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---- the rules --------------------------------------------------------------------------------

#[tokio::test]
async fn another_users_thread_never_appears_by_title_or_content_for_any_role() {
    let h = Harness::start().await;
    h.thread(ALICE, 1, "Pineapple plans", &["nothing"]).await;
    h.thread(ALICE, 2, "Other", &["a pineapple in a message"])
        .await;
    h.thread(BOB, 3, "Bobs pineapple", &["pineapple pineapple"])
        .await;
    for (token, want) in [("alice", 2), ("bob", 1), ("root", 0), ("chat", 0)] {
        let page = h.found(token, &json!({"q": "pineapple"})).await;
        assert_eq!(page["results"].as_array().unwrap().len(), want, "{token}");
    }
    // the administrator searches their own threads and finds them
    h.thread(ROOT, 4, "Root pineapple", &[]).await;
    let page = h.found("root", &json!({"q": "pineapple"})).await;
    assert_eq!(page["results"].as_array().unwrap().len(), 1);
    assert_eq!(page["results"][0]["thread"]["owner"], ROOT);
}

#[tokio::test]
async fn the_results_are_a_subset_of_what_list_threads_lists() {
    let h = Harness::start().await;
    play_corpus(&h).await;
    for archived in ["exclude", "only", "include"] {
        for branches in [false, true] {
            let mut body = json!({"q": "zebra", "archived": archived, "limit": 50});
            let mut query = format!("&archived={archived}");
            if branches {
                body["branches"] = json!("include");
                query.push_str("&branches=include");
            }
            let page = h.found("alice", &body).await;
            let listed = h.get_threads("alice", &query).await;
            for id in result_ids(&page) {
                assert!(listed.contains(&id), "{archived} {branches}: {id}");
            }
            assert!(!result_ids(&page).is_empty());
        }
    }
}

#[tokio::test]
async fn owner_is_refused_like_any_member_that_is_not_one() {
    let h = Harness::start().await;
    for body in [
        json!({"q": "x1", "owner": ALICE}),
        json!({"q": "x1", "owner": "*"}),
        json!({"q": "x1", "owner": ""}),
        json!({"q": "x1", "extra": 1}),
    ] {
        let r = h.search("alice", &body).await;
        h.refused(r, 400, Some("bad_request")).await;
    }
}

#[tokio::test]
async fn a_deleted_threads_words_are_gone_at_once() {
    let h = Harness::start().await;
    let id = h.thread(ALICE, 1, "Walrus", &["walrus words"]).await;
    h.thread(ALICE, 2, "Keeper", &["walrus too"]).await;
    assert_eq!(
        result_ids(&h.found("alice", &json!({"q": "walrus"})).await).len(),
        2
    );
    let record = h.store().get_thread(None, id).await.unwrap().unwrap();
    h.store()
        .delete_threads(&UserId::new(ALICE), &[(id, record.version)], t0())
        .await
        .unwrap();
    let page = h.found("alice", &json!({"q": "walrus"})).await;
    assert_eq!(
        result_ids(&page),
        vec!["0190aaaa-0000-7000-8000-000000000002"]
    );
}

#[tokio::test]
async fn the_length_of_a_query_is_bounded_and_counted_in_characters() {
    let h = Harness::start().await;
    h.found("alice", &json!({"q": "x".repeat(200)})).await;
    h.found("alice", &json!({"q": "\u{1f389}".repeat(200)}))
        .await;
    let r = h.search("alice", &json!({"q": "x".repeat(201)})).await;
    h.refused(r, 400, Some("query_too_long")).await;
    let r = h
        .search("alice", &json!({"q": "\u{1f389}".repeat(201)}))
        .await;
    h.refused(r, 400, Some("query_too_long")).await;
    let r = h.search("alice", &json!({"q": " \t\n "})).await;
    h.refused(r, 400, Some("query_empty")).await;
}

#[tokio::test]
async fn nine_terms_use_eight() {
    let h = Harness::start().await;
    let page = h
        .found("alice", &json!({"q": "aa bb cc dd ee ff gg hh ii"}))
        .await;
    assert_eq!(
        page["query"]["terms"],
        json!(["aa", "bb", "cc", "dd", "ee", "ff", "gg", "hh"])
    );
}

#[tokio::test]
async fn text_that_looks_like_sql_or_a_tsquery_is_only_words() {
    let h = Harness::start().await;
    h.thread(ALICE, 1, "Orphan", &["drop table threads"]).await;
    let page = h.found("alice", &json!({"q": "' OR 1=1 --"})).await;
    assert_eq!(page["query"]["terms"], json!(["or"]));
    assert_eq!(result_ids(&page).len(), 1, "or is the start of orphan");
    for q in [
        "foo:* | bar & !baz <-> qux",
        "x'); DROP TABLE threads;--",
        "\"quoted\" (paren) {brace} \\ back `tick`",
        "'; SELECT pg_sleep(10); --",
    ] {
        let page = h.found("alice", &json!({"q": q})).await;
        assert!(
            page["query"]["terms"].as_array().unwrap().iter().all(|t| t
                .as_str()
                .unwrap()
                .chars()
                .all(char::is_alphanumeric)),
            "{q}"
        );
    }
    // the store is as it was
    let page = h.found("alice", &json!({"q": "threads"})).await;
    assert_eq!(result_ids(&page).len(), 1);
}

#[tokio::test]
async fn operators_and_quotes_are_punctuation() {
    let h = Harness::start().await;
    h.thread(ALICE, 1, "Both", &["foo and bar"]).await;
    h.thread(ALICE, 2, "Only foo", &["foo"]).await;
    // `-bar` does not exclude, quotes do not make a phrase, `AND` is a word the message has
    for q in ["foo -bar", "\"foo bar\"", "foo AND bar", "(foo) (bar)"] {
        let ids = result_ids(&h.found("alice", &json!({"q": q})).await);
        assert_eq!(ids, vec!["0190aaaa-0000-7000-8000-000000000001"], "{q}");
    }
    // and `OR` is a word too: neither message has one that starts with `or`
    assert!(result_ids(&h.found("alice", &json!({"q": "foo OR bar"})).await).is_empty());
}

#[tokio::test]
async fn cafe_finds_cafe_with_an_accent_and_coeur_finds_the_ligature() {
    let h = Harness::start().await;
    h.thread(ALICE, 1, "Caf\u{e9} menu", &["Le c\u{153}ur de la carte"])
        .await;
    for q in [
        "cafe",
        "CAF\u{c9}",
        "cafe\u{301}",
        "coeur",
        "C\u{152}UR",
        "c\u{153}ur",
    ] {
        let page = h.found("alice", &json!({"q": q})).await;
        assert_eq!(result_ids(&page).len(), 1, "{q}");
    }
}

#[tokio::test]
async fn emoji_and_surrogate_pairs_have_utf16_offsets() {
    let h = Harness::start().await;
    h.thread(
        ALICE,
        1,
        "x",
        &["\u{1f389}\u{1f389} party \u{1d49c}\u{1d4b7}cd done"],
    )
    .await;
    let page = h.found("alice", &json!({"q": "party ab"})).await;
    let hit = &page["results"][0]["hits"][0];
    let snippet = &hit["snippet"];
    let text = snippet["text"].as_str().unwrap();
    let units: Vec<u16> = text.encode_utf16().collect();
    let marked: Vec<String> = snippet["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let (s, e) = (
                usize::try_from(m["start"].as_u64().unwrap()).unwrap(),
                usize::try_from(m["end"].as_u64().unwrap()).unwrap(),
            );
            String::from_utf16(&units[s..e]).expect("never inside a surrogate pair")
        })
        .collect();
    assert_eq!(marked, ["party", "\u{1d49c}\u{1d4b7}cd"]);
    // bytes and code points would say otherwise
    assert_eq!(snippet["matches"][0]["start"], 5);
}

#[tokio::test]
async fn a_cursor_is_for_its_own_request() {
    let h = Harness::start().await;
    for n in 1..=3 {
        h.thread(ALICE, n, "Zebra", &["zebra"]).await;
    }
    let first = h.found("alice", &json!({"q": "zebra", "limit": 2})).await;
    let next = first["next"].as_str().unwrap().to_owned();
    let second = h
        .found("alice", &json!({"q": "zebra", "limit": 2, "cursor": next}))
        .await;
    assert_eq!(result_ids(&second).len(), 1);
    assert!(second.get("next").is_none());
    for body in [
        json!({"q": "other", "cursor": next}),
        json!({"q": "zebra", "archived": "exclude", "cursor": next}),
        json!({"q": "zebra", "branches": "include", "cursor": next}),
        json!({"q": "zebra", "cursor": "nope"}),
        json!({"q": "zebra", "cursor": "v1-9-x-y"}),
        json!({"q": "zebra", "cursor": "x".repeat(600)}),
        json!({"q": "zebra", "cursor": ""}),
    ] {
        let r = h.search("alice", &body).await;
        h.refused(r, 400, Some("bad_cursor")).await;
    }
}

/// A cursor is a position (rank, thread id) checked against the request's terms and filters: it carries no
/// authority. The same terms and filters accept it whoever sends it and however the query is spelled; a
/// position that is not valid, or not in the one form this server writes, is `bad_cursor`.
#[tokio::test]
async fn a_cursor_is_a_position_that_carries_no_authority() {
    let h = Harness::start().await;
    for n in 1..=3 {
        h.thread(ALICE, n, "Zebra", &["zebra"]).await;
    }
    h.thread(BOB, 4, "Zebra", &["zebra"]).await;
    let first = h.found("alice", &json!({"q": "zebra", "limit": 2})).await;
    let next = first["next"].as_str().unwrap().to_owned();
    let parts: Vec<&str> = next.splitn(3, '-').collect();
    assert_eq!(parts[0], "v1");
    // the same terms read from another spelling of the query, and another person: accepted, and only the
    // sender's own threads are in the answer
    let spelled = h
        .found(
            "alice",
            &json!({"q": "  ZEBRA!! ", "limit": 2, "cursor": next}),
        )
        .await;
    assert_eq!(result_ids(&spelled).len(), 1);
    let bobs = h
        .found("bob", &json!({"q": "zebra", "limit": 2, "cursor": next}))
        .await;
    assert!(result_ids(&bobs).iter().all(|id| id.ends_with("0004")));
    // a position that is not one
    let rest = parts[2];
    let (id_text, print) = rest.rsplit_once('-').unwrap();
    for tampered in [
        format!("v1-7-{rest}"),
        format!("v1-3-{rest}"),
        format!("v1-00-{rest}"),
        format!("v1-+0-{rest}"),
        format!("v1-0-{}-{print}", id_text.to_ascii_uppercase()),
        format!("v1-0-0190aaaa-0000-7000-8000-00000000000g-{print}"),
        format!("v1-0--{print}"),
    ] {
        let r = h
            .search(
                "alice",
                &json!({"q": "zebra", "limit": 2, "cursor": tampered}),
            )
            .await;
        h.refused(r, 400, Some("bad_cursor")).await;
    }
}

/// A store that cannot answer is 503 `search_unavailable` with `Retry-After`, and what was asked is in
/// no log line of that path (the log test of this crate holds the words).
#[tokio::test]
async fn a_store_that_cannot_answer_is_503_with_retry_after() {
    let h = Harness::start().await;
    h.thread(ALICE, 1, "Zebra", &["zebra"]).await;
    h.store().set_search_down(true);
    let r = h.search("alice", &json!({"q": "zebra"})).await;
    assert_eq!(r.headers["retry-after"], "5");
    assert_eq!(r.headers["cache-control"], "no-store");
    h.refused(r, 503, Some("search_unavailable")).await;
    h.store().set_search_down(false);
    assert_eq!(
        result_ids(&h.found("alice", &json!({"q": "zebra"})).await).len(),
        1
    );
}

/// At most eight searches run at once in a process, for everybody: the ninth is answered at once with 503
/// `search_unavailable` and `Retry-After: 1`, the eight finish, and the next search is served.
#[tokio::test]
async fn at_most_eight_searches_run_at_once_and_the_ninth_is_503() {
    let h = Harness::with(
        ApiConfig {
            search_limits: AuthorizeLimits {
                burst: 1000,
                refill: Duration::from_millis(1),
            },
            ..ApiConfig::default()
        },
        Policy::default(),
    )
    .await;
    h.thread(ALICE, 1, "Zebra", &["zebra"]).await;
    h.store().hold_searches();
    let mut running = Vec::new();
    for _ in 0..8 {
        let (client, url) = (h.client.clone(), format!("{}/api/search/threads", h.base));
        running.push(tokio::spawn(async move {
            client
                .post(url)
                .bearer_auth("alice")
                .header("Content-Type", "application/json")
                .body(r#"{"q":"zebra"}"#)
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }));
    }
    tokio::time::timeout(
        Duration::from_secs(20),
        h.store().wait_until_searches_held(8),
    )
    .await
    .expect("eight searches reach the store");
    let ninth = h.search("bob", &json!({"q": "zebra"})).await;
    assert_eq!(ninth.headers["retry-after"], "1");
    h.refused(ninth, 503, Some("search_unavailable")).await;
    h.store().release_searches();
    for task in running {
        assert_eq!(task.await.unwrap(), 200);
    }
    assert_eq!(
        result_ids(&h.found("bob", &json!({"q": "zebra"})).await).len(),
        0
    );
    assert_eq!(
        result_ids(&h.found("alice", &json!({"q": "zebra"})).await).len(),
        1
    );
}

#[tokio::test]
async fn a_search_is_rate_limited_with_retry_after() {
    let h = Harness::with(
        ApiConfig {
            search_limits: AuthorizeLimits {
                burst: 3,
                refill: Duration::from_secs(3600),
            },
            ..ApiConfig::default()
        },
        Policy::default(),
    )
    .await;
    for _ in 0..3 {
        h.found("alice", &json!({"q": "zebra"})).await;
    }
    let r = h.search("alice", &json!({"q": "zebra"})).await;
    assert!(
        r.headers["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    h.refused(r, 429, Some("rate_limited")).await;
    // a person's own bucket: bob has his
    h.found("bob", &json!({"q": "zebra"})).await;
}

#[tokio::test]
async fn without_thread_read_it_is_403_and_without_json_it_is_415() {
    let policy = Policy::new(
        [
            (Role::new("user"), RoleGrant::user()),
            (
                Role::new("no-read"),
                RoleGrant {
                    permissions: [Permission::AgentRead].into_iter().collect(),
                    ..RoleGrant::user()
                },
            ),
        ]
        .into(),
        None,
    );
    let h = Harness::with(ApiConfig::default(), policy.unwrap()).await;
    let r = h.search("nobody", &json!({"q": "zebra"})).await;
    h.refused(r, 403, Some("forbidden")).await;
    // who may search is decided before the request is read: a body that is no search is still 403
    let r = h
        .post_raw("nobody", Some("text/plain"), b"not json".to_vec())
        .await;
    h.refused(r, 403, Some("forbidden")).await;
    let r = h
        .post_raw("alice", Some("text/plain"), b"q=zebra".to_vec())
        .await;
    assert_eq!(r.status, 415);
    h.contract.problem(415, &r.json());
    let r = h
        .post_raw("alice", None, b"{\"q\":\"zebra\"}".to_vec())
        .await;
    assert_eq!(r.status, 415);
    let r = h
        .post_raw("alice", Some("application/json"), b"not json".to_vec())
        .await;
    h.refused(r, 400, Some("bad_request")).await;
    // no sign-in
    let r = h
        .client
        .post(format!("{}/api/search/threads", h.base))
        .header("Content-Type", "application/json")
        .body("{\"q\":\"zebra\"}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);
}

#[tokio::test]
async fn get_config_says_the_route_exists() {
    let h = Harness::start().await;
    let r = h
        .client
        .get(format!("{}/api/config", h.base))
        .bearer_auth("alice")
        .send()
        .await
        .unwrap();
    let config: Value = r.json().await.unwrap();
    h.contract.validate(
        &json!({"$ref": "#/components/schemas/PublicConfig"}),
        &config,
    );
    assert_eq!(
        config["search"],
        json!({"maxQueryLength": 200, "maxTerms": 8})
    );
    assert!(config["ui"].is_object());
}

/// Anything in a result that a client draws is text, never markup: a title that is HTML comes back as it is.
#[tokio::test]
async fn a_snippet_is_plain_text() {
    let h = Harness::start().await;
    h.thread(
        ALICE,
        1,
        "<script>alert(1)</script> zebra",
        &["<b>zebra</b>"],
    )
    .await;
    let page = h.found("alice", &json!({"q": "zebra"})).await;
    let text = page["results"][0]["hits"][0]["snippet"]["text"]
        .as_str()
        .unwrap();
    assert_eq!(text, "<script>alert(1)</script> zebra");
}
