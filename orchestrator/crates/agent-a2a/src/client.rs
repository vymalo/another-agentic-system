//! [`A2aAgentClient`]: the A2A implementation of the [`AgentClient`] port.

use std::collections::{BTreeSet, HashMap, VecDeque};
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
use orch_a2a_mapping::{StreamMapper, sessions_news, snapshot, usage_totals, wants_task_read};
use orch_core::{
    ACCESS_TIER_EXTENSION, BoxError, CHILD_SESSIONS_EXTENSION, KnownExtension, MENTIONS_EXTENSION,
    STEER_EXTENSION, STEPS_EXTENSION, TEXT_STREAM_EXTENSION, THREAD_TOOLS_EXTENSION,
    TRUSTED_COMPONENTS_EXTENSION, TrustedComponentsOffer, UI_CATALOG_EXTENSION, UPLOADS_EXTENSION,
    USAGE_EXTENSION, UiDelivery, UiVersion, history_preamble,
};
use orch_ports::{
    AgentCardInfo, AgentClient, AgentEndpoint, AgentError, AgentStream, AgentTransport,
    SendContent, SendRequest, TaskHandle, TaskSnapshot, UiSupport,
};
use orch_thread_token::{ThreadToolsGrant, ThreadToolsIssuer};
use serde_json::{Value, json};

use crate::a2ui::{
    action_part, inline_catalog, renderer_capabilities, ui_catalog_metadata, ui_from_card,
};
use crate::access_tier::{access_tier_from_card, check_plain_under_cap, tier_metadata};
use crate::child_sessions::{child_sessions_from_card, session_metadata};
use crate::commands::commands_from_card;
use crate::errors::classify;
use crate::extensions::{build_from_card, extensions_from_card};
use crate::files::{Fetcher, FileFetch};
use crate::mentions::mentions_metadata;
use crate::releases::{RELEASE_CHANNELS_URI, releases_from_card};
use crate::thread_tools::{mint, thread_tools_metadata};
use crate::trusted::trusted_components_from_card;
use crate::uploads::uploads_metadata;

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
    /// What mints the thread-tools grants (`thread-tools/v1`, ADR 0023): the keys, the URL agents
    /// reach the orchestrator at and the token lifetime. With it, a message to an agent whose live
    /// card lists the extension carries `{url, token, expiresAt}` for the request's thread; without
    /// it (the default) no agent is given a grant. `Debug` shows no key.
    pub thread_tools: Option<Arc<ThreadToolsIssuer>>,
    /// Whether the thread endpoint the grants open offers `ask_agent` (ADR 0026). Off (the
    /// default) a message that mentions agents to an agent that lists `mentions/v1` carries the
    /// references and **no `coordinate` member**, because the agent has no tool to ask them with;
    /// on, `coordinate: {"tool": "ask_agent"}` is added when the card also lists `thread-tools/v1`
    /// and a grant was minted. The composition root turns it on when it mounts the tool.
    pub asks: bool,
    /// The hosts a `url` part of an artifact may be fetched from, and the size it may have
    /// (ADR 0032, `artifacts.fetchHosts`). Without it (the default) every `url` part stays a link.
    pub fetch_files: Option<FileFetch>,
    /// How often a stream that activated `child-sessions/v1` reads its task for the sessions it
    /// lists (ADR 0072): the poll that learns a session from an agent that sends no events. Not in
    /// the configuration file.
    pub child_sessions_poll: Duration,
}

impl Default for A2aConfig {
    fn default() -> Self {
        A2aConfig {
            card_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(90),
            call_timeout: Duration::from_secs(30),
            use_system_proxy: true,
            thread_tools: None,
            asks: false,
            fetch_files: None,
            child_sessions_poll: Duration::from_secs(5),
        }
    }
}

