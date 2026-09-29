//! [`A2aAgentClient`]: the A2A implementation of the [`AgentClient`] port.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use a2a::{
    A2AError, AgentCard, CancelTaskRequest, GetTaskRequest, ListTasksRequest, Message, Part, Role,
    SVC_PARAM_EXTENSIONS, SendMessageRequest, SubscribeToTaskRequest, TRANSPORT_PROTOCOL_HTTP_JSON,
    TRANSPORT_PROTOCOL_JSONRPC,
};
use a2a_client::auth::AuthInterceptor;
use a2a_client::jsonrpc::JsonRpcTransportFactory;
use a2a_client::middleware::CallInterceptor;
use a2a_client::rest::RestTransportFactory;
use a2a_client::{A2AClient, A2AClientFactory, ServiceParams, Transport};
use futures::StreamExt;
use futures::stream::BoxStream;
use orch_ports::{
    AgentCardInfo, AgentClient, AgentEndpoint, AgentError, AgentStream, SendRequest, TaskHandle,
    TaskSnapshot,
};
use serde_json::json;

use crate::errors::classify;
use crate::mapping::{StreamMapper, snapshot};
use crate::releases::{RELEASE_CHANNELS_URI, releases_from_card};

/// Tunables of the A2A client. The defaults suit production.
#[derive(Debug, Clone)]
pub struct A2aConfig {
    /// Whole-request timeout for fetching an agent card.
    pub card_timeout: Duration,
    /// Timeout for establishing a connection.
    pub connect_timeout: Duration,
    /// Longest silence tolerated on an open response, which covers streams: A2A servers send
    /// keepalive comments every 15 s, so a stream quieter than this is dead. A total timeout
    /// would kill legitimately long tasks and is deliberately not used.
    pub read_timeout: Duration,
    /// Timeout of unary calls (`GetTask`, `CancelTask`, `ListTasks`) and of opening a stream.
    pub call_timeout: Duration,
    /// Honour `HTTP(S)_PROXY`/`NO_PROXY` from the environment (tests turn this off).
    pub use_system_proxy: bool,
}

impl Default for A2aConfig {
    fn default() -> Self {
        A2aConfig {
            card_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(90),
            call_timeout: Duration::from_secs(30),
            use_system_proxy: true,
        }
    }
}

/// The HTTP client could not be built (TLS backend initialisation).
#[derive(Debug, thiserror::Error)]
#[error("cannot build the A2A HTTP client: {0}")]
pub struct BuildError(String);

