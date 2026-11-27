//! The chat conformance kit over the HTTP home (`orch-surface-agui` over `orch-api`): the four
//! cases of the chat contract of `docs/api/agui.md`, over real HTTP.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::result_large_err)]

use std::sync::Arc;
use std::time::Duration;

use orch_agui_proto as agui;
use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, Dispatcher, DispatcherConfig};
use orch_auth_jwt::testkit::TestIdp;
use orch_auth_jwt::{JwtAuth, JwtConfig};
use orch_chat_kit::{
    ChatHome, Frame, HomeError, LimitRule, Sent, Setup, UserMessage, chat_conformance,
};
use orch_core::AgentId;
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{AgentEndpoint, PortSet, SystemClock};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const AUDIENCE: &str = "another-agentic";
const PERSON: &str = "kit@example.invalid";
const AGENT: &str = "helper";

/// A server of the resource API and the AG-UI surface, with a dispatcher over the scripted agent.
struct Server {
    base: String,
    token: String,
    _idp: TestIdp,
    task: JoinHandle<()>,
    dispatcher: JoinHandle<()>,
    shutdown: CancellationToken,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.dispatcher.abort();
        self.task.abort();
    }
}

async fn serve(min_client_version: Option<&str>) -> Option<Server> {
    let idp = TestIdp::start().await;
    let jwt = JwtConfig::new(idp.issuer(), [AUDIENCE]).without_system_proxy();
    let directory = AgentDirectory::new(vec![AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(AGENT),
            "https://helper.invalid/.well-known/agent-card.json",
            None,
        ),
        name: "Helper".to_owned(),
    }]);
    let app = Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: ScriptedAgent::new(),
                clock: SystemClock,
                ids: SeqIds::default(),
                model: orch_ports::NoModel,
                auth: JwtAuth::new(jwt).ok()?,
                registry: directory.fixed_registry(),
                user_tools: orch_ports::NoUserTools::off(),
            },
            directory,
            AppConfig {
                stream_poll: Duration::from_millis(50),
                ..AppConfig::default()
            },
        )
        .ok()?,
    );
    let shutdown = CancellationToken::new();
    let dispatcher = tokio::spawn(
        Dispatcher::new(
            Arc::clone(&app),
            DispatcherConfig {
                concurrency: 8,
                poll_interval: Duration::from_millis(50),
                heartbeat: Duration::from_millis(100),
                backoff_base: Duration::from_millis(20),
                backoff_max: Duration::from_millis(200),
                poll_min: Duration::from_millis(20),
                poll_max: Duration::from_millis(100),
                live_flush: Duration::from_millis(20),
                ..DispatcherConfig::default()
            },
            "chat-kit",
        )
        .run(shutdown.child_token()),
    );
    let cfg = ApiConfig {
        min_client_version: min_client_version.and_then(|version| {
            version
                .parse::<orch_api::client_version::ClientVersion>()
                .ok()
        }),
        ..ApiConfig::default()
    };
    let surfaces = vec![orch_surface_agui::routes(
        Arc::clone(&app),
        cfg.sse_keepalive,
        cfg.sse_lifetime,
    )];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.ok()?;
    let addr = listener.local_addr().ok()?;
    let router = orch_api::router_with_surfaces(Arc::clone(&app), cfg, surfaces);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let token = idp.token(AUDIENCE, PERSON);
    Some(Server {
        base: format!("http://{addr}"),
        token,
        _idp: idp,
        task,
        dispatcher,
        shutdown,
    })
}

/// The HTTP home: one server, the caller's token, and the version the client states.
struct HttpHome {
    server: Server,
    client: reqwest::Client,
    client_version: Option<String>,
}

async fn make(setup: Setup) -> Option<HttpHome> {
    let server = serve(setup.min_client_version).await?;
    Some(HttpHome {
        server,
        client: reqwest::Client::builder().no_proxy().build().ok()?,
        client_version: setup.client_version.map(str::to_owned),
    })
}

impl HttpHome {
    /// A request to the home, with the bearer and the version the client states.
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let request = self
            .client
            .request(method, format!("{}{path}", self.server.base))
            .bearer_auth(&self.server.token);
        match &self.client_version {
            Some(version) => request.header("X-Client-Version", version),
            None => request,
        }
    }

    /// Sends a request and turns a refusal into the kit's error.
    async fn answer(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, HomeError> {
        let response = request
            .send()
            .await
            .map_err(|error| HomeError::Other(error.to_string()))?;
        match response.status().as_u16() {
            200 => Ok(response),
            426 => Err(update_required(response).await),
            status => Err(HomeError::Other(format!(
                "HTTP {status}: {}",
                body_text(response).await
            ))),
        }
    }
}

