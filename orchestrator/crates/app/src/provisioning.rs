//! A person's own agent on the message path (ANR-806, part 2): before a message is accepted for
//! an agent the deployment provisions, the person's agent of that kind is made, or resumed, and
//! waited for until it is ready.
//!
//! The port is [`AgentProvisioner`] (`orch-ports`). It returns `impl Future`, so it cannot be a
//! trait object; [`Provisioning`] erases it once, at the composition root, and the application
//! holds it in [`AppConfig`](crate::AppConfig). **Off by default**: a deployment with no
//! provisioner changes nothing.
//!
//! Which messages: the target agent of a thread is `coder` or `workspace` (the kind's
//! [`prefix`](AgentKind::prefix)) and the deployment provisions that kind. The shared `chat` agent
//! and every other agent are never provisioned.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use orch_core::{AgentId, UserId};
use orch_ports::{AgentKind, AgentOwner, AgentProvisioner, ProvisionError, ProvisionedAgent};
use sha2::{Digest, Sha256};

use crate::error::AppError;

/// How long past the wait-ready bound the application itself waits for the provisioner. The
/// adapter bounds its own wait and answers [`ProvisionError::NotReady`]; this is the backstop for
/// an adapter that does not, and it leaves the adapter's requests room to finish.
pub const OUTER_SLACK: Duration = Duration::from_secs(10);

/// Characters of the owner key: 24 hex digits (96 bits of the person's hash), so that the longest
/// agent name, `workspace-<owner>`, stays under the platform's 40.
const OWNER_HEX_CHARS: usize = 24;

trait ErasedProvisioner: Send + Sync {
    fn ensure_ready<'a>(
        &'a self,
        owner: &'a AgentOwner,
        kind: AgentKind,
    ) -> BoxFuture<'a, Result<ProvisionedAgent, ProvisionError>>;
}

impl<T: AgentProvisioner> ErasedProvisioner for T {
    fn ensure_ready<'a>(
        &'a self,
        owner: &'a AgentOwner,
        kind: AgentKind,
    ) -> BoxFuture<'a, Result<ProvisionedAgent, ProvisionError>> {
        Box::pin(AgentProvisioner::ensure_ready(self, owner, kind))
    }
}

/// The provisioner and the kinds the deployment provisions, as the message path uses them.
#[derive(Clone)]
pub struct Provisioning {
    inner: Arc<dyn ErasedProvisioner>,
    kinds: Vec<AgentKind>,
    ready_timeout: Duration,
}

impl fmt::Debug for Provisioning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provisioning")
            .field("kinds", &self.kinds)
            .field("ready_timeout", &self.ready_timeout)
            .finish_non_exhaustive()
    }
}

impl Provisioning {
    /// `provisioner` for the `kinds` the deployment makes for people. `ready_timeout` is the
    /// **wait-ready bound** (`provisioner.readyTimeoutSecs`, the one the adapter is given too): a
    /// person is told that the agent did not become ready within it, and nothing else bounds the
    /// wait but a backstop of [`OUTER_SLACK`] past it.
    pub fn new(
        provisioner: impl AgentProvisioner,
        kinds: impl IntoIterator<Item = AgentKind>,
        ready_timeout: Duration,
    ) -> Self {
        let mut kinds: Vec<AgentKind> = kinds.into_iter().collect();
        kinds.sort();
        kinds.dedup();
        Provisioning {
            inner: Arc::new(provisioner),
            kinds,
            ready_timeout,
        }
    }

    /// The wait-ready bound.
    pub fn ready_timeout(&self) -> Duration {
        self.ready_timeout
    }

    /// The owner key of a person: 24 hex digits of the SHA-256 of their user id. The same person
    /// always has the same key, two people never do, and the key names nobody (an e-mail is not
    /// a legal key and is not the platform's business).
    pub fn owner_of(user: &UserId) -> AgentOwner {
        let digest = Sha256::digest(user.as_str().as_bytes());
        let hex: String = digest
            .iter()
            .flat_map(|b| [b >> 4, b & 0x0f])
            .take(OWNER_HEX_CHARS)
            .map(|n| char::from_digit(u32::from(n), 16).unwrap_or('0'))
            .collect();
        // 24 characters of 0-9a-f always fit the rule.
        AgentOwner::new(hex).unwrap_or_else(|_| unreachable!("hex digits are a legal owner key"))
    }

    /// The kind the deployment provisions for a thread aimed at an agent known by `names` (its id
    /// and its aliases, ADR 0049), if any: the agent the deployment calls `coder` or `workspace`.
    pub fn kind_of<'a>(&self, names: impl IntoIterator<Item = &'a AgentId>) -> Option<AgentKind> {
        names.into_iter().find_map(|name| {
            self.kinds
                .iter()
                .copied()
                .find(|k| k.prefix() == name.0.as_str())
        })
    }

    /// Makes or resumes the person's agent for a message to the agent known by `names` and waits
    /// until it is ready. Nothing to do (and `Ok`) for an agent the deployment does not provision.
    ///
    /// # Errors
    /// [`AppError::AgentNotReady`] when it is not ready within the wait-ready bound,
    /// [`AppError::Provisioning`] for any other refusal of the platform.
    pub(crate) async fn ensure(&self, user: &UserId, names: &[AgentId]) -> Result<(), AppError> {
        let Some(kind) = self.kind_of(names) else {
            return Ok(());
        };
        let owner = Self::owner_of(user);
        let bound = self.ready_timeout.saturating_add(OUTER_SLACK);
        let result = tokio::time::timeout(bound, self.inner.ensure_ready(&owner, kind)).await;
        match result {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(ProvisionError::NotReady { last })) => Err(AppError::AgentNotReady {
                kind: kind.prefix(),
                waited: self.ready_timeout,
                last: Some(last),
            }),
            Ok(Err(source)) => Err(AppError::Provisioning {
                kind: kind.prefix(),
                source,
            }),
            Err(_elapsed) => Err(AppError::AgentNotReady {
                kind: kind.prefix(),
                waited: self.ready_timeout,
                last: None,
            }),
        }
    }
}
