//! The `AgentProvisioner` port: a person's own agent, made on their first message and woken on the
//! next one.
//!
//! The platform makes the agents (ADR 0022); this port is how the orchestrator asks for one that
//! belongs to a person. The shared `chat` agent is not provisioned per person and never goes
//! through it.

use std::fmt;
use std::future::Future;

use orch_core::{BoxError, Classify, ErrorClass};

/// The longest an owner key is, in characters: the agent's name is `<kind>-<owner>` and a name is
/// at most 40.
pub const MAX_OWNER_CHARS: usize = 32;

/// Who an agent belongs to, as a key the platform accepts: 1 to [`MAX_OWNER_CHARS`] characters of
/// lower-case letters, digits and `-`, not ending in `-`. The caller derives it from the person,
/// the same way every time; the port never sees the person's identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentOwner(String);

/// An owner key that does not fit the rule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("an owner key is 1 to 32 characters of a-z, 0-9 and '-', not ending in '-'")]
pub struct InvalidAgentOwner;

impl AgentOwner {
    /// Checks and keeps `key`.
    pub fn new(key: impl Into<String>) -> Result<Self, InvalidAgentOwner> {
        let key = key.into();
        let ok = !key.is_empty()
            && key.chars().count() <= MAX_OWNER_CHARS
            && !key.ends_with('-')
            && key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if ok {
            Ok(AgentOwner(key))
        } else {
            Err(InvalidAgentOwner)
        }
    }

    /// The key.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The kinds of agent a person has of their own. Closed on purpose: a new kind must fail to
/// compile in every adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AgentKind {
    /// A coding agent (a task to a pull request).
    Coder,
    /// A workspace agent.
    Workspace,
}

impl AgentKind {
    /// The first part of the agent's name on the platform.
    pub fn prefix(self) -> &'static str {
        match self {
            AgentKind::Coder => "coder",
            AgentKind::Workspace => "workspace",
        }
    }

    /// The name of the owner's agent of this kind: `<prefix>-<owner>`. One owner has at most one
    /// agent of a kind, so asking twice names the same agent.
    pub fn agent_name(self, owner: &AgentOwner) -> String {
        format!("{}-{}", self.prefix(), owner)
    }
}

/// Where an agent is, as the platform says it (its lifecycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Being made or changed.
    Saving,
    /// Cannot start (its configuration is refused).
    Blocked,
    /// Running, not fully healthy.
    Degraded,
    /// Ready for a message.
    Ready,
    /// Switched off by its owner; a message must resume it.
    Suspended,
    /// Being deleted.
    Deleting,
}

/// An agent of a person that is ready for a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionedAgent {
    /// The agent's name on the platform (the id the registry lists it under).
    pub name: String,
    /// Always [`AgentState::Ready`] when returned by [`AgentProvisioner::ensure_ready`].
    pub state: AgentState,
}

/// Why the provisioner did not give an agent.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProvisionError {
    /// The platform could not be reached or answered with a failure that may pass.
    #[error("the platform is unavailable")]
    Unavailable {
        /// The driver's error.
        #[source]
        source: BoxError,
    },
    /// The platform refused the orchestrator's credentials or its permission.
    #[error("the platform refused the orchestrator")]
    Refused,
    /// The platform refused the agent's form. Retrying the same request cannot help.
    #[error("the platform refused the agent's form")]
    Invalid,
    /// The agent did not become ready in time; `last` is where it stood.
    #[error("the agent is not ready (it is {last:?})")]
    NotReady {
        /// The last state seen.
        last: AgentState,
    },
    /// The agent is being deleted; ask again once it is gone.
    #[error("the agent is being deleted")]
    Deleting,
}

impl ProvisionError {
    /// The platform is down or answered nonsense.
    pub fn unavailable(source: impl Into<BoxError>) -> Self {
        ProvisionError::Unavailable {
            source: source.into(),
        }
    }
}

impl Classify for ProvisionError {
    fn class(&self) -> ErrorClass {
        match self {
            ProvisionError::Unavailable { .. }
            | ProvisionError::NotReady { .. }
            | ProvisionError::Deleting => ErrorClass::Transient,
            ProvisionError::Refused => ErrorClass::Unauthenticated,
            ProvisionError::Invalid => ErrorClass::Invalid,
        }
    }
}

/// Makes, wakes, suspends and deletes a person's own agents.
///
/// Every method acts on **the agent of this owner and kind** and nothing else: an owner never
/// reaches another owner's agent, whatever the platform holds.
pub trait AgentProvisioner: Send + Sync + 'static {
    /// The owner's agent of `kind`, ready for a message. Makes it when there is none, resumes it
    /// when it is suspended, and **waits until it is [`AgentState::Ready`]** (or gives up with
    /// [`ProvisionError::NotReady`]). Two calls at once for the same owner and kind make **one**
    /// agent and both get it.
    fn ensure_ready(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> impl Future<Output = Result<ProvisionedAgent, ProvisionError>> + Send;

    /// Where the owner's agent of `kind` is; `None` when there is none. Never makes one.
    fn state(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> impl Future<Output = Result<Option<AgentState>, ProvisionError>> + Send;

    /// Suspends the owner's agent (a later [`ensure_ready`](Self::ensure_ready) resumes it).
    /// Suspending one that is absent or already suspended changes nothing.
    fn suspend(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> impl Future<Output = Result<(), ProvisionError>> + Send;

    /// Deletes the owner's agent. Deleting one that is absent changes nothing.
    fn delete(
        &self,
        owner: &AgentOwner,
        kind: AgentKind,
    ) -> impl Future<Output = Result<(), ProvisionError>> + Send;
}
