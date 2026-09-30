//! The MCP server: `initialize`, `tools/list` and `tools/call` over [`App`].
//!
//! A tool call is one `App` call made as the token's user, so it is scoped exactly as the
//! resource API is: a job that is not the caller's is "no such job", never "forbidden". No tool
//! keeps state in the process; a fresh server is built for every request (stateless mode).

use std::sync::Arc;
use std::time::Duration;

use orch_app::{AgentDirectory, App, AppError, Creation, Inbound, NewThread};
use orch_core::{
    AgentId, AgentTarget, Classify, ErrorClass, Input, Origin, ThreadId, UserId, report,
};
use orch_ports::{IdGen, Ports};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ProgressNotificationParam, ProgressToken, ServerCapabilities,
    ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, Peer, RoleServer, ServerHandler};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::Settings;
use crate::auth::McpUser;
use crate::id::{MAX_CLIENT_REQUEST_ID_BYTES, job_id_for};
use crate::job::{JobSummary, summarise};
use crate::tools::{
    AnswerArgs, CancelJobArgs, GetJobArgs, NoArgs, StartJobArgs, ToolName, WaitForJobArgs,
};
use crate::wait::{ProgressSink, WaitRequest, wait_for_job};

/// What the server tells a client about itself, once, at `initialize`.
const INSTRUCTIONS: &str = "Start a job with start_job and look at it with get_job, or follow it \
    with wait_for_job (call it again with resume_after_seq to keep waiting). \
    A job in state blocked waits for an answer: send it with answer. Jobs are the caller's own; \
    a job_id that is not yours is reported as unknown.";

/// The server for one request.
pub(crate) struct McpServer<P: Ports> {
    app: Arc<App<P>>,
    settings: Arc<Settings>,
}

impl<P: Ports> McpServer<P> {
    pub(crate) fn new(app: Arc<App<P>>, settings: Arc<Settings>) -> Self {
        McpServer { app, settings }
    }
}

/// Progress notifications to the client that made the request, when it asked for them (a
/// `progressToken`).
struct PeerSink {
    peer: Peer<RoleServer>,
    token: Option<ProgressToken>,
}

impl ProgressSink for PeerSink {
    async fn send(&self, progress: u64, message: String) -> bool {
        let Some(token) = &self.token else {
            return true;
        };
        // A counter of a few thousand at most: exact in an f64.
        #[allow(clippy::cast_precision_loss)]
        let progress = progress as f64;
        self.peer
            .notify_progress(
                ProgressNotificationParam::new(token.clone(), progress).with_message(message),
            )
            .await
            .is_ok()
    }
}

/// What `wait_for_job` answers: how the wait ended, where to resume, and the job as `get_job`
/// says it.
#[derive(Serialize)]
struct WaitResult {
    outcome: &'static str,
    resume_after_seq: i64,
    #[serde(flatten)]
    job: JobSummary,
}

/// The user the bearer check let in. A request that reaches a tool without one did not pass the
/// check, which only a wiring mistake can cause: the call is refused (fail closed).
fn caller(context: &RequestContext<RoleServer>) -> Result<UserId, ErrorData> {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<McpUser>())
        .map(|user| user.0.clone())
        .ok_or_else(|| {
            tracing::error!("an MCP request reached a tool without an authenticated user");
            ErrorData::internal_error("the request was not authenticated", None)
        })
}

/// Parses the arguments of a tool call.
fn parse_args<T: DeserializeOwned>(
    arguments: Option<serde_json::Map<String, Value>>,
) -> Result<T, ErrorData> {
    serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
        .map_err(|e| ErrorData::invalid_params(format!("invalid arguments: {e}"), None))
}

/// A successful result: the JSON, as text for the model and as structured content.
fn success(value: &impl Serialize) -> Result<CallToolResult, ErrorData> {
    let value = serde_json::to_value(value).map_err(|e| {
        ErrorData::internal_error(format!("cannot serialise the result: {e}"), None)
    })?;
    Ok(CallToolResult::structured(value))
}