/// The HTTP client could not be built (TLS backend initialisation).
#[derive(Debug, thiserror::Error)]
#[error("cannot build the A2A HTTP client")]
pub struct BuildError(#[source] BoxError);

/// Installs the process-wide `rustls` crypto provider (`aws-lc-rs`, the one the A2A SDK's
/// `reqwest` uses). Idempotent; an already installed provider is left alone.
pub fn install_crypto_provider() {
    // Err means a provider is already installed, which is what we want.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

/// The longest `Retry-After` honoured: a peer cannot park a delivery for longer.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(3600);

/// `Retry-After` as delta-seconds or an HTTP date, capped at [`MAX_RETRY_AFTER`].
fn retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    let wait = match value.parse::<u64>() {
        Ok(secs) => Duration::from_secs(secs),
        Err(_) => {
            let at = jiff::fmt::rfc2822::parse(value).ok()?;
            let ahead = at.timestamp().duration_since(jiff::Timestamp::now());
            Duration::try_from(ahead).unwrap_or(Duration::ZERO)
        }
    };
    Some(wait.min(MAX_RETRY_AFTER))
}

/// What an unsuccessful answer to the card request means (fixes a 429 or 408 being a permanent
/// rejection): 401/403 need other credentials, 429 asks to slow down, 408 and 5xx are the
/// agent's (or its proxy's) trouble, other statuses refuse the request for good.
fn card_status_error(
    status: reqwest::StatusCode,
    headers: &reqwest::header::HeaderMap,
) -> AgentError {
    use reqwest::StatusCode as S;
    let detail = format!("the agent card request answered HTTP {status}");
    match status {
        S::UNAUTHORIZED | S::FORBIDDEN => AgentError::unauthenticated(detail),
        S::TOO_MANY_REQUESTS => AgentError::RateLimited {
            retry_after: retry_after(headers),
        },
        S::REQUEST_TIMEOUT => AgentError::unreachable(detail),
        s if s.is_server_error() => AgentError::unreachable(detail),
        _ => AgentError::Rejected(detail),
    }
}

/// The card URL and bearer of an A2A endpoint. This adapter serves the `A2a` transport only: any
/// other is answered with `Unsupported` (the one place that decides it), never dereferenced.
fn a2a_parts(ep: &AgentEndpoint) -> Result<(&str, Option<&str>), AgentError> {
    match &ep.transport {
        AgentTransport::A2a { card_url, bearer } => Ok((card_url, bearer.as_deref())),
        AgentTransport::Local { .. } => Err(AgentError::Unsupported(format!(
            "agent {} is hosted in-process; the A2A client only serves A2A endpoints",
            ep.id
        ))),
    }
}

/// Adds the `A2A-Extensions` header that activates the extensions this call uses (release
/// channels, A2UI). Only extensions the live card offers are ever listed.
struct ActivateExtensions(Vec<String>);

#[async_trait::async_trait]
impl CallInterceptor for ActivateExtensions {
    async fn before(&self, _method: &str, params: &mut ServiceParams) -> Result<(), A2AError> {
        params.insert(SVC_PARAM_EXTENSIONS.to_owned(), self.0.clone());
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
    fetcher: Option<Fetcher>,
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
            .map_err(|e| BuildError(e.without_url().into()))?;
        let rpc_http = base(reqwest::Client::builder())
            .connect_timeout(cfg.connect_timeout)
            .read_timeout(cfg.read_timeout)
            .build()
            .map_err(|e| BuildError(e.without_url().into()))?;
        let fetcher = cfg
            .fetch_files
            .clone()
            .map(|policy| Fetcher::new(policy, cfg.use_system_proxy))
            .transpose()
            .map_err(|e| BuildError(e.without_url().into()))?;
        Ok(A2aAgentClient {
            card_http,
            rpc_http,
            fetcher,
            cfg,
        })
    }

    /// The card document URL: a URL ending in `.json` is used as is, anything else is treated
    /// as the agent's base URL.
    fn card_url(ep: &AgentEndpoint) -> Result<String, AgentError> {
        let (card_url, _) = a2a_parts(ep)?;
        let url = card_url.trim();
        Ok(if url.trim_end_matches('/').ends_with(".json") {
            url.to_owned()
        } else {
            format!("{}/.well-known/agent-card.json", url.trim_end_matches('/'))
        })
    }

    async fn fetch_card(&self, ep: &AgentEndpoint) -> Result<AgentCard, AgentError> {
        let (_, bearer) = a2a_parts(ep)?;
        let mut req = self.card_http.get(Self::card_url(ep)?);
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await.map_err(|e| {
            AgentError::unreachable("the agent card could not be fetched")
                .with_source(e.without_url())
        })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(card_status_error(status, resp.headers()));
        }
        let body = resp.bytes().await.map_err(|e| {
            AgentError::unreachable("the agent card could not be read").with_source(e.without_url())
        })?;
        serde_json::from_slice::<AgentCard>(&body).map_err(|e| {
            AgentError::protocol("the agent card is not a valid A2A 1.0 card").with_source(e)
        })
    }

    async fn client_for(
        &self,
        ep: &AgentEndpoint,
        card: &AgentCard,
        activate: Vec<String>,
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
        let (_, bearer) = a2a_parts(ep)?;
        if let Some(token) = bearer {
            builder = builder.with_interceptor(Arc::new(AuthInterceptor::bearer(token.to_owned())));
        }
        if !activate.is_empty() {
            builder = builder.with_interceptor(Arc::new(ActivateExtensions(activate)));
        }
        builder
            .build()
            .create_from_card(card)
            .await
            .map_err(classify)
    }

    /// Fetches the live card and builds a client for a call about a task (`GetTask`, `CancelTask`,
    /// `ListTasks`), and says whether it activated `child-sessions/v1`. The card read for this very
    /// call decides: activated, the task says its sessions and a round of a child context can be
    /// read at all (ADR 0072); not, nothing the agent says of sessions is believed.
    async fn connect_for_task(
        &self,
        ep: &AgentEndpoint,
    ) -> Result<(A2AClient<Box<dyn Transport>>, bool), AgentError> {
        let card = self.fetch_card(ep).await?;
        let activate = child_sessions_from_card(&card)
            .map(|_| vec![CHILD_SESSIONS_EXTENSION.to_owned()])
            .unwrap_or_default();
        let activated = !activate.is_empty();
        Ok((self.client_for(ep, &card, activate).await?, activated))
    }

    /// The snapshot with the links on allowed hosts turned into the files they name.
    async fn files_of(&self, snap: TaskSnapshot) -> Result<TaskSnapshot, AgentError> {
        Ok(match &self.fetcher {
            Some(fetcher) => fetcher.resolve_snapshot(snap).await,
            None => snap,
        })
    }

    /// The reader of what only the task holds, for a stream whose call activated `usage/v1` (the
    /// totals, ADR 0056) or `child-sessions/v1` (the sessions, ADR 0072); `None` when it activated
    /// neither.
    fn task_reader(
        &self,
        ep: &AgentEndpoint,
        activated: &[String],
        client: &Arc<A2AClient<Box<dyn Transport>>>,
    ) -> Option<TaskReader> {
        let usage = activated.iter().any(|uri| uri == USAGE_EXTENSION);
        let sessions = activated.iter().any(|uri| uri == CHILD_SESSIONS_EXTENSION);
        (usage || sessions).then(|| TaskReader {
            client: Arc::clone(client),
            timeout: self.cfg.call_timeout,
            usage,
            sessions,
            poll: self.cfg.child_sessions_poll,
            agent: self.clone(),
            endpoint: ep.clone(),
        })
    }

    /// Bounds a call that has no protocol-level timeout of its own.
    async fn timed<T>(
        &self,
        what: &str,
        call: impl Future<Output = Result<T, A2AError>>,
    ) -> Result<T, AgentError> {
        match tokio::time::timeout(self.cfg.call_timeout, call).await {
            Ok(result) => result.map_err(classify),
            Err(_) => Err(AgentError::unreachable(format!(
                "{what} timed out after {:?}",
                self.cfg.call_timeout
            ))),
        }
    }
}

