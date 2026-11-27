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

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use orch_core::{AgentId, UserId};
use orch_ports::{
    AgentKind, AgentOwner, AgentProvisioner, AgentState, ProvisionError, ProvisionedAgent,
};
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tokio::time::Instant;

use crate::error::AppError;

/// How long past the wait-ready bound the application itself waits for the provisioner. The
/// adapter bounds its own wait and answers [`ProvisionError::NotReady`]; this is the backstop for
/// an adapter that does not, and it leaves the adapter's requests room to finish.
///
/// Public only so that the tests can bound how long a wait may last (`tests/provisioning.rs`
/// asserts it ends between the wait-ready bound and the bound plus this); nothing configures it.
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

/// How a wake ended, kept so that every waiter of the flight gets it (the port's error is not
/// `Clone`: each waiter gets [`ProvisionError::duplicate`], its cause chain kept).
enum Outcome {
    Ready(ProvisionedAgent),
    /// Not ready in time: the adapter's `NotReady`, or the backstop (`None`).
    NotReady(Option<AgentState>),
    Failed(ProvisionError),
}

type Flight = watch::Receiver<Option<Arc<Outcome>>>;
type Flights = Mutex<HashMap<(AgentOwner, AgentKind), Flight>>;

/// Removes a flight from the map when its task ends, whichever way (a panic included).
struct FlightEnd {
    flights: Arc<Flights>,
    key: (AgentOwner, AgentKind),
}

impl Drop for FlightEnd {
    fn drop(&mut self) {
        self.flights
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

/// The provisioner and the kinds the deployment provisions, as the message path uses them.
///
/// **A wake is a flight** (ANR-1078 PR 2): `ensure_ready` for one (owner, kind) runs in a task of
/// its own, one at a time, and every caller waits on it. A caller that stops waiting (the
/// dispatcher's probe is a `timeout`) drops its wait and never the request, so a platform slower
/// than the probe still finishes the wake, and a person with many waiting rows causes one request
/// to the platform. The map holds a flight until its task ends, which the backstop bounds. It is
/// process-local and holds no datum of the person's: a restart loses it and the next call starts
/// a flight again (the port makes that safe), so the invariant of one persistent state is kept.
#[derive(Clone)]
pub struct Provisioning {
    inner: Arc<dyn ErasedProvisioner>,
    kinds: Vec<AgentKind>,
    ready_timeout: Duration,
    flights: Arc<Flights>,
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
            flights: Arc::default(),
        }
    }

    /// The wait-ready bound.
    pub fn ready_timeout(&self) -> Duration {
        self.ready_timeout
    }

    /// The owner key of a person: 24 hex digits of the SHA-256 of their user id. The same person
    /// always has the same key, two people never do, and the key names nobody (an e-mail is not
    /// a legal key and is not the platform's business).
    ///
    /// The key is an unsalted hash of the user id: a stable pseudonym, **not a secret**. It is a
    /// function of the id the identity provider gives, so a deployment that changes which claim
    /// names a person (`sub`, an e-mail) moves every person to a new key, and so to a new agent;
    /// the old ones are left where they are (ADR 0103).
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

    /// The id the person's own agent is listed under (`<kind>-<owner>`, the name
    /// [`ProvisionedAgent::name`] carries), for a thread aimed at the agent known by `names`; none
    /// for an agent the deployment does not provision. A pure function of the person and the
    /// kind: the route of a fork that ran no `ensure` (ANR-1061, ADR 0103); a thread's route is
    /// otherwise the `name` the platform answered with, recorded in `Job.route`.
    pub(crate) fn own_agent_id(&self, user: &UserId, names: &[AgentId]) -> Option<AgentId> {
        let kind = self.kind_of(names)?;
        Some(AgentId::new(kind.agent_name(&Self::owner_of(user))))
    }

