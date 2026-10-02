//! The MCP servers a person may attach to a conversation (ADR 0024): what the application knows of
//! each, which is the part of the deployment's list that is **not a secret**.
//!
//! The URL of a server and its credentials are not here. They belong to the relay that will call
//! the server for the agent, and reach nothing the application writes: no event, no outbox row, no
//! API answer, no log line. What the application keeps of a server is its id, the words a person
//! reads, its icon, which agents may use it, and how long a call may take.

use std::fmt;
use std::time::Duration;

use orch_core::{AgentId, AttachedServer};

/// The key of a run's `forwardedProps` that attaches MCP servers to the thread the run creates
/// (ADR 0024): an array of server ids.
pub const THREAD_TOOLS_KEY: &str = "vymalo.tools";

/// A server the deployment lists, as far as the application needs it (`toolServers` of the
/// configuration, [`docs/api/config.md`](../../../../docs/api/config.md)).
#[derive(Clone, PartialEq, Eq)]
pub struct ToolServerInfo {
    /// The id, `^[a-z0-9][a-z0-9-]{0,30}$`: what a thread records and the prefix of the server's
    /// tools on the thread's endpoint (`<id>__<tool>`).
    pub id: String,
    /// The name the picker and the steps show.
    pub name: String,
    /// What the server is for, shown to the person and told to the agent.
    pub description: Option<String>,
    /// The icon: a `data:` URI of at most 8 KiB, drawn as it is. Never fetched, never a URL.
    pub icon: Option<String>,
    /// The allow-list of the server's own tool names the relay serves; `None`: every tool whose
    /// name the relay can expose. Not told to the web.
    pub tools: Option<Vec<String>>,
    /// The agents the server may be attached for; `None`: every agent.
    pub agents: Option<Vec<AgentId>>,
    /// The longest one call may take.
    pub timeout: Duration,
}

impl ToolServerInfo {
    /// A server with a name and nothing else: every agent, every tool, 120 seconds.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        ToolServerInfo {
            id: id.into(),
            name: name.into(),
            description: None,
            icon: None,
            tools: None,
            agents: None,
            timeout: Duration::from_secs(120),
        }
    }

    /// Whether the server may be attached for `agent`, which the thread talks to: the `agents`
    /// list names it, or there is no list.
    pub fn allows(&self, agent: &AgentId) -> bool {
        self.agents
            .as_ref()
            .is_none_or(|agents| agents.contains(agent))
    }

    /// What an agent is told of the server (`attached` of the `thread-tools/v1` message): id,
    /// name and description. Never a URL or a credential: this type has neither.
    pub fn told(&self) -> AttachedServer {
        AttachedServer {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
        }
    }
}

impl fmt::Debug for ToolServerInfo {
    /// Without the icon, which is up to 8 KiB of base64.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolServerInfo")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("description", &self.description)
            .field(
                "icon",
                &self.icon.as_ref().map(|i| format!("{} bytes", i.len())),
            )
            .field("tools", &self.tools)
            .field("agents", &self.agents)
            .field("timeout", &self.timeout)
            .finish()
    }
}