type Item = Result<orch_ports::AgentEnvelope, AgentError>;

/// How a stream reads what only the task holds: the `usage/v1` totals, which are on the task and
/// never on a status update (ADR 0056), and the `sessions` object of `child-sessions/v1` (ADR
/// 0072), which a stream whose agent sends no events learns by reading the task again and again.
///
/// The reads run inline in the stream's loop: while one is out, the stream is not read (a call is
/// bounded by `call_timeout`, and the agent keeps what it sends in the connection meanwhile). The
/// order of what a read says is the totals, then the sessions: the listing is the newest truth, so
/// it comes last, and it wins over what an event said before it.
#[derive(Clone)]
struct TaskReader {
    client: Arc<A2AClient<Box<dyn Transport>>>,
    timeout: Duration,
    /// The call activated `usage/v1`.
    usage: bool,
    /// The call activated `child-sessions/v1`.
    sessions: bool,
    /// How often the stream reads the task for its sessions.
    poll: Duration,
    /// To read the card again before each read of sessions (ADR 0008: never remembered).
    agent: A2aAgentClient,
    endpoint: AgentEndpoint,
}

/// What one read of the task gave.
struct TaskRead {
    totals: Option<orch_ports::AgentEnvelope>,
    sessions: Option<orch_ports::AgentEnvelope>,
}

impl TaskReader {
    /// The task, once (`GetTask`); `None` when it cannot be read (best effort: a poll later reads it
    /// too).
    async fn task(&self, task_id: &str) -> Option<a2a::Task> {
        let request = GetTaskRequest {
            id: task_id.to_owned(),
            history_length: Some(0),
            tenant: None,
        };
        match tokio::time::timeout(self.timeout, self.client.get_task(&request)).await {
            Ok(Ok(task)) => Some(task),
            Ok(Err(e)) => {
                tracing::debug!(task = task_id, error = %e, "reading the task failed");
                None
            }
            Err(_) => {
                tracing::debug!(task = task_id, "reading the task timed out");
                None
            }
        }
    }

    /// Whether the card, read now, still offers `child-sessions/v1` well formed. A card that cannot
    /// be read does not: sessions are not read from an agent that cannot say it offers them.
    async fn still_offered(&self) -> bool {
        match self.agent.fetch_card(&self.endpoint).await {
            Ok(card) => child_sessions_from_card(&card).is_some(),
            Err(_) => false,
        }
    }

    /// What the task holds, for the read before a status that ends or pauses the turn.
    async fn read(&self, task_id: &str) -> TaskRead {
        let offered = self.sessions && self.still_offered().await;
        if !self.usage && !offered {
            return TaskRead {
                totals: None,
                sessions: None,
            };
        }
        let Some(task) = self.task(task_id).await else {
            return TaskRead {
                totals: None,
                sessions: None,
            };
        };
        TaskRead {
            totals: self.usage.then(|| usage_totals(&task)).flatten(),
            sessions: offered.then(|| sessions_news(&task)).flatten(),
        }
    }
}

/// What a read of the task says, in its place: after what the update itself said, and before the
/// first status it comes with (the totals, then the sessions). With no status among them it comes
/// first.
fn with_reads(mut mapped: Vec<Item>, reads: Vec<orch_ports::AgentEnvelope>) -> Vec<Item> {
    let at = mapped
        .iter()
        .position(|item| {
            matches!(
                item,
                Ok(orch_ports::AgentEnvelope {
                    update: Some(orch_core::AgentUpdate::Status { .. }),
                    ..
                })
            )
        })
        .unwrap_or(0);
    mapped.splice(at..at, reads.into_iter().map(Ok));
    mapped
}