/// The 426 problem as the kit's error, naming the minimum it carries.
async fn update_required(response: reqwest::Response) -> HomeError {
    let body: Value = response.json().await.unwrap_or(Value::Null);
    HomeError::UpdateRequired {
        min: body["minClientVersion"]
            .as_str()
            .unwrap_or("unknown")
            .to_owned(),
    }
}

/// A bounded preview of a refusal body, for the failure message.
async fn body_text(response: reqwest::Response) -> String {
    let text = response.text().await.unwrap_or_default();
    text.chars().take(512).collect()
}

/// Reads an SSE response to its end as AG-UI frames.
async fn collect_sse(mut response: reqwest::Response) -> Result<Vec<Frame>, HomeError> {
    let mut buffer = String::new();
    let mut frames = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| HomeError::Other(error.to_string()))?
    {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buffer.find("\n\n") {
            let block: String = buffer.drain(..end + 2).collect();
            let mut resume_id = None;
            let mut data = Vec::new();
            for line in block.lines() {
                if let Some(value) = line.strip_prefix("id:") {
                    resume_id = value.trim().parse::<i64>().ok();
                } else if let Some(value) = line.strip_prefix("data:") {
                    data.push(value.strip_prefix(' ').unwrap_or(value).to_owned());
                }
            }
            // An `id:` with no `data:` is the connect stream's head marker: a cursor, not an event.
            if data.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(&data.join("\n")).map_err(|error| {
                HomeError::Other(format!("an SSE data line that is not JSON: {error}"))
            })?;
            let event: agui::Event = serde_json::from_value(value).map_err(|error| {
                HomeError::Other(format!("an SSE frame that is not an AG-UI event: {error}"))
            })?;
            frames.push(Frame { resume_id, event });
        }
    }
    Ok(frames)
}

impl ChatHome for HttpHome {
    fn limit_rule(&self) -> LimitRule {
        // The orchestrator's HTTP API has no application-level limit. Its own over-quota refusal
        // is 403 `allowance_spent`, and `vymalo.limit_reached` (402) is the application's, per
        // docs/api/agui.md: a capability of this home, not a gap to fix here.
        LimitRule::NotApplicable(
            "the orchestrator's HTTP API has no application-level limit: its own over-quota \
             refusal is 403 allowance_spent, and the 402 vymalo.limit_reached is the \
             application's (BFF or embed Admission) per docs/api/agui.md",
        )
    }

    async fn send(
        &self,
        thread: Option<&str>,
        message_id: &str,
        text: &str,
    ) -> Result<Sent, HomeError> {
        let thread = thread.map_or_else(|| Uuid::now_v7().to_string(), str::to_owned);
        let run = Uuid::now_v7().to_string();
        let body = json!({
            "threadId": thread,
            "runId": run,
            "messages": [{"id": message_id, "role": "user", "content": text}],
        });
        let response = self
            .answer(
                self.request(reqwest::Method::POST, &format!("/agui/agents/{AGENT}"))
                    .header("Accept", "text/event-stream")
                    .json(&body),
            )
            .await?;
        Ok(Sent {
            thread,
            frames: collect_sse(response).await?,
        })
    }

    async fn replay(&self, thread: &str, after: i64) -> Result<Vec<Frame>, HomeError> {
        let response = self
            .answer(
                self.request(
                    reqwest::Method::GET,
                    &format!("/agui/threads/{thread}/connect?mode=run"),
                )
                .header("Accept", "text/event-stream")
                .header("Last-Event-ID", after.to_string()),
            )
            .await?;
        collect_sse(response).await
    }

    async fn user_messages(&self, thread: &str) -> Result<Vec<UserMessage>, HomeError> {
        let response = self
            .answer(
                self.request(
                    reqwest::Method::GET,
                    &format!("/api/threads/{thread}/export"),
                )
                .header("Accept", "application/json"),
            )
            .await?;
        let body: Value = response
            .json()
            .await
            .map_err(|error| HomeError::Other(format!("the export is not JSON: {error}")))?;
        let events = body["events"]
            .as_array()
            .ok_or_else(|| HomeError::Other("the export has no events".to_owned()))?;
        Ok(events
            .iter()
            .filter(|event| event["kind"] == "user_message")
            .filter_map(|event| {
                let text = event["data"]["text"].as_str()?.to_owned();
                let id = event["data"]["messageId"]
                    .as_str()
                    .map_or_else(|| format!("evt-{}", event["seq"]), str::to_owned);
                Some(UserMessage { id, text })
            })
            .collect())
    }
}

chat_conformance!(make);
