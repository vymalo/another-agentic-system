//! Single flight: one refresh at a time per `(owner, server)` in a process (ADR 0046, decision 5 of
//! the amendment of 2026-10-11), and what the callers that wait for it are told. Across processes a
//! claim written with `SecretStore::rotate` settles it (see `broker.rs`).
//!
//! Two tools, both keyed by `(owner, server)`:
//!
//! - [`Flights::acquire`], a mutex: whoever writes the credential holds it (a refresh, a revoke,
//!   the end of an authorization), so none of them writes under another.
//! - [`Flights::join`], the first caller of a refresh **leads** it and every other that arrives while
//!   it runs **follows**: it waits for the leader's outcome and does not ask the authorization
//!   server again. Without it a refresh that fails (or hangs until its timeout) is repeated by every
//!   caller in turn, which is a request per caller to a server that is failing already.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use orch_core::UserId;
use orch_ports::{CredentialError, CredentialLease, EgressRefusal, UserToolServerId};
use tokio::sync::{OwnedMutexGuard, watch};

use crate::broker::egress_refusal;

type Key = (UserId, UserToolServerId);

/// How many keys the table may hold before the entries nobody holds are dropped.
const PRUNE_AT: usize = 64;

/// How a refresh ended, as the callers that waited for it are told. Nothing here holds a secret, a
/// URL or a source error: the detail is the leader's fixed phrase, and the refusal is a rule.
#[derive(Debug, Clone)]
pub(crate) enum Settled {
    /// The tokens were refreshed and kept (or another replica did it): read them again.
    Refreshed,
    NeedsReauth,
    Unknown,
    Unavailable {
        detail: String,
        refusal: Option<EgressRefusal>,
    },
}

impl Settled {
    /// What the leader's result tells the followers.
    pub(crate) fn of(result: &Result<CredentialLease, CredentialError>) -> Self {
        match result {
            Ok(_) => Settled::Refreshed,
            Err(CredentialError::NeedsReauth) => Settled::NeedsReauth,
            Err(CredentialError::Unknown) => Settled::Unknown,
            Err(error @ CredentialError::Unavailable { detail, .. }) => Settled::Unavailable {
                detail: detail.clone(),
                refusal: egress_refusal(error),
            },
            // a kind of failure a later version of the port names
            Err(_) => Settled::Unavailable {
                detail: "the refresh failed".to_owned(),
                refusal: None,
            },
        }
    }

    /// The error a follower returns for a leader that failed; `None` for `Refreshed`.
    pub(crate) fn error(&self) -> Option<CredentialError> {
        match self {
            Settled::Refreshed => None,
            Settled::NeedsReauth => Some(CredentialError::NeedsReauth),
            Settled::Unknown => Some(CredentialError::Unknown),
            Settled::Unavailable { detail, refusal } => {
                let error = CredentialError::unavailable(detail.clone());
                Some(match refusal {
                    // the API finds the refusal in the chain, for a follower as for the leader
                    Some(refusal) => error.with_source(*refusal),
                    None => error,
                })
            }
        }
    }
}

type Outcome = Option<Settled>;

/// The first caller of a refresh: it does the work and tells the followers how it came out.
pub(crate) struct Lead {
    sender: Arc<watch::Sender<Outcome>>,
}

impl Lead {
    /// Tells the followers. A leader dropped without it (a panic) leaves them an interruption.
    pub(crate) fn settle(self, outcome: Settled) {
        self.sender.send_replace(Some(outcome));
    }
}

/// A caller that arrived while a refresh was running.
pub(crate) struct Follow {
    receiver: watch::Receiver<Outcome>,
}

impl Follow {
    /// The leader's outcome; `None` when it went away without one.
    pub(crate) async fn settled(mut self) -> Option<Settled> {
        let seen = self.receiver.wait_for(Option::is_some).await.ok()?;
        seen.clone()
    }
}

pub(crate) enum Joined {
    Lead(Lead),
    Follow(Follow),
}

/// Keyed async mutexes and refresh flights. A key is remembered only while somebody holds or waits
/// for it (the tables hold weak references), so the tables do not grow with the people seen.
#[derive(Debug, Default)]
pub(crate) struct Flights {
    table: Mutex<HashMap<Key, Weak<tokio::sync::Mutex<()>>>>,
    refreshes: Mutex<HashMap<Key, Weak<watch::Sender<Outcome>>>>,
}

