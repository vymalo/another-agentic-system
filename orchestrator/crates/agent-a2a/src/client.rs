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
use orch_a2a_mapping::{StreamMapper, snapshot};
use orch_core::{
    BoxError, KnownExtension, STEPS_EXTENSION, TEXT_STREAM_EXTENSION, THREAD_TOOLS_EXTENSION,
    UI_CATALOG_EXTENSION, UiDelivery, UiVersion, history_preamble,
};
use orch_ports::{
    AgentCardInfo, AgentClient, AgentEndpoint, AgentError, AgentStream, AgentTransport,
    SendContent, SendRequest, TaskHandle, TaskSnapshot, UiSupport,
};
use orch_thread_token::{ThreadToolsGrant, ThreadToolsIssuer};
use serde_json::json;

use crate::a2ui::{
    action_part, client_capabilities, inline_catalog, ui_catalog_metadata, ui_from_card,
};
use crate::errors::classify;
use crate::extensions::extensions_from_card;
use crate::files::{Fetcher, FileFetch};
use crate::releases::{RELEASE_CHANNELS_URI, releases_from_card};
use crate::thread_tools::{mint, thread_tools_metadata};

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
    /// The hosts a `url` part of an artifact may be fetched from, and the size it may have
    /// (ADR 0032, `artifacts.fetchHosts`). Without it (the default) every `url` part stays a link.
    pub fetch_files: Option<FileFetch>,
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
            fetch_files: None,
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

    /// Fetches the live card and builds a client for it.
    async fn connect(
        &self,
        ep: &AgentEndpoint,
    ) -> Result<A2AClient<Box<dyn Transport>>, AgentError> {
        let card = self.fetch_card(ep).await?;
        self.client_for(ep, &card, Vec::new()).await
    }

    /// The snapshot with the links on allowed hosts turned into the files they name.
    async fn files_of(&self, snap: TaskSnapshot) -> Result<TaskSnapshot, AgentError> {
        Ok(match &self.fetcher {
            Some(fetcher) => fetcher.resolve_snapshot(snap).await,
            None => snap,
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

struct Mapping {
    inner: BoxStream<'static, Result<a2a::StreamResponse, A2AError>>,
    fetcher: Option<Fetcher>,
    mapper: StreamMapper,
    queue: VecDeque<Item>,
    seen_any: bool,
    ended: bool,
}

/// Maps the SDK's stream. A response that ends without a single event is reported as an
/// error: the SDK treats a body-less answer to a streaming call as an empty stream, which is
/// what a refused request (for example a proxy's 401, whose status the SDK drops) looks like.
fn map_stream(
    inner: BoxStream<'static, Result<a2a::StreamResponse, A2AError>>,
    fetcher: Option<Fetcher>,
) -> AgentStream {
    let state = Mapping {
        inner,
        fetcher,
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
/// when the card read for this very call lists `ui-catalog/v1` (ADR 0008: nothing is remembered,
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
/// that the agent reports its work as nested steps (ADR 0025), and `text-stream/v1`, so that it
/// sends its reply as it writes it (ADR 0027). An agent without them is plain A2A.
fn reporting_extensions(card_extensions: &BTreeSet<KnownExtension>) -> Vec<String> {
    [
        (KnownExtension::Steps, STEPS_EXTENSION),
        (KnownExtension::TextStream, TEXT_STREAM_EXTENSION),
    ]
    .into_iter()
    .filter(|(ext, _)| card_extensions.contains(ext))
    .map(|(_, uri)| uri.to_owned())
    .collect()
}

/// The extensions this message uses, by URI: release channels when a release is selected, A2UI
/// when the live card lists it, the UI catalog when the message carries one, the thread tools
/// when it carries a grant, and `steps/v1` and `text-stream/v1` when the card lists them (ADR 0008:
/// read for this very call, never remembered).
fn extensions_of(
    req: &SendRequest,
    ui: Option<&UiSupport>,
    catalog: Option<&UiDelivery>,
    thread_tools: Option<&ThreadToolsGrant>,
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
        uris.push(UI_CATALOG_EXTENSION.to_owned());
    }
    if thread_tools.is_some() {
        uris.push(THREAD_TOOLS_EXTENSION.to_owned());
    }
    uris.extend(reporting.iter().cloned());
    uris
}

fn user_message(
    req: &SendRequest,
    ui: Option<&UiSupport>,
    catalog: Option<&UiDelivery>,
    thread_tools: Option<&ThreadToolsGrant>,
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
    };
    let mut message = Message::new(Role::User, vec![part]);
    message.message_id = req.message_id.clone();
    message.context_id = Some(req.context_id.clone());
    message.task_id = req.task_id.clone();
    // A2A `referenceTaskIds`: the earlier tasks this one is about (ADR 0021)
    if !req.reference_task_ids.is_empty() {
        message.reference_task_ids = Some(req.reference_task_ids.clone());
    }
    let mut metadata: HashMap<String, serde_json::Value> = HashMap::new();
    if let Some(release) = &req.release {
        metadata.insert(
            RELEASE_CHANNELS_URI.to_owned(),
            json!({ "release": release }),
        );
    }
    // The renderer's capabilities go with every message, and only to an agent whose live card
    // lists the extension: without it the message is plain A2A. The screen's own catalog is
    // listed in them (and carried inline when the agent takes it) only to an agent that also
    // lists ui-catalog/v1.
    let accepts_inline = ui.is_some_and(|ui| ui.accepts_inline_catalogs);
    if let Some((key, value)) = ui
        .and_then(UiSupport::preferred)
        .and_then(|version| client_capabilities(version, catalog, accepts_inline))
    {
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
    // or logged anywhere else (see `crate::thread_tools`).
    if let Some(grant) = thread_tools {
        let attached = req
            .thread_tools
            .as_ref()
            .map_or(&[][..], |g| g.attached.as_slice());
        metadata.insert(
            THREAD_TOOLS_EXTENSION.to_owned(),
            thread_tools_metadata(grant, attached),
        );
    }
    let extensions = extensions_of(req, ui, catalog, thread_tools, reporting);
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
            description: Some(card.description.clone()).filter(|d| !d.trim().is_empty()),
            version: Some(card.version.clone()).filter(|v| !v.trim().is_empty()),
            releases: releases_from_card(&card),
            ui: ui_from_card(&card),
            extensions: extensions_from_card(&card),
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
        let ui = ui_from_card(&card);
        let known = extensions_from_card(&card);
        let catalog = catalog_for(&req, &known);
        // Minted for this message only, from the card read for this very call (ADR 0008).
        let thread_tools = mint(
            self.cfg.thread_tools.as_deref(),
            req.thread_tools.as_ref(),
            &req.message_id,
            &known,
            jiff::Timestamp::now(),
        );
        let reporting = reporting_extensions(&known);
        let client = self
            .client_for(
                &req.endpoint,
                &card,
                extensions_of(
                    &req,
                    ui.as_ref(),
                    catalog,
                    thread_tools.as_ref(),
                    &reporting,
                ),
            )
            .await?;
        let request = SendMessageRequest {
            message: user_message(
                &req,
                ui.as_ref(),
                catalog,
                thread_tools.as_ref(),
                &reporting,
            ),
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
        Ok(map_stream(stream, self.fetcher.clone()))
    }

    async fn resubscribe(&self, task: &TaskHandle) -> Result<AgentStream, AgentError> {
        // The card is read for this call, and an agent that lists `steps/v1` or `text-stream/v1` is
        // asked for them again: a stream that was cut and picked up must carry the same events the
        // first did.
        let card = self.fetch_card(&task.endpoint).await?;
        let reporting = reporting_extensions(&extensions_from_card(&card));
        let client = self.client_for(&task.endpoint, &card, reporting).await?;
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
        Ok(map_stream(stream, self.fetcher.clone()))
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
        self.files_of(snapshot(&found)?).await
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
        self.files_of(snapshot(&canceled)?).await
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