    /// Makes or resumes the person's agent for a message to the agent known by `names` and waits
    /// until it is ready. The agent's id, and `None` (nothing done) for an agent the deployment does not provision.
    ///
    /// # Errors
    /// [`AppError::AgentNotReady`] when it is not ready within the wait-ready bound,
    /// [`AppError::Provisioning`] for any other refusal of the platform.
    pub(crate) async fn ensure(
        &self,
        user: &UserId,
        names: &[AgentId],
    ) -> Result<Option<AgentId>, AppError> {
        let Some(kind) = self.kind_of(names) else {
            return Ok(None);
        };
        let owner = Self::owner_of(user);
        // This caller's own deadline, whatever flight it joins.
        let began = Instant::now();
        let deadline = began + self.ready_timeout.saturating_add(OUTER_SLACK);
        loop {
            let (outcome, started) = self.fly(owner.clone(), kind).await;
            match &*outcome {
                Outcome::Ready(agent) => return Ok(Some(AgentId::new(agent.name.clone()))),
                // A flight this caller joined may have been started long before it came, and ended
                // `NotReady` on its own bound. This caller still has time: it joins the next
                // flight, or starts one. A flight it started itself is final. (The dispatcher's
                // probe cuts this whole call at the probe, whichever way it goes.)
                Outcome::NotReady(_) if !started && Instant::now() < deadline => {}
                Outcome::NotReady(last) => {
                    return Err(AppError::AgentNotReady {
                        kind: kind.prefix(),
                        // the bound that ran out for a flight of its own; the time this caller
                        // actually waited for one it joined
                        waited: if started {
                            self.ready_timeout
                        } else {
                            began.elapsed()
                        },
                        last: *last,
                    });
                }
                Outcome::Failed(error) => {
                    return Err(AppError::Provisioning {
                        kind: kind.prefix(),
                        source: error.duplicate(),
                    });
                }
            }
        }
    }

    /// Joins the flight of `owner`'s agent of `kind`, starting it when there is none, and waits for
    /// its end; the flag says that this call started it. Dropping this future leaves the flight
    /// running. No lock is held across an await, nor across the spawn.
    async fn fly(&self, owner: AgentOwner, kind: AgentKind) -> (Arc<Outcome>, bool) {
        let key = (owner, kind);
        let (mut flight, task) = {
            let mut flights = self.flights.lock().unwrap_or_else(PoisonError::into_inner);
            let existing = flights.get(&key).cloned();
            match existing {
                Some(flight) => (flight, None),
                None => {
                    let (tx, rx) = watch::channel(None);
                    flights.insert(key.clone(), rx.clone());
                    let inner = Arc::clone(&self.inner);
                    // made here, not in the task: a task dropped before its first poll (a
                    // runtime that is shutting down) still removes its entry
                    let end = FlightEnd {
                        flights: Arc::clone(&self.flights),
                        key: key.clone(),
                    };
                    let bound = self.ready_timeout.saturating_add(OUTER_SLACK);
                    let owner = key.0.clone();
                    let task = async move {
                        let result =
                            tokio::time::timeout(bound, inner.ensure_ready(&owner, kind)).await;
                        let outcome = match result {
                            Ok(Ok(agent)) => Outcome::Ready(agent),
                            Ok(Err(ProvisionError::NotReady { last })) => {
                                Outcome::NotReady(Some(last))
                            }
                            Ok(Err(error)) => Outcome::Failed(error),
                            Err(_elapsed) => Outcome::NotReady(None),
                        };
                        // out of the map first: a call that comes now starts a flight of its own
                        drop(end);
                        let _ = tx.send(Some(Arc::new(outcome)));
                    };
                    (rx, Some(task))
                }
            }
        };
        // the lock is released: a spawn on a runtime that is shutting down drops the task at once,
        // and its `FlightEnd` takes the lock
        let started = task.is_some();
        if let Some(task) = task {
            tokio::spawn(task);
        }
        let outcome = match flight.wait_for(Option::is_some).await {
            Ok(done) => match &*done {
                Some(outcome) => Arc::clone(outcome),
                None => unreachable!("wait_for returned for a value that is set"),
            },
            // the task ended without an answer (it panicked): not ready
            Err(_) => Arc::new(Outcome::NotReady(None)),
        };
        (outcome, started)
    }
}
