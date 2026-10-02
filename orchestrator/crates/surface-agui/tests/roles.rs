//! The AG-UI surface under roles (ADR 0033): an administrator follows any thread and runs only
//! their own, a role that names some agents runs only those, and a person whose roles grant
//! nothing is refused before any stream.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, AgentScope, App, AppConfig, Dispatcher, Permission, Policy,
    RoleGrant,
};
use orch_core::AgentId;
use orch_ports::memory::{MemoryAuth, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{
    AgentEndpoint, FixedRegistry, NoArtifacts, NoModel, PortSet, Principal, Role, SystemClock,
};
use serde_json::Value;
use support::{Stream, fast_dispatcher, input, new_thread_id, resp_of};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    MemoryAuth,
    NoArtifacts,
>;

struct Rig {
    base: String,
    client: reqwest::Client,
    app: Arc<App<Stack>>,
    agent: ScriptedAgent,
    server: JoinHandle<()>,
    dispatcher: JoinHandle<()>,
    token: CancellationToken,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.token.cancel();
        self.server.abort();
        self.dispatcher.abort();
    }
}

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(orch_core::UserId::new(email))
    }
}

impl Rig {
    async fn start(policy: Policy) -> Self {
        let entry = |id: &str| AgentEntry {
            endpoint: AgentEndpoint::a2a(
                AgentId::new(id),
                format!("https://{id}.example.com/.well-known/agent-card.json"),
                None,
            ),
            name: id.to_owned(),
        };
        let directory = AgentDirectory::new(vec![entry("coder"), entry("plain")]);
        let auth = MemoryAuth::new();
        // A token that ran out a day ago, and one that runs out in a day.
        let now = orch_core::Timestamp::now();
        let expiring = |email: &str, seconds: i64| Principal {
            expires_at: Some(orch_core::Timestamp::from_second(now.as_second() + seconds).unwrap()),
            ..principal(email, &["user"])
        };
        auth.allow("expired", expiring("alice@example.com", -86_400));
        auth.allow("for-a-day", expiring("alice@example.com", 86_400));
        for (token, email, role) in [
            ("alice", "alice@example.com", "user"),
            ("bob", "bob@example.com", "user"),
            ("root", "root@example.com", "admin"),
            ("chatter", "chatter@example.com", "chat"),
            ("stranger", "stranger@example.com", "wizard"),
        ] {
            auth.allow(token, principal(email, &[role]));
        }
        let agent = ScriptedAgent::new();
        let app = Arc::new(
            App::new(
                PortSet {
                    artifacts: NoArtifacts,
                    store: MemoryStore::new(),
                    wakeup: MemoryWakeup::new(),
                    agents: agent.clone(),
                    clock: SystemClock,
                    ids: SeqIds::default(),
                    model: NoModel,
                    auth,
                    registry: directory.fixed_registry(),
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
        let token = CancellationToken::new();
        let dispatcher = tokio::spawn(
            Dispatcher::new(Arc::clone(&app), fast_dispatcher(), "test-dispatcher")
                .run(token.clone()),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let api = ApiConfig {
            sse_keepalive: Duration::from_millis(150),
            ..ApiConfig::default()
        };
        let keepalive = api.sse_keepalive;
        let router = orch_api::router_with_surfaces(
            Arc::clone(&app),
            api,
            vec![orch_surface_agui::routes(Arc::clone(&app), keepalive)],
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Rig {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            app,
            agent,
            server,
            dispatcher,
            token,
        }
    }

    async fn run(&self, agent: &str, token: &str, body: &Value) -> reqwest::Response {
        self.client
            .post(format!("{}/agui/agents/{agent}", self.base))
            .bearer_auth(token)
            .header("Accept", "text/event-stream")
            .json(body)
            .send()
            .await
            .unwrap()
    }

    async fn connect(&self, thread: &str, token: &str) -> reqwest::Response {
        self.client
            .get(format!(
                "{}/agui/threads/{thread}/connect?mode=run",
                self.base
            ))
            .bearer_auth(token)
            .header("Accept", "text/event-stream")
            .send()
            .await
            .unwrap()
    }

    /// A thread of Alice's that has run to its end.
    async fn finished_thread_of_alice(&self) -> String {
        let thread = new_thread_id();
        let resp = self
            .run(
                "plain",
                "alice",
                &input(&thread, "r1", &[("m1", "echo hi")]),
            )
            .await;
        assert_eq!(resp.status().as_u16(), 200);
        let frames = Stream::new(resp).all().await;
        assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
        thread
    }
}

fn chat_policy() -> Policy {
    let chat = RoleGrant {
        permissions: BTreeSet::from([
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
            Permission::ThreadWrite,
        ]),
        agents: AgentScope::from_patterns(["plain"]),
        ..RoleGrant::user()
    };
    let mut roles = orch_app::built_in_roles();
    roles.insert(Role::new("chat"), chat);
    Policy::new(roles, None).unwrap()
}

#[tokio::test]
async fn an_administrator_follows_any_thread_and_a_user_follows_only_their_own() {
    let rig = Rig::start(chat_policy()).await;
    let thread = rig.finished_thread_of_alice().await;

    // Alice and the administrator replay it; Bob is told it does not exist.
    for token in ["alice", "root"] {
        let resp = rig.connect(&thread, token).await;
        assert_eq!(resp.status().as_u16(), 200, "{token}");
        let frames = Stream::new(resp).all().await;
        assert!(
            frames.iter().any(|f| f.kind() == "RUN_FINISHED"),
            "{token}: {frames:?}"
        );
    }
    let r = resp_of(rig.connect(&thread, "bob").await).await;
    r.problem(404);
    // The same answer as for a thread nobody has.
    resp_of(rig.connect(&new_thread_id(), "bob").await)
        .await
        .problem(404);
}

#[tokio::test]
async fn an_administrator_runs_only_their_own_threads() {
    let rig = Rig::start(chat_policy()).await;
    let thread = rig.finished_thread_of_alice().await;
    let sends = rig.agent.sends().len();
    let alice = principal("alice@example.com", &["user"]);
    let events = rig
        .app
        .list_events(&alice, thread.parse().unwrap(), 0, 100)
        .await
        .unwrap()
        .len();

    // A run that continues Alice's thread is a write: read-only for the administrator, 404 for Bob.
    let body = input(&thread, "r2", &[("m2", "echo more")]);
    let r = resp_of(rig.run("plain", "root", &body).await).await;
    let problem = r.problem(403);
    assert_eq!(problem["code"], "read_only");
    resp_of(rig.run("plain", "bob", &body).await)
        .await
        .problem(404);
    // Nothing was sent to the agent, and nothing was written to the thread.
    assert_eq!(rig.agent.sends().len(), sends);
    assert_eq!(
        rig.app
            .list_events(&alice, thread.parse().unwrap(), 0, 100)
            .await
            .unwrap()
            .len(),
        events
    );

    // Their own thread is theirs.
    let own = new_thread_id();
    let resp = rig
        .run("plain", "root", &input(&own, "r3", &[("m3", "echo mine")]))
        .await;
    assert_eq!(resp.status().as_u16(), 200);
    let frames = Stream::new(resp).all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
}

#[tokio::test]
async fn a_role_runs_only_the_agents_it_names() {
    let rig = Rig::start(chat_policy()).await;
    // Alice's role names every agent, the chatter's names `plain`.
    let thread = new_thread_id();
    let r = resp_of(
        rig.run(
            "coder",
            "chatter",
            &input(&thread, "r1", &[("m1", "echo hi")]),
        )
        .await,
    )
    .await;
    assert_eq!(r.problem(403)["code"], "forbidden");
    // Refused before anything was created.
    assert!(
        rig.app
            .thread_for_tools(thread.parse().unwrap())
            .await
            .unwrap()
            .is_none()
    );
    let resp = rig
        .run(
            "plain",
            "chatter",
            &input(&thread, "r2", &[("m2", "echo hi")]),
        )
        .await;
    assert_eq!(resp.status().as_u16(), 200);
    Stream::new(resp).all().await;

    // The capabilities of an agent are read with agent.read for it.
    for (token, agent, status) in [
        ("chatter", "plain", 200),
        ("chatter", "coder", 403),
        ("alice", "coder", 200),
    ] {
        let resp = rig
            .client
            .get(format!("{}/agui/agents/{agent}/capabilities", rig.base))
            .bearer_auth(token)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), status, "{token} {agent}");
    }
}

#[tokio::test]
async fn a_person_whose_roles_grant_nothing_is_refused_before_any_stream() {
    // No default role: `stranger`'s role is not defined, so nothing is granted.
    let rig = Rig::start(chat_policy()).await;
    let thread = new_thread_id();
    let r = resp_of(
        rig.run(
            "plain",
            "stranger",
            &input(&thread, "r1", &[("m1", "echo hi")]),
        )
        .await,
    )
    .await;
    assert_eq!(r.problem(403)["code"], "no_access");
    let r = resp_of(rig.connect(&thread, "stranger").await).await;
    assert_eq!(r.problem(403)["code"], "no_access");
    // The default role makes the same person a user.
    let mut roles = orch_app::built_in_roles();
    roles.insert(Role::new("chat"), RoleGrant::none());
    let rig = Rig::start(Policy::new(roles, Some(Role::new("user"))).unwrap()).await;
    let resp = rig
        .run(
            "plain",
            "stranger",
            &input(&thread, "r1", &[("m1", "echo hi")]),
        )
        .await;
    assert_eq!(resp.status().as_u16(), 200);
    Stream::new(resp).all().await;
}

#[tokio::test]
async fn a_stream_lasts_as_long_as_the_token_it_was_opened_with() {
    let rig = Rig::start(chat_policy()).await;
    let thread = rig.finished_thread_of_alice().await;
    let url = format!("{}/agui/threads/{thread}/connect", rig.base);
    let open = |token: &'static str| {
        let (client, url) = (rig.client.clone(), url.clone());
        async move {
            Stream::new(
                client
                    .get(url)
                    .bearer_auth(token)
                    .header("Accept", "text/event-stream")
                    .send()
                    .await
                    .unwrap(),
            )
        }
    };
    // A token that ran out beyond the leeway gets a stream that ends at once, with no event: the
    // client reconnects with a fresh one.
    let mut expired = open("expired").await;
    assert_eq!(expired.status, 200);
    let first = expired.next(Duration::from_secs(2)).await;
    assert!(first.is_none(), "{first:?}");
    assert!(expired.ended(), "the stream of an expired token is over");
    // A token for a day gets the whole thread, and the stream stays open after it.
    let mut live = open("for-a-day").await;
    let frames = live.until(|f| f.kind() == "RUN_FINISHED").await;
    assert_eq!(frames.first().unwrap().kind(), "RUN_STARTED");
    assert!(live.is_quiet_for(Duration::from_millis(400)).await);
    // So does a credential that does not run out (the same token without an expiry).
    let mut forever = open("alice").await;
    forever.until(|f| f.kind() == "RUN_FINISHED").await;
    assert!(forever.is_quiet_for(Duration::from_millis(400)).await);
}