impl Flights {
    /// Waits for the key, then holds it until the guard is dropped.
    pub(crate) async fn acquire(
        &self,
        owner: &UserId,
        server: &UserToolServerId,
    ) -> OwnedMutexGuard<()> {
        let lock = {
            // A panic while the table was held left a map that is still whole.
            let mut table = self.table.lock().unwrap_or_else(PoisonError::into_inner);
            if table.len() >= PRUNE_AT {
                table.retain(|_, lock| lock.strong_count() > 0);
            }
            let key = (owner.clone(), server.clone());
            match table.get(&key).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(tokio::sync::Mutex::new(()));
                    table.insert(key, Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }

    /// Leads the refresh of the key, or follows the one that is running.
    pub(crate) fn join(&self, owner: &UserId, server: &UserToolServerId) -> Joined {
        let mut table = self
            .refreshes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if table.len() >= PRUNE_AT {
            table.retain(|_, sender| sender.strong_count() > 0);
        }
        let key = (owner.clone(), server.clone());
        if let Some(sender) = table.get(&key).and_then(Weak::upgrade) {
            return Joined::Follow(Follow {
                receiver: sender.subscribe(),
            });
        }
        let (sender, _) = watch::channel(None);
        let sender = Arc::new(sender);
        table.insert(key, Arc::downgrade(&sender));
        Joined::Lead(Lead { sender })
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.table
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    fn key(n: usize) -> (UserId, UserToolServerId) {
        (
            UserId::new(&format!("p{n}@example.test")),
            UserToolServerId::parse("u-0000000001").unwrap(),
        )
    }

    #[tokio::test]
    async fn one_holder_at_a_time_per_key() {
        let flights = Arc::new(Flights::default());
        let (owner, server) = key(0);
        let inside = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let (flights, inside) = (flights.clone(), inside.clone());
            let (owner, server) = (owner.clone(), server.clone());
            tasks.push(tokio::spawn(async move {
                let _guard = flights.acquire(&owner, &server).await;
                assert_eq!(inside.fetch_add(1, Ordering::SeqCst), 0, "two holders");
                tokio::time::sleep(Duration::from_millis(5)).await;
                inside.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn different_keys_do_not_wait_for_each_other() {
        let flights = Flights::default();
        let (a, b) = (key(1), key(2));
        let _first = flights.acquire(&a.0, &a.1).await;
        tokio::time::timeout(Duration::from_secs(1), flights.acquire(&b.0, &b.1))
            .await
            .expect("another key is free");
    }

    #[tokio::test]
    async fn followers_get_the_outcome_of_the_leader_and_a_later_caller_leads_again() {
        let flights = Flights::default();
        let (owner, server) = key(7);
        let Joined::Lead(lead) = flights.join(&owner, &server) else {
            panic!("the first caller leads");
        };
        let followers: Vec<_> = (0..3)
            .map(|_| match flights.join(&owner, &server) {
                Joined::Follow(follow) => tokio::spawn(follow.settled()),
                Joined::Lead(_) => panic!("a second leader"),
            })
            .collect();
        lead.settle(Settled::Unavailable {
            detail: "the token endpoint did not answer".to_owned(),
            refusal: None,
        });
        for follower in followers {
            let outcome = follower.await.unwrap().expect("an outcome");
            assert!(
                matches!(outcome, Settled::Unavailable { .. }),
                "{outcome:?}"
            );
        }
        // the flight is over: the next caller starts another
        assert!(matches!(flights.join(&owner, &server), Joined::Lead(_)));
    }

    #[tokio::test]
    async fn a_leader_that_goes_away_without_an_outcome_leaves_an_interruption() {
        let flights = Flights::default();
        let (owner, server) = key(8);
        let lead = match flights.join(&owner, &server) {
            Joined::Lead(lead) => lead,
            Joined::Follow(_) => panic!("the first caller leads"),
        };
        let Joined::Follow(follow) = flights.join(&owner, &server) else {
            panic!("a follower");
        };
        drop(lead);
        assert!(follow.settled().await.is_none());
    }

    #[test]
    fn a_failure_is_told_to_followers_with_its_refusal_and_without_its_source() {
        let leader: Result<CredentialLease, CredentialError> =
            Err(CredentialError::unavailable("discovery failed")
                .with_source(EgressRefusal::NotAllowed));
        let settled = Settled::of(&leader);
        let told = settled.error().expect("an error");
        assert!(matches!(told, CredentialError::Unavailable { .. }));
        assert_eq!(egress_refusal(&told), Some(EgressRefusal::NotAllowed));
        assert!(Settled::Refreshed.error().is_none());
        assert!(matches!(
            Settled::of(&Err(CredentialError::NeedsReauth)).error(),
            Some(CredentialError::NeedsReauth)
        ));
    }

    #[tokio::test]
    async fn the_table_forgets_keys_nobody_holds() {
        let flights = Flights::default();
        for n in 0..(PRUNE_AT * 3) {
            let (owner, server) = key(n);
            drop(flights.acquire(&owner, &server).await);
        }
        assert!(
            flights.len() <= PRUNE_AT,
            "the table grew to {}",
            flights.len()
        );
    }
}
