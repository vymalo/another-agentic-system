//! In-memory tool servers: MCP servers that live in a test, and a [`ToolServerClient`] that talks to
//! them without a network.
//!
//! A test adds servers by URL ([`MemoryToolServers::add`]), each with the tools it offers and what
//! each tool does ([`ToolScript`]), the credentials it wants, and takes one down
//! ([`MemoryToolServers::set_down`]). The client checks the credentials of the endpoint it is
//! given as a real server would, bounds every request by the endpoint's timeout, cuts a result
//! at [`MAX_RESULT_BYTES`], and the server keeps a journal of what it was asked
//! ([`MemoryToolServers::seen`]). It is the reference implementation of the tool-server testkit
//! and what the relay's unit tests will play a server with.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Map, Value, json};

use crate::testkit::tool_server::{FAIL_TEXT, SeenRequest};
use crate::{
    MAX_RESULT_BYTES, ToolCall, ToolCallOutput, ToolDef, ToolSecret, ToolServerClient,
    ToolServerEndpoint, ToolServerError,
};

/// What a tool of a [`MemoryToolServer`] does with a call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolScript {
    /// Answers its arguments, as one text block (their JSON) and as the structured result.
    Echo,
    /// Answers this text.
    Text(String),
    /// Answers this text with `isError: true`: the tool ran and failed.
    Failed(String),
    /// Answers one text block of `x`, as many as the argument `bytes` says.
    Filler,
    /// Never answers: the caller's timeout, or dropping the call, ends it.
    Hang,
    /// Answers with a JSON-RPC error of this code and message.
    Raise {
        /// The JSON-RPC error code.
        code: i32,
        /// The message.
        message: String,
    },
}

/// One server: the tools it offers and the credentials it wants.
#[derive(Debug, Clone, Default)]
pub struct MemoryToolServer {
    bearer: Option<ToolSecret>,
    headers: Vec<(String, ToolSecret)>,
    tools: Vec<(ToolDef, ToolScript)>,
    down: bool,
    seen: Vec<SeenRequest>,
}

impl MemoryToolServer {
    /// A server with no tools that wants no credentials.
    pub fn new() -> Self {
        Self::default()
    }

    /// The server's tools, as the tool-server testkit describes them: `echo`, `fail`, `slow` and
    /// `big`.
    #[must_use]
    pub fn with_standard_tools(self) -> Self {
        let object = |properties: Value| -> Map<String, Value> {
            let Value::Object(map) = json!({"type": "object", "properties": properties}) else {
                unreachable!("an object")
            };
            map
        };
        let tool = |name: &str, description: &str, properties: Value| ToolDef {
            name: name.to_owned(),
            title: None,
            description: Some(description.to_owned()),
            input_schema: object(properties),
            output_schema: None,
            annotations: None,
        };
        self.with_tool(
            tool(
                "echo",
                "Answers its arguments.",
                json!({"text": {"type": "string"}}),
            ),
            ToolScript::Echo,
        )
        .with_tool(
            tool("fail", "Always fails.", json!({})),
            ToolScript::Failed(FAIL_TEXT.to_owned()),
        )
        .with_tool(tool("slow", "Never answers.", json!({})), ToolScript::Hang)
        .with_tool(
            tool(
                "big",
                "Answers `bytes` bytes of text.",
                json!({"bytes": {"type": "integer"}}),
            ),
            ToolScript::Filler,
        )
    }

    /// The server wants `Authorization: Bearer <bearer>`.
    #[must_use]
    pub fn with_bearer(mut self, bearer: impl Into<String>) -> Self {
        self.bearer = Some(ToolSecret::new(bearer));
        self
    }

    /// The server wants the header `name: value`.
    #[must_use]
    pub fn with_required_header(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.headers.push((name.into(), ToolSecret::new(value)));
        self
    }

    /// The server offers `tool`, which does what `script` says.
    #[must_use]
    pub fn with_tool(mut self, tool: ToolDef, script: ToolScript) -> Self {
        self.tools.push((tool, script));
        self
    }

    /// Whether the endpoint's credentials are the ones this server wants.
    fn accepts(&self, endpoint: &ToolServerEndpoint) -> bool {
        let bearer_ok = match (&self.bearer, &endpoint.bearer) {
            (Some(want), Some(got)) => want == got,
            (Some(_), None) => false,
            (None, _) => true,
        };
        bearer_ok
            && self.headers.iter().all(|(name, want)| {
                endpoint
                    .headers
                    .iter()
                    .any(|(n, got)| n.eq_ignore_ascii_case(name) && got == want)
            })
    }
}

#[derive(Debug, Default)]
struct State {
    servers: BTreeMap<String, MemoryToolServer>,
}

