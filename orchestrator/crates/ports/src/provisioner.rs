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

    /// A copy of this error, for one more waiter of a call they share (the error is not `Clone`:
    /// an `Unavailable` holds a boxed driver error). The copy is the same variant with the same
    /// fields; an `Unavailable`'s cause is copied as text, **with its whole `source()` chain** (so
    /// a log that walks the chain still sees "connection refused" under "error sending request"),
    /// but it is no longer the driver's own type.
    ///
    /// The match is exhaustive on purpose, with no `_` arm: **a variant added to this enum must be
    /// copied faithfully here**, and the compiler refuses to build until it is. A fallback arm would
    /// turn it into another kind of error for every waiter but the first.
    #[must_use]
    pub fn duplicate(&self) -> Self {
        match self {
            ProvisionError::Unavailable { source } => ProvisionError::Unavailable {
                source: Box::new(CopiedError::of(&**source)),
            },
            ProvisionError::Refused => ProvisionError::Refused,
            ProvisionError::Invalid => ProvisionError::Invalid,
            ProvisionError::NotReady { last } => ProvisionError::NotReady { last: *last },
            ProvisionError::Deleting => ProvisionError::Deleting,
        }
    }
}

/// An error copied as text with its `source()` chain: what [`ProvisionError::duplicate`] puts in
/// an `Unavailable`.
#[derive(Debug)]
struct CopiedError {
    text: String,
    source: Option<Box<CopiedError>>,
}

impl CopiedError {
    fn of(error: &(dyn std::error::Error + 'static)) -> Self {
        CopiedError {
            text: error.to_string(),
            source: error.source().map(|cause| Box::new(CopiedError::of(cause))),
        }
    }
}

impl fmt::Display for CopiedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::error::Error for CopiedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Outer(std::io::Error);

    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("error sending request")
        }
    }

    impl std::error::Error for Outer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn a_duplicate_keeps_the_variant_and_the_cause_chain() {
        let error = ProvisionError::unavailable(Outer(std::io::Error::other("connection refused")));
        let copy = error.duplicate();
        let ProvisionError::Unavailable { source } = &copy else {
            panic!("not Unavailable: {copy:?}");
        };
        assert_eq!(source.to_string(), "error sending request");
        let cause = std::error::Error::source(&**source).expect("the cause is kept");
        assert_eq!(cause.to_string(), "connection refused");
        assert!(matches!(
            ProvisionError::NotReady {
                last: AgentState::Blocked
            }
            .duplicate(),
            ProvisionError::NotReady {
                last: AgentState::Blocked
            }
        ));
        assert!(matches!(
            ProvisionError::Refused.duplicate(),
            ProvisionError::Refused
        ));
        assert!(matches!(
            ProvisionError::Invalid.duplicate(),
            ProvisionError::Invalid
        ));
        assert!(matches!(
            ProvisionError::Deleting.duplicate(),
            ProvisionError::Deleting
        ));
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
    ///
    /// **A call that is dropped (cut by a timeout) strands nothing** (ANR-1078): the agent it was
    /// making or resuming goes on coming up by itself, and the next call finds it ready or still
    /// coming up and waits for it, never making a second one. Nothing the caller does after a
    /// point it is not at (a sleep, a second request) is what turns a state into `Ready`: the
    /// caller is cut at any `.await`, and the dispatcher cuts it on purpose to free its slot.
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