/// A tool-level failure the caller should read.
fn refused(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

/// A job id as the caller wrote it. A malformed one is the same answer as an unknown one.
fn job_id(raw: &str) -> Option<ThreadId> {
    raw.trim().parse::<ThreadId>().ok()
}

/// What an application failure looks like to the caller. The causes of a failure the caller
/// cannot act on are logged, never sent.
fn failure(err: &AppError) -> Result<CallToolResult, ErrorData> {
    match err.class() {
        ErrorClass::NotFound => Ok(refused("no such job")),
        ErrorClass::Invalid => Ok(refused(err.to_string())),
        ErrorClass::Rejected => Ok(refused(match err {
            AppError::Finished => "the job is finished; start a new one with start_job".to_owned(),
            other => other.to_string(),
        })),
        ErrorClass::Conflict | ErrorClass::Transient | ErrorClass::RateLimited => {
            tracing::warn!(error = %report(err), "a tool call failed temporarily");
            Ok(refused("temporarily unavailable; try again"))
        }
        // Unauthenticated, unsupported, corrupt, internal, and any class added later: a fault of
        // the server, not something the caller can act on.
        _ => {
            tracing::error!(error = %report(err), class = ?err.class(), "a tool call failed");
            Err(ErrorData::internal_error("internal error", None))
        }
    }
}

impl<P: Ports> McpServer<P> {
    async fn list_agents(&self) -> Result<CallToolResult, ErrorData> {
        let agents = self.app.list_agents().await;
        let agents: Vec<Value> = agents
            .iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "name": a.name,
                    "description": a.description,
                })
            })
            .collect();
        success(&json!({ "agents": agents }))
    }

    /// The agent `start_job` uses when it is not named: the first configured one (ADR 0014).
    fn default_agent(directory: &AgentDirectory) -> Option<AgentId> {
        directory.iter().next().map(|e| e.endpoint.id.clone())
    }

    async fn start_job(
        &self,
        user: &UserId,
        args: StartJobArgs,
    ) -> Result<CallToolResult, ErrorData> {
        let client_request_id = match args.client_request_id.as_deref().map(str::trim) {
            None => None,
            Some("") => return Ok(refused("client_request_id must not be empty")),
            Some(id) if id.len() > MAX_CLIENT_REQUEST_ID_BYTES => {
                return Ok(refused(format!(
                    "client_request_id must be at most {MAX_CLIENT_REQUEST_ID_BYTES} bytes"
                )));
            }
            Some(id) => Some(id),
        };
        let agent_id = match args.agent.as_deref().map(str::trim) {
            Some(id) if !id.is_empty() => AgentId::new(id),
            _ => match Self::default_agent(self.app.directory()) {
                Some(id) => id,
                None => return Ok(refused("no agent is configured")),
            },
        };
        // A retried call names the same job; without a request id every call is new.
        let id = match client_request_id {
            Some(request) => job_id_for(user, request),
            None => ThreadId(self.app.ports().ids().new_id()),
        };
        let new = NewThread {
            title: args.title,
            target: AgentTarget {
                agent_id,
                release: None,
            },
            text: args.text,
        };
        let inbound = Inbound {
            origin: Origin::Mcp,
            ..Inbound::default()
        };
        let (thread, created) = match self.app.create_thread_as(user, id, new, inbound).await {
            Ok(Creation::Created { thread, .. }) => (thread, true),
            // The caller's own job with this id: a retry, or a request that lost a race with
            // itself. Nothing was written; the answer is the job that is there.
            Ok(Creation::Exists) => match self.app.get_thread(user, id).await {
                Ok(thread) => (thread, false),
                Err(e) => return failure(&e),
            },
            Err(e) => return failure(&e),
        };
        let mut result = json!({
            "job_id": thread.id,
            "state": thread.state.as_str(),
            "created": created,
        });
        if let (Some(url), Some(object)) =
            (self.settings.web_url(thread.id), result.as_object_mut())
        {
            object.insert("web_url".to_owned(), Value::String(url));
        }
        success(&result)
    }

    async fn get_job(&self, user: &UserId, args: GetJobArgs) -> Result<CallToolResult, ErrorData> {
        let Some(id) = job_id(&args.job_id) else {
            return Ok(refused("no such job"));
        };
        let thread = match self.app.get_thread(user, id).await {
            Ok(thread) => thread,
            Err(e) => return failure(&e),
        };
        match summarise(&self.app, user, &thread).await {
            Ok(summary) => success(&summary),
            Err(e) => failure(&e),
        }
    }

    async fn wait_for_job(
        &self,
        user: &UserId,
        args: WaitForJobArgs,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(id) = job_id(&args.job_id) else {
            return Ok(refused("no such job"));
        };
        if args.after_seq.is_some_and(|after| after < 0) {
            return Ok(refused("after_seq must not be negative"));
        }
        let request = WaitRequest {
            after_seq: args.after_seq,
            timeout: Duration::from_secs(args.timeout_secs).min(self.settings.wait_max),
            heartbeat: self.settings.heartbeat,
        };
        let sink = PeerSink {
            peer: context.peer.clone(),
            token: context.meta.get_progress_token(),
        };
        let waited = match wait_for_job(&self.app, user, id, &request, &sink, &context.ct).await {
            Ok(waited) => waited,
            Err(e) => return failure(&e),
        };
        match summarise(&self.app, user, &waited.thread).await {
            Ok(job) => success(&WaitResult {
                outcome: waited.end.as_str(),
                resume_after_seq: waited.resume_after_seq,
                job,
            }),
            Err(e) => failure(&e),
        }
    }

    async fn answer(&self, user: &UserId, args: AnswerArgs) -> Result<CallToolResult, ErrorData> {
        let Some(id) = job_id(&args.job_id) else {
            return Ok(refused("no such job"));
        };
        let input = Input::UserMessage {
            user: user.clone(),
            text: args.text,
            message_id: None,
            run_id: None,
            origin: Origin::Mcp,
        };
        // No idempotency key: an `answer` is not made safe to retry (ADR 0019).
        match self.app.submit(user, id, input, None).await {
            Ok(orch_app::ApplyOutcome::Applied { thread, .. }) => {
                success(&json!({ "job_id": thread.id, "state": thread.state.as_str() }))
            }
            Ok(orch_app::ApplyOutcome::Duplicate | orch_app::ApplyOutcome::Fenced) => {
                tracing::error!("a message without an idempotency key or a lease was not applied");
                Err(ErrorData::internal_error("internal error", None))
            }
            Err(e) => failure(&e),
        }
    }

    async fn cancel_job(
        &self,
        user: &UserId,
        args: CancelJobArgs,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(id) = job_id(&args.job_id) else {
            return Ok(refused("no such job"));
        };
        // Cancelling a finished job is a no-op, as in the chat.
        if let Err(e) = self.app.cancel(user, id).await {
            return failure(&e);
        }
        let thread = match self.app.get_thread(user, id).await {
            Ok(thread) => thread,
            Err(e) => return failure(&e),
        };
        // The agent confirms a cancel later: until then the job is still what it was.
        success(&json!({
            "job_id": thread.id,
            "state": thread.state.as_str(),
            "finished": thread.state.is_terminal(),
        }))
    }
}

impl<P: Ports> ServerHandler for McpServer<P> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(
            ToolName::ALL.iter().map(|t| t.definition()).collect(),
        ))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        ToolName::parse(name).map(ToolName::definition)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let user = caller(&context)?;
        let Some(tool) = ToolName::parse(&request.name) else {
            return Err(ErrorData::invalid_params(
                format!("unknown tool {:?}", request.name),
                None,
            ));
        };
        let result = match tool {
            ToolName::ListAgents => {
                parse_args::<NoArgs>(request.arguments)?;
                self.list_agents().await
            }
            ToolName::StartJob => self.start_job(&user, parse_args(request.arguments)?).await,
            ToolName::GetJob => self.get_job(&user, parse_args(request.arguments)?).await,
            ToolName::WaitForJob => {
                let args = parse_args(request.arguments)?;
                self.wait_for_job(&user, args, &context).await
            }
            ToolName::Answer => self.answer(&user, parse_args(request.arguments)?).await,
            ToolName::CancelJob => self.cancel_job(&user, parse_args(request.arguments)?).await,
        };
        result.map(CallToolResponse::from)
    }
}