/// The in-memory [`ToolServerClient`]: the servers a test added, by URL. Cheap to clone: clones share
/// the servers and their journals, so a test keeps one to script and read while the application
/// holds another. A URL no server was added at is a server nobody answers for.
#[derive(Debug, Clone, Default)]
pub struct MemoryToolServers {
    state: Arc<Mutex<State>>,
}

impl MemoryToolServers {
    /// No servers.
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Serves `server` at `url`, in place of any server there.
    pub fn add(&self, url: impl Into<String>, server: MemoryToolServer) {
        self.state().servers.insert(url.into(), server);
    }

    /// Takes the server at `url` down (`true`): until it is put back up nobody answers for it.
    pub fn set_down(&self, url: &str, down: bool) {
        if let Some(server) = self.state().servers.get_mut(url) {
            server.down = down;
        }
    }

    /// What the server at `url` was asked since it was added, oldest first; requests it refused for
    /// their credentials are not in it.
    pub fn seen(&self, url: &str) -> Vec<SeenRequest> {
        self.state()
            .servers
            .get(url)
            .map(|s| s.seen.clone())
            .unwrap_or_default()
    }

    /// What the script says, for the server at the endpoint's URL, or why it cannot say.
    fn request(
        &self,
        endpoint: &ToolServerEndpoint,
        method: &str,
        call: Option<&ToolCall>,
    ) -> Result<Answer, ToolServerError> {
        let mut state = self.state();
        let server = state
            .servers
            .get_mut(&endpoint.url)
            .filter(|s| !s.down)
            .ok_or_else(|| ToolServerError::unreachable("nothing answers there"))?;
        if !server.accepts(endpoint) {
            return Err(ToolServerError::Unauthenticated);
        }
        server.seen.push(SeenRequest {
            method: method.to_owned(),
            name: call.map(|c| c.name.clone()),
            arguments: call.map(|c| c.arguments.clone()),
            meta: call.and_then(|c| c.meta.clone()),
            bearer: endpoint.bearer.as_ref().map(|b| b.expose().to_owned()),
            headers: endpoint
                .headers
                .iter()
                .map(|(n, v)| (n.to_ascii_lowercase(), v.expose().to_owned()))
                .collect(),
        });
        match call {
            None => Ok(Answer::Tools(
                server.tools.iter().map(|(def, _)| def.clone()).collect(),
            )),
            Some(call) => {
                let (_, script) = server
                    .tools
                    .iter()
                    .find(|(def, _)| def.name == call.name)
                    .ok_or_else(|| ToolServerError::remote(-32602, "unknown tool"))?;
                Ok(Answer::Script(script.clone()))
            }
        }
    }
}

/// What a request found, once the lock is let go.
enum Answer {
    Tools(Vec<ToolDef>),
    Script(ToolScript),
}

impl ToolServerClient for MemoryToolServers {
    async fn list_tools(
        &self,
        server: &ToolServerEndpoint,
    ) -> Result<Vec<ToolDef>, ToolServerError> {
        match self.request(server, "tools/list", None)? {
            Answer::Tools(tools) => Ok(tools),
            Answer::Script(_) => unreachable!("a listing is answered with tools"),
        }
    }

    async fn call_tool(
        &self,
        server: &ToolServerEndpoint,
        call: &ToolCall,
    ) -> Result<ToolCallOutput, ToolServerError> {
        let Answer::Script(script) = self.request(server, "tools/call", Some(call))? else {
            unreachable!("a call is answered with a script")
        };
        let run = async {
            match script {
                ToolScript::Echo => {
                    let arguments = Value::Object(call.arguments.clone());
                    Ok(ToolCallOutput {
                        structured: Some(arguments.clone()),
                        ..ToolCallOutput::text(arguments.to_string())
                    })
                }
                ToolScript::Text(text) => Ok(ToolCallOutput::text(text)),
                ToolScript::Failed(text) => Ok(ToolCallOutput::failed(text)),
                ToolScript::Filler => {
                    let bytes = call
                        .arguments
                        .get("bytes")
                        .and_then(Value::as_u64)
                        .and_then(|n| usize::try_from(n).ok())
                        .unwrap_or(0);
                    Ok(ToolCallOutput::text("x".repeat(bytes)))
                }
                ToolScript::Hang => std::future::pending().await,
                ToolScript::Raise { code, message } => Err(ToolServerError::remote(code, &message)),
            }
        };
        match tokio::time::timeout(server.timeout, run).await {
            Ok(answer) => answer.map(|output| output.bounded(MAX_RESULT_BYTES)),
            Err(_) => Err(ToolServerError::TimedOut {
                after: server.timeout,
            }),
        }
    }
}
