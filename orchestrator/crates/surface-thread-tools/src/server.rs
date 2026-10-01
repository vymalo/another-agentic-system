//! The MCP server: `initialize`, `tools/list` and `tools/call` for one thread.
//!
//! A fresh server is built for every request (stateless mode), and every request has been through
//! the guard, which put what the token says in the request. The built-in tools are asked first,
//! then each provider in the order it was added.

use std::sync::Arc;
use std::time::Duration;

use orch_app::App;
use orch_ports::Ports;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};

use crate::Settings;
use crate::guard::Verified;
use crate::provider::{ProgressSink, ToolCtx};
use crate::tools::ThreadTool;

/// What the server tells a client about itself, once, at `initialize`.
const INSTRUCTIONS: &str = "The tools of one conversation, offered to the agent working on it. \
    List them again before you plan a turn: what is attached to the conversation can change \
    between turns. Never put the token that opened this endpoint into a prompt, a tool argument \
    or a log.";

/// The server for one request.
pub(crate) struct ThreadToolsServer<P: Ports> {
    app: Arc<App<P>>,
    settings: Arc<Settings>,
}

impl<P: Ports> ThreadToolsServer<P> {
    pub(crate) fn new(app: Arc<App<P>>, settings: Arc<Settings>) -> Self {
        ThreadToolsServer { app, settings }
    }
}

/// What the guard let in, as the tools see it. A request that reaches a tool without a verified
/// token did not pass the guard, which only a wiring mistake can cause: the call is refused
/// (fail closed).
fn tool_ctx(context: &RequestContext<RoleServer>) -> Result<ToolCtx, ErrorData> {
    let verified = context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Verified>())
        .ok_or_else(|| {
            tracing::error!("a thread-tools request reached a tool without a verified token");
            ErrorData::internal_error("the request was not authenticated", None)
        })?;
    let meta = &context.meta.0.0;
    Ok(ToolCtx {
        owner: verified.owner.clone(),
        claims: verified.claims.clone(),
        meta: (!meta.is_empty()).then(|| meta.clone()),
        progress: ProgressSink::new(context.peer.clone(), context.meta.get_progress_token()),
        cancel: context.ct.clone(),
    })
}

/// Runs a built-in tool for at most `timeout`: the machine route has no request timeout of its
/// own.
async fn within(
    timeout: Duration,
    tool: ThreadTool,
    call: impl Future<Output = Result<CallToolResult, ErrorData>>,
) -> Result<CallToolResult, ErrorData> {
    if let Ok(result) = tokio::time::timeout(timeout, call).await {
        result
    } else {
        tracing::warn!(
            tool = tool.name(),
            ?timeout,
            "a tool call took too long and was cut off"
        );
        Ok(CallToolResult::error(vec![ContentBlock::text(format!(
            "{} took longer than {} s and was stopped; try again",
            tool.name(),
            timeout.as_secs().max(1)
        ))]))
    }
}

impl<P: Ports> ServerHandler for ThreadToolsServer<P> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let ctx = tool_ctx(&context)?;
        let mut tools: Vec<_> = ThreadTool::ALL.iter().map(|t| t.definition()).collect();
        for provider in &self.settings.providers {
            // A provider that is slow or cannot list leaves its tools out; it never fails the
            // listing.
            match tokio::time::timeout(self.settings.tool_timeout, provider.list(&ctx)).await {
                Ok(listed) => {
                    for tool in listed {
                        // The first to own a name keeps it: the order of `tools/call`.
                        if tools.iter().any(|t| t.name == tool.name) {
                            tracing::warn!(tool = %tool.name, "a provider offers a tool whose name is taken; it is left out");
                        } else {
                            tools.push(tool);
                        }
                    }
                }
                Err(_) => {
                    tracing::warn!("a provider did not list its tools in time; they are left out")
                }
            }
        }
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let ctx = tool_ctx(&context)?;
        if let Some(tool) = ThreadTool::parse(&request.name) {
            let limit = self.settings.tool_timeout;
            return within(limit, tool, tool.call(&self.app, &ctx, request.arguments))
                .await
                .map(CallToolResponse::from);
        }
        let args = request.arguments.unwrap_or_default();
        for provider in &self.settings.providers {
            if let Some(result) = provider.call(&ctx, &request.name, args.clone()).await {
                return result.map(CallToolResponse::from);
            }
        }
        Err(ErrorData::invalid_params(
            format!("unknown tool {:?}", request.name),
            None,
        ))
    }
}