/// Drops what an agent said of sessions to a call that did not ask for it (the contract: an
/// agent says none of it to a request that did not activate the extension; one that does anyway
/// is not believed, ADR 0008).
fn unasked(item: &Item) -> bool {
    matches!(
        item,
        Ok(orch_ports::AgentEnvelope {
            update: Some(
                orch_core::AgentUpdate::ChildSessions(_)
                    | orch_core::AgentUpdate::ChildSessionsRejected(_)
            ),
            ..
        })
    )
}

/// A snapshot without what its agent said of sessions, for a call that did not activate them.
fn without_sessions(mut snap: TaskSnapshot) -> TaskSnapshot {
    snap.envelopes.retain(|e| {
        !matches!(
            e.update,
            Some(
                orch_core::AgentUpdate::ChildSessions(_)
                    | orch_core::AgentUpdate::ChildSessionsRejected(_)
            )
        )
    });
    snap
}

/// Whether an item is a trusted-component request the card read for this call did not offer
/// (ADR 0083): a component the card does not list is never drawn, whatever the agent asked for.
fn unoffered(item: &Item, trusted: Option<&TrustedComponentsOffer>) -> bool {
    matches!(
        item,
        Ok(orch_ports::AgentEnvelope {
            update: Some(orch_core::AgentUpdate::TrustedRequest(request)),
            ..
        }) if trusted.is_none_or(|offer| !offer.offers(request.component))
    )
}

struct Mapping {
    inner: BoxStream<'static, Result<a2a::StreamResponse, A2AError>>,
    fetcher: Option<Fetcher>,
    /// Present when the call activated `usage/v1` or `child-sessions/v1`.
    reader: Option<TaskReader>,
    /// The call activated `child-sessions/v1`: only then is anything said of sessions believed.
    sessions: bool,
    /// The card read for the call offered `trusted-components/v1` well formed: only a request for
    /// one of its components is believed (ADR 0083).
    trusted: Option<TrustedComponentsOffer>,
    /// Ticks while the task runs, when the call activated `child-sessions/v1`.
    poll: Option<tokio::time::Interval>,
    mapper: StreamMapper,
    queue: VecDeque<Item>,
    seen_any: bool,
    ended: bool,
}

/// Maps the SDK's stream. A response that ends without a single event is reported as an
/// error: the SDK treats a body-less answer to a streaming call as an empty stream, which is
/// what a refused request (for example a proxy's 401, whose status the SDK drops) looks like.
///
/// With `reader` (the call activated `usage/v1` or `child-sessions/v1`), a status that ends or
/// pauses the task has the task read (`GetTask`): its totals and its sessions are passed on after
/// what that update said itself and before its status. With `child-sessions/v1` the task is also read
/// every `child_sessions_poll` while the stream is open and has carried no event, which is how a
/// parent that sends none is followed; the card is read again before each read, and a card that no
/// longer offers the extension stops them. What an agent says of sessions to a call that did not
/// activate the extension is dropped.
fn map_stream(
    inner: BoxStream<'static, Result<a2a::StreamResponse, A2AError>>,
    fetcher: Option<Fetcher>,
    reader: Option<TaskReader>,
    trusted: Option<TrustedComponentsOffer>,
) -> AgentStream {
    let sessions = reader.as_ref().is_some_and(|r| r.sessions);
    let poll = reader.as_ref().filter(|r| r.sessions).map(|r| {
        let mut interval = tokio::time::interval_at(tokio::time::Instant::now() + r.poll, r.poll);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval
    });
    let state = Mapping {
        inner,
        fetcher,
        reader,
        sessions,
        trusted,
        poll,
        mapper: StreamMapper::default(),
        queue: VecDeque::new(),
        seen_any: false,
        ended: false,
    };
    futures::stream::unfold(state, |mut st| async move {
        loop {
            if let Some(item) = st.queue.pop_front() {
                // A link on an allowed host becomes the file it names (ADR 0032), in order.
                let item = match (item, &st.fetcher) {
                    (Ok(env), Some(fetcher)) => Ok(fetcher.resolve(env).await),
                    (item, _) => item,
                };
                return Some((item, st));
            }
            if st.ended {
                return None;
            }
            let next = match st.poll.as_mut() {
                Some(interval) => tokio::select! {
                    item = st.inner.next() => Some(item),
                    _ = interval.tick() => None,
                },
                None => Some(st.inner.next().await),
            };
            let Some(next) = next else {
                // time to read the task for its sessions, unless the agent sends events (it is told
                // when something changes) or its card no longer offers them
                let task_id = st.mapper.task_id().map(str::to_owned);
                match (&st.reader, task_id) {
                    (Some(reader), Some(task_id)) if !st.mapper.has_session_events() => {
                        if reader.still_offered().await {
                            if let Some(task) = reader.task(&task_id).await
                                && let Some(news) = sessions_news(&task)
                                && let Some(news) = st.mapper.sessions_read(news, false)
                            {
                                st.queue.push_back(Ok(news));
                            }
                        } else {
                            st.poll = None;
                        }
                    }
                    (_, _) if st.mapper.has_session_events() => st.poll = None,
                    _ => {}
                }
                continue;
            };
            match next {
                Some(Ok(response)) => {
                    st.seen_any = true;
                    let wants = wants_task_read(&response).map(str::to_owned);
                    let read = match (&st.reader, wants) {
                        (Some(reader), Some(task_id)) => Some(reader.read(&task_id).await),
                        _ => None,
                    };
                    let mapped = st.mapper.map(response);
                    // what the update said itself, then what the task says, then its status
                    let mut reads = Vec::new();
                    if let Some(read) = read {
                        reads.extend(read.totals);
                        reads.extend(
                            read.sessions
                                .and_then(|news| st.mapper.sessions_read(news, true)),
                        );
                    }
                    let mut items = with_reads(mapped, reads);
                    if !st.sessions {
                        items.retain(|item| !unasked(item));
                    }
                    items.retain(|item| !unoffered(item, st.trusted.as_ref()));
                    st.queue.extend(items);
                }
                Some(Err(e)) => {
                    st.seen_any = true;
                    st.queue.push_back(Err(classify(e)));
                }
                None => {
                    st.ended = true;
                    if !st.seen_any {
                        st.queue.push_back(Err(AgentError::protocol(
                            "the agent closed the stream without sending an event; check its \
                             authentication and availability",
                        )));
                    }
                }
            }
        }
    })
    .boxed()
}