/// Installs the process-wide `rustls` crypto provider (`aws-lc-rs`, the one the A2A SDK's
/// `reqwest` uses). Idempotent; an already installed provider is left alone.
pub fn install_crypto_provider() {
    // Err means a provider is already installed, which is what we want.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// Adds the `A2A-Extensions` header that activates the release-channels extension.
struct ActivateReleaseChannels;

#[async_trait::async_trait]
impl CallInterceptor for ActivateReleaseChannels {
    async fn before(&self, _method: &str, params: &mut ServiceParams) -> Result<(), A2AError> {
        params.insert(
            SVC_PARAM_EXTENSIONS.to_owned(),
            vec![RELEASE_CHANNELS_URI.to_owned()],
        );
        Ok(())
    }
}

/// Talks to A2A 1.0 agents (`SendStreamingMessage`, `SubscribeToTask`, `GetTask`,
/// `CancelTask`, `ListTasks`) over JSON-RPC or HTTP+JSON.
///
/// The client is stateless: every operation reads the agent's card live (ADR 0008: release
/// data is read fresh and never cached), so a moved endpoint or a changed release list is
/// noticed on the next call. Only the underlying HTTP connection pools are reused.
#[derive(Clone)]
pub struct A2aAgentClient {
    card_http: reqwest::Client,
    rpc_http: reqwest::Client,
    cfg: A2aConfig,
}

impl std::fmt::Debug for A2aAgentClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("A2aAgentClient")
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

impl A2aAgentClient {
    /// Builds the client and installs the `rustls` crypto provider if none is installed yet.
    pub fn new(cfg: A2aConfig) -> Result<Self, BuildError> {
        install_crypto_provider();
        let base = |builder: reqwest::ClientBuilder| {
            if cfg.use_system_proxy {
                builder
            } else {
                builder.no_proxy()
            }
        };
        let card_http = base(reqwest::Client::builder())
            .timeout(cfg.card_timeout)
            .connect_timeout(cfg.connect_timeout)
            .build()
            .map_err(|e| BuildError(e.to_string()))?;
        let rpc_http = base(reqwest::Client::builder())
            .connect_timeout(cfg.connect_timeout)
            .read_timeout(cfg.read_timeout)
            .build()
            .map_err(|e| BuildError(e.to_string()))?;
        Ok(A2aAgentClient {
            card_http,
            rpc_http,
            cfg,
        })
    }

    /// The card document URL: a URL ending in `.json` is used as is, anything else is treated
    /// as the agent's base URL.
    fn card_url(ep: &AgentEndpoint) -> String {
        let url = ep.card_url.trim();
        if url.trim_end_matches('/').ends_with(".json") {
            url.to_owned()
        } else {
            format!("{}/.well-known/agent-card.json", url.trim_end_matches('/'))
        }
    }

    async fn fetch_card(&self, ep: &AgentEndpoint) -> Result<AgentCard, AgentError> {
        let mut req = self.card_http.get(Self::card_url(ep));
        if let Some(token) = &ep.bearer {
            req = req.bearer_auth(token);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| AgentError::Unreachable(format!("failed to fetch agent card: {e}")))?;
        let status = resp.status();
        if status.is_server_error() {
            return Err(AgentError::Unreachable(format!(
                "agent card fetch returned HTTP {status}"
            )));
        }
        if !status.is_success() {
            return Err(AgentError::Rejected(format!(
                "agent card fetch returned HTTP {status}"
            )));
        }
        let body = resp
            .bytes()
            .await
            .map_err(|e| AgentError::Unreachable(format!("failed to read agent card: {e}")))?;
        serde_json::from_slice::<AgentCard>(&body).map_err(|e| {
            AgentError::Protocol(format!("the agent card is not a valid A2A 1.0 card: {e}"))
        })
    }

    async fn client_for(
        &self,
        ep: &AgentEndpoint,
        card: &AgentCard,
        activate_releases: bool,
    ) -> Result<A2AClient<Box<dyn Transport>>, AgentError> {
        let mut builder = A2AClientFactory::builder()
            .no_defaults()
            .preferred_bindings(vec![
                TRANSPORT_PROTOCOL_JSONRPC.to_owned(),
                TRANSPORT_PROTOCOL_HTTP_JSON.to_owned(),
            ])
            .register(Arc::new(JsonRpcTransportFactory::new(Some(
                self.rpc_http.clone(),
            ))))
            .register(Arc::new(RestTransportFactory::new(Some(
                self.rpc_http.clone(),
            ))));
        if let Some(token) = &ep.bearer {
            builder = builder.with_interceptor(Arc::new(AuthInterceptor::bearer(token.clone())));
        }
        if activate_releases {
            builder = builder.with_interceptor(Arc::new(ActivateReleaseChannels));
        }
        builder
            .build()
            .create_from_card(card)
            .await
            .map_err(classify)
    }

    /// Fetches the live card and builds a client for it.
    async fn connect(
        &self,
        ep: &AgentEndpoint,
    ) -> Result<A2AClient<Box<dyn Transport>>, AgentError> {
        let card = self.fetch_card(ep).await?;
        self.client_for(ep, &card, false).await
    }

    /// Bounds a call that has no protocol-level timeout of its own.
    async fn timed<T>(
        &self,
        what: &str,
        call: impl Future<Output = Result<T, A2AError>>,
    ) -> Result<T, AgentError> {
        match tokio::time::timeout(self.cfg.call_timeout, call).await {
            Ok(result) => result.map_err(classify),
            Err(_) => Err(AgentError::Unreachable(format!(
                "{what} timed out after {:?}",
                self.cfg.call_timeout
            ))),
        }
    }
}

type Item = Result<orch_ports::AgentEnvelope, AgentError>;

struct Mapping {
    inner: BoxStream<'static, Result<a2a::StreamResponse, A2AError>>,
    mapper: StreamMapper,
    queue: VecDeque<Item>,
    seen_any: bool,
    ended: bool,
}

/// Maps the SDK's stream. A response that ends without a single event is reported as an
/// error: the SDK treats a body-less answer to a streaming call as an empty stream, which is
/// what a refused request (for example a proxy's 401, whose status the SDK drops) looks like.
fn map_stream(inner: BoxStream<'static, Result<a2a::StreamResponse, A2AError>>) -> AgentStream {
    let state = Mapping {
        inner,
        mapper: StreamMapper::default(),
        queue: VecDeque::new(),
        seen_any: false,
        ended: false,
    };
    futures::stream::unfold(state, |mut st| async move {
        loop {
            if let Some(item) = st.queue.pop_front() {
                return Some((item, st));
            }
            if st.ended {
                return None;
            }
            match st.inner.next().await {
                Some(Ok(response)) => {
                    st.seen_any = true;
                    let mapped = st.mapper.map(response);
                    st.queue.extend(mapped);
                }
                Some(Err(e)) => {
                    st.seen_any = true;
                    st.queue.push_back(Err(classify(e)));
                }
                None => {
                    st.ended = true;
                    if !st.seen_any {
                        st.queue.push_back(Err(AgentError::Protocol(
                            "the agent closed the stream without sending an event; check its \
                             authentication and availability"
                                .to_owned(),
                        )));
                    }
                }
            }
        }
    })
    .boxed()
}

fn user_message(req: &SendRequest) -> Message {
    let mut message = Message::new(Role::User, vec![Part::text(req.text.clone())]);
    message.message_id = req.message_id.clone();
    message.context_id = Some(req.context_id.clone());
    message.task_id = req.task_id.clone();
    if let Some(release) = &req.release {
        message.metadata = Some(HashMap::from([(
            RELEASE_CHANNELS_URI.to_owned(),
            json!({ "release": release }),
        )]));
        message.extensions = Some(vec![RELEASE_CHANNELS_URI.to_owned()]);
    }
    message
}

impl AgentClient for A2aAgentClient {
    async fn read_card(&self, ep: &AgentEndpoint) -> Result<AgentCardInfo, AgentError> {
        let card = self.fetch_card(ep).await?;
        Ok(AgentCardInfo {
            description: Some(card.description.clone()).filter(|d| !d.trim().is_empty()),
            releases: releases_from_card(&card),
        })
    }

    async fn send_stream(&self, req: SendRequest) -> Result<AgentStream, AgentError> {
        let card = self.fetch_card(&req.endpoint).await?;
        if req.release.is_some() && releases_from_card(&card).is_none() {
            // The card is read live for this very call: if the extension is gone, sending the
            // message would silently run the default release. Fail closed instead.
            return Err(AgentError::Rejected(
                "the agent no longer offers release channels; refusing to run a default release"
                    .to_owned(),
            ));
        }
        let client = self
            .client_for(&req.endpoint, &card, req.release.is_some())
            .await?;
        let request = SendMessageRequest {
            message: user_message(&req),
            configuration: None,
            metadata: None,
            tenant: None,
        };
        let stream = self
            .timed(
                "opening the message stream",
                client.send_streaming_message(&request),
            )
            .await?;
        Ok(map_stream(stream))
    }

    async fn resubscribe(&self, task: &TaskHandle) -> Result<AgentStream, AgentError> {
        let client = self.connect(&task.endpoint).await?;
        let request = SubscribeToTaskRequest {
            id: task.task_id.clone(),
            tenant: None,
        };
        let stream = self
            .timed(
                "subscribing to the task",
                client.subscribe_to_task(&request),
            )
            .await?;
        Ok(map_stream(stream))
    }

    async fn get_task(&self, task: &TaskHandle) -> Result<TaskSnapshot, AgentError> {
        let client = self.connect(&task.endpoint).await?;
        let request = GetTaskRequest {
            id: task.task_id.clone(),
            history_length: Some(0),
            tenant: None,
        };
        let found = self
            .timed("reading the task", client.get_task(&request))
            .await?;
        snapshot(&found)
    }

    async fn cancel(&self, task: &TaskHandle) -> Result<TaskSnapshot, AgentError> {
        let client = self.connect(&task.endpoint).await?;
        let request = CancelTaskRequest {
            id: task.task_id.clone(),
            metadata: None,
            tenant: None,
        };
        let canceled = self
            .timed("cancelling the task", client.cancel_task(&request))
            .await?;
        snapshot(&canceled)
    }

    /// A2A 1.0 has no lookup by message id, so this lists the tasks of the context (`ListTasks`)
    /// and finds the one whose history holds the message. Limits: a follow-up message to an
    /// existing task is only found if the agent records it in the task history (the SDK's
    /// default handler does not), and an agent without `ListTasks` yields `Ok(None)`.
    async fn find_task_by_message(
        &self,
        ep: &AgentEndpoint,
        context_id: &str,
        message_id: &str,
    ) -> Result<Option<String>, AgentError> {
        const MAX_PAGES: usize = 10;
        let client = self.connect(ep).await?;
        let mut page_token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let request = ListTasksRequest {
                context_id: Some(context_id.to_owned()),
                status: None,
                page_size: Some(50),
                page_token: page_token.take(),
                history_length: Some(100),
                status_timestamp_after: None,
                include_artifacts: Some(false),
                tenant: None,
            };
            let page = match self
                .timed("listing tasks", client.list_tasks(&request))
                .await
            {
                Ok(page) => page,
                Err(AgentError::Unsupported(_)) => return Ok(None),
                Err(e) => return Err(e),
            };
            let hit = page.tasks.iter().find(|t| {
                t.history
                    .iter()
                    .flatten()
                    .any(|m| m.message_id == message_id)
            });
            if let Some(task) = hit {
                return Ok(Some(task.id.clone()));
            }
            if page.next_page_token.is_empty() {
                return Ok(None);
            }
            page_token = Some(page.next_page_token);
        }
        Ok(None)
    }
}