/// What the message tells the agent of the screen's catalog: the request's delivery, and only
/// when the card read for this very call lists `ui-catalog/v2` (ADR 0008: nothing is remembered,
/// and a card without the extension gets the message it got before it existed).
fn catalog_for<'r>(
    req: &'r SendRequest,
    card_extensions: &BTreeSet<KnownExtension>,
) -> Option<&'r UiDelivery> {
    req.ui_catalog
        .as_ref()
        .filter(|_| card_extensions.contains(&KnownExtension::UiCatalog))
}

/// The extensions of a call that makes the agent report (a send or a resubscribe) that do not
/// depend on the message, each when the card read for this very call lists it: `steps/v1`, so
/// that the agent reports its work as nested steps (ADR 0025), `text-stream/v1`, so that it
/// sends its reply as it writes it (ADR 0027), `usage/v1`, so that it reports the tokens of its
/// model calls (ADR 0056), `child-sessions/v1`, so that it reports its helpers as sessions (ADR
/// 0072; `child_sessions` is whether `child_sessions_from_card` read a well-formed offer from that
/// card), and `trusted-components/v1`, so that it may ask the client to draw one (ADR 0083;
/// `trusted_components` is whether the card offered a component this build draws). An agent
/// without them is plain A2A.
fn reporting_extensions(
    card_extensions: &BTreeSet<KnownExtension>,
    child_sessions: bool,
    trusted_components: bool,
) -> Vec<String> {
    let mut uris: Vec<String> = [
        (KnownExtension::Steps, STEPS_EXTENSION),
        (KnownExtension::TextStream, TEXT_STREAM_EXTENSION),
        (KnownExtension::Usage, USAGE_EXTENSION),
    ]
    .into_iter()
    .filter(|(ext, _)| card_extensions.contains(ext))
    .map(|(_, uri)| uri.to_owned())
    .collect();
    if child_sessions {
        uris.push(CHILD_SESSIONS_EXTENSION.to_owned());
    }
    if trusted_components {
        uris.push(TRUSTED_COMPONENTS_EXTENSION.to_owned());
    }
    uris
}

/// What rides on a message besides its text, each only when the card read for this very call allows
/// it (ADR 0008): the thread-tools grant, the mentions, the access tier and the upload limits.
#[derive(Clone, Copy, Default)]
struct Riders<'a> {
    thread_tools: Option<&'a ThreadToolsGrant>,
    mentions: Option<&'a Value>,
    tier: Option<&'a Value>,
    uploads: Option<&'a Value>,
}

/// The extensions this message uses, by URI: release channels when a release is selected, A2UI
/// when the live card lists it, the UI catalog when the message carries one, the thread tools
/// when it carries a grant, and `steps/v1` and `text-stream/v1` when the card lists them (ADR 0008:
/// read for this very call, never remembered).
fn extensions_of(
    req: &SendRequest,
    ui: Option<&UiSupport>,
    catalog: Option<&UiDelivery>,
    riders: Riders<'_>,
    reporting: &[String],
) -> Vec<String> {
    let mut uris = Vec::new();
    if req.release.is_some() {
        uris.push(RELEASE_CHANNELS_URI.to_owned());
    }
    if let Some(uri) = ui
        .and_then(UiSupport::preferred)
        .and_then(UiVersion::extension_uri)
    {
        uris.push(uri.to_owned());
    }
    if catalog.is_some() {
        // `ui-catalog/v2`, activated only when the card lists it (`catalog_for`)
        uris.push(UI_CATALOG_EXTENSION.to_owned());
    }
    // a steer says what it is by the extension (steer/v1 section 2): the request, not the message
    // body, carries it
    if req.steer {
        uris.push(STEER_EXTENSION.to_owned());
    }
    if riders.thread_tools.is_some() {
        uris.push(THREAD_TOOLS_EXTENSION.to_owned());
    }
    if riders.mentions.is_some() {
        uris.push(MENTIONS_EXTENSION.to_owned());
    }
    if riders.tier.is_some() {
        uris.push(ACCESS_TIER_EXTENSION.to_owned());
    }
    if riders.uploads.is_some() {
        uris.push(UPLOADS_EXTENSION.to_owned());
    }
    uris.extend(reporting.iter().cloned());
    uris
}

fn user_message(
    req: &SendRequest,
    ui: Option<&UiSupport>,
    catalog: Option<&UiDelivery>,
    riders: Riders<'_>,
    reporting: &[String],
) -> Message {
    let part = match &req.content {
        // The first task of a fork is told the conversation it continues, in front of the
        // message and in the same part: plain A2A, any agent reads it (ADR 0029).
        SendContent::Text(text) => Part::text(match &req.history {
            Some(history) => format!("{}{text}", history_preamble(history)),
            None => text.clone(),
        }),
        SendContent::UiAction { action, at } => action_part(action, *at),
        // the contract asks for one text part: the prompt of a start, "close" for a close
        SendContent::SessionStart { text, .. } => Part::text(text.clone()),
        SendContent::SessionClose { .. } => Part::text("close"),
    };
    let mut message = Message::new(Role::User, vec![part]);
    message.message_id = req.message_id.clone();
    // No context for the first message of a conversation: the agent starts one and names it in its
    // answer, and the dispatcher records it (ADR 0055). An agent such as kagent refuses a context
    // it did not create.
    message.context_id = req.context_id.clone().filter(|c| !c.is_empty());
    // a person's start or close is a message of a context, never of a task (contract sections 4
    // and 6: a `close` that names a `taskId` is `InvalidParams`)
    let session = session_metadata(&req.content);
    message.task_id = req.task_id.clone().filter(|_| session.is_none());
    // A2A `referenceTaskIds`: the earlier tasks this one is about (ADR 0021)
    if !req.reference_task_ids.is_empty() && session.is_none() {
        message.reference_task_ids = Some(req.reference_task_ids.clone());
    }
    let mut metadata: HashMap<String, serde_json::Value> = HashMap::new();
    if let Some(session) = session {
        metadata.insert(CHILD_SESSIONS_EXTENSION.to_owned(), session);
    }
    if let Some(release) = &req.release {
        metadata.insert(
            RELEASE_CHANNELS_URI.to_owned(),
            json!({ "release": release }),
        );
    }
    // The renderer's capabilities (`a2uiRendererCapabilities`, `"1.0"`) go with every message, and
    // only to an agent whose live card lists the A2UI v1.0 extension: without it the message is
    // plain A2A. The screen's own catalog is listed in them (and carried inline when the agent
    // takes it) only to an agent that also lists ui-catalog/v2 (ADR 0075).
    let accepts_inline = ui.is_some_and(|ui| ui.accepts_inline_catalogs);
    if ui.is_some() {
        let (key, value) = renderer_capabilities(catalog, accepts_inline);
        metadata.insert(key.to_owned(), value);
    }
    if let Some(delivery) = catalog {
        // `inline` says whether the catalog is in `inlineCatalogs` of this very message (which
        // needs the A2UI extension, so `accepts_inline` is false without it).
        let inline = inline_catalog(Some(delivery), accepts_inline).is_some();
        metadata.insert(
            UI_CATALOG_EXTENSION.to_owned(),
            ui_catalog_metadata(delivery, inline),
        );
    }
    // The endpoint of the thread and the token that opens it: only in the message, never stored
    // or logged anywhere else (see `crate::thread_tools`); with it the servers attached and, for
    // an asked agent, what the person said in the thread.
    if let Some(grant) = riders.thread_tools {
        let (attached, person) = req.thread_tools.as_ref().map_or((&[][..], None), |g| {
            (g.attached.as_slice(), g.person.as_ref())
        });
        metadata.insert(
            THREAD_TOOLS_EXTENSION.to_owned(),
            thread_tools_metadata(grant, attached, person),
        );
    }
    // The agents the person addressed, as references: only to an agent whose live card lists the
    // extension (`crate::mentions`); the text itself is untouched.
    if let Some(mentions) = riders.mentions {
        metadata.insert(MENTIONS_EXTENSION.to_owned(), mentions.clone());
    }
    // How much the agent may do on its own for this message, only when the live card honours it
    // (`crate::access_tier`): the person's choice, never the model's.
    if let Some(tier) = riders.tier {
        metadata.insert(ACCESS_TIER_EXTENSION.to_owned(), tier.clone());
    }
    // What the deployment offers for large files, only when the live card lists the extension and
    // the agent has the endpoint the tool is on (`crate::uploads`).
    if let Some(uploads) = riders.uploads {
        metadata.insert(UPLOADS_EXTENSION.to_owned(), uploads.clone());
    }
    let extensions = extensions_of(req, ui, catalog, riders, reporting);
    if !metadata.is_empty() {
        message.metadata = Some(metadata);
    }
    if !extensions.is_empty() {
        message.extensions = Some(extensions);
    }
    message
}

impl AgentClient for A2aAgentClient {
    async fn read_card(&self, ep: &AgentEndpoint) -> Result<AgentCardInfo, AgentError> {
        let card = self.fetch_card(ep).await?;
        Ok(AgentCardInfo {
            name: Some(card.name.clone()).filter(|n| !n.trim().is_empty()),
            description: Some(card.description.clone()).filter(|d| !d.trim().is_empty()),
            version: Some(card.version.clone()).filter(|v| !v.trim().is_empty()),
            releases: releases_from_card(&card),
            ui: ui_from_card(&card),
            extensions: extensions_from_card(&card),
            access_tier: access_tier_from_card(&card),
            commands: commands_from_card(&card),
            child_sessions: child_sessions_from_card(&card),
            trusted_components: trusted_components_from_card(&card),
            build: build_from_card(&card),
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
        let known = extensions_from_card(&card);
        // A person's start or close is sent only to an agent whose card, read for this very call,
        // offers child-sessions/v1 well formed (ADR 0008, fail closed): without it the agent would
        // take the message as a turn of the parent, or refuse it in words no one can act on.
        if session_metadata(&req.content).is_some() && child_sessions_from_card(&card).is_none() {
            return Err(AgentError::Unsupported(
                "the agent does not offer child-sessions/v1".to_owned(),
            ));
        }
        // A message for a running task is sent only to an agent whose card, read for this very
        // call, lists steer/v1 (ADR 0008, fail closed): the specification leaves it undefined, so
        // an agent without the promise could do anything with it.
        if req.steer && !known.contains(&KnownExtension::Steer) {
            return Err(AgentError::Unsupported(
                "the agent does not list steer/v1".to_owned(),
            ));
        }
        // A steer is the person's words to a task that runs: no screen capabilities, no catalog and
        // no reporting extensions of its own (the task keeps reporting on the stream that started
        // it), only what rides along on any message (the thread tools).
        let ui = ui_from_card(&card).filter(|_| !req.steer);
        let catalog = catalog_for(&req, &known).filter(|_| !req.steer);
        // Minted for this message only, from the card read for this very call (ADR 0008). An ask
        // whose token cannot be minted is not sent (`crate::thread_tools`).
        let thread_tools = mint(
            self.cfg.thread_tools.as_deref(),
            req.thread_tools.as_ref(),
            &req.message_id,
            &known,
            jiff::Timestamp::now(),
        )?;
        let mentions = mentions_metadata(&req, &known, thread_tools.is_some(), self.cfg.asks);
        // A message that names a tier is sent only to a card that still honours it (ADR 0061).
        let listing = access_tier_from_card(&card);
        check_plain_under_cap(&req, &listing)?;
        let tier = tier_metadata(&req, listing.offer())?;
        // The limits of a large file, when the extension is activated for this message (ADR 0058).
        let uploads = uploads_metadata(&req, &known, thread_tools.is_some());
        // a steer asks for no reporting of its own (the task keeps reporting on the stream that
        // started it), but the contract wants `child-sessions/v1` on every request about a child
        // context, and the card read for this call decides; a trusted component may be asked in
        // answer to the steer too, so that extension is activated when the card offers one
        let child = child_sessions_from_card(&card).is_some();
        let trusted = trusted_components_from_card(&card);
        let reporting = if req.steer {
            let mut uris = Vec::new();
            if child {
                uris.push(CHILD_SESSIONS_EXTENSION.to_owned());
            }
            if trusted.is_some() {
                uris.push(TRUSTED_COMPONENTS_EXTENSION.to_owned());
            }
            uris
        } else {
            reporting_extensions(&known, child, trusted.is_some())
        };
        let riders = Riders {
            thread_tools: thread_tools.as_ref(),
            mentions: mentions.as_ref(),
            tier: tier.as_ref(),
            uploads: uploads.as_ref(),
        };
        let client = Arc::new(
            self.client_for(
                &req.endpoint,
                &card,
                extensions_of(&req, ui.as_ref(), catalog, riders, &reporting),
            )
            .await?,
        );
        let reader = self.task_reader(&req.endpoint, &reporting, &client);
        let request = SendMessageRequest {
            message: user_message(&req, ui.as_ref(), catalog, riders, &reporting),
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
        Ok(map_stream(stream, self.fetcher.clone(), reader, trusted))
    }

    async fn resubscribe(&self, task: &TaskHandle) -> Result<AgentStream, AgentError> {
        // The card is read for this call, and an agent that lists `steps/v1` or `text-stream/v1` is
        // asked for them again: a stream that was cut and picked up must carry the same events the
        // first did.
        let card = self.fetch_card(&task.endpoint).await?;
        let trusted = trusted_components_from_card(&card);
        let reporting = reporting_extensions(
            &extensions_from_card(&card),
            child_sessions_from_card(&card).is_some(),
            trusted.is_some(),
        );
        let client = Arc::new(
            self.client_for(&task.endpoint, &card, reporting.clone())
                .await?,
        );
        let reader = self.task_reader(&task.endpoint, &reporting, &client);
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
        Ok(map_stream(stream, self.fetcher.clone(), reader, trusted))
    }

    async fn get_task(&self, task: &TaskHandle) -> Result<TaskSnapshot, AgentError> {
        let (client, activated) = self.connect_for_task(&task.endpoint).await?;
        let request = GetTaskRequest {
            id: task.task_id.clone(),
            history_length: Some(0),
            tenant: None,
        };
        let found = self
            .timed("reading the task", client.get_task(&request))
            .await?;
        let snap = snapshot(&found)?;
        self.files_of(if activated {
            snap
        } else {
            without_sessions(snap)
        })
        .await
    }

    async fn cancel(&self, task: &TaskHandle) -> Result<TaskSnapshot, AgentError> {
        let (client, activated) = self.connect_for_task(&task.endpoint).await?;
        let request = CancelTaskRequest {
            id: task.task_id.clone(),
            metadata: None,
            tenant: None,
        };
        let canceled = self
            .timed("cancelling the task", client.cancel_task(&request))
            .await?;
        let snap = snapshot(&canceled)?;
        self.files_of(if activated {
            snap
        } else {
            without_sessions(snap)
        })
        .await
    }

    /// A2A 1.0 has no lookup by message id, so this lists the tasks of the context (`ListTasks`;
    /// of every context when the message named none, the first of a thread, ADR 0055, bounded by
    /// the same page limit) and finds the one whose history holds the message. Limits: a follow-up message to an
    /// existing task is only found if the agent records it in the task history (the SDK's
    /// default handler does not), and an agent without `ListTasks` yields `Ok(None)`.
    async fn find_task_by_message(
        &self,
        ep: &AgentEndpoint,
        context_id: Option<&str>,
        message_id: &str,
    ) -> Result<Option<String>, AgentError> {
        const MAX_PAGES: usize = 10;
        // a child context is listed only by a request that activated the extension
        let (client, _) = self.connect_for_task(ep).await?;
        let mut page_token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let request = ListTasksRequest {
                context_id: context_id.map(str::to_owned),
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use orch_core::{
        AgentTaskState, AgentUpdate, TrustedComponent, TrustedHints, TrustedRequestData,
    };
    use orch_ports::{AgentEnvelope, IdemKey};

    use super::*;

    fn env(word: &str, update: Option<AgentUpdate>) -> AgentEnvelope {
        AgentEnvelope {
            task_id: "t".into(),
            context_id: "c".into(),
            task_state: None,
            revision: None,
            key: IdemKey::Turn(word.into()),
            update,
            live: None,
        }
    }

    fn status() -> AgentEnvelope {
        env(
            "status",
            Some(AgentUpdate::Status {
                state: AgentTaskState::Completed,
                detail: None,
                tier: None,
            }),
        )
    }

    fn word(item: &Item) -> String {
        match &item.as_ref().unwrap().key {
            IdemKey::Turn(w) | IdemKey::Task(w) => w.clone(),
        }
    }

    #[test]
    fn what_a_read_of_the_task_says_comes_after_the_updates_own_news_and_before_its_status() {
        let own = env("own-news", None);
        let mapped = vec![Ok(own), Ok(status())];
        let reads = vec![env("totals", None), env("listing", None)];
        let order: Vec<String> = with_reads(mapped, reads).iter().map(word).collect();
        // the newest truth, the task's own listing, is the last thing said before the status
        assert_eq!(order, ["own-news", "totals", "listing", "status"]);
    }

    #[test]
    fn a_read_with_no_status_to_precede_is_said_first() {
        let order: Vec<String> = with_reads(vec![Ok(env("a", None))], vec![env("r", None)])
            .iter()
            .map(word)
            .collect();
        assert_eq!(order, ["r", "a"]);
    }

    #[test]
    fn the_trusted_components_extension_is_activated_only_when_the_card_offers_one() {
        let known = BTreeSet::new();
        let activated = |trusted| {
            reporting_extensions(&known, false, trusted)
                .contains(&TRUSTED_COMPONENTS_EXTENSION.to_owned())
        };
        assert!(!activated(false));
        assert!(activated(true));
    }

    #[test]
    fn a_request_for_a_component_the_card_did_not_offer_is_dropped() {
        let offer =
            TrustedComponentsOffer::from_params(&json!({"components": ["McpCredential"]})).unwrap();
        let item: Item = Ok(env(
            "trusted",
            Some(AgentUpdate::TrustedRequest(TrustedRequestData {
                component: TrustedComponent::McpCredential,
                hints: TrustedHints::default(),
            })),
        ));
        // the card read for the call offered it: believed
        assert!(!unoffered(&item, Some(&offer)));
        // the card did not list the extension, or listed none this build draws: never drawn
        assert!(unoffered(&item, None));
    }
}
