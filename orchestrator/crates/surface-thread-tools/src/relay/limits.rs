//! How many requests to people's servers run at once (ADR 0046, slice 5a review).
//!
//! A person's server is a stranger's, and every `tools/list` and `tools/call` to it holds a
//! connection, up to 4 MiB of buffered answer, and a task, for as long as the stranger takes. Left
//! unbounded, one person with eight servers and a patient agent could hold as many requests as the
//! agents can make; a lookup that never answers pins a blocking-pool thread (`getaddrinfo` cannot be
//! cancelled, only abandoned). Two semaphores bound it:
//!
//! - **per owner**: [`MAX_PER_OWNER`] (4) requests at once for one person's servers, so one person
//!   cannot take the process's share;
//! - **global**: [`MAX_IN_FLIGHT`] (16) at once for every person's servers together, which is what
//!   the process's memory and blocking pool are sized against (16 answers of 4 MiB, at the most).
//!
//! A request waits for a place for at most [`PERMIT_WAIT`] (10 s, for both together) and is then
//! `busy`: a listing leaves the server out, a call is a failed step that says so. Nothing is queued
//! beyond that wait. The owner's place is taken first, so a person who is over their share holds no
//! global place while they wait. A lease is taken **after** the places, so a request that waits holds
//! no credential.
//!
//! The numbers are constants a deployment cannot change yet (open item).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use orch_core::UserId;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

/// The most requests to people's servers in flight in one process.
pub const MAX_IN_FLIGHT: usize = 16;

/// The most requests to one person's servers in flight.
pub const MAX_PER_OWNER: usize = 4;

/// The longest a request waits for its places.
pub const PERMIT_WAIT: Duration = Duration::from_secs(10);

/// The places of a process: the global semaphore and one per owner with a request in flight.
pub(super) struct Limits {
    global: Arc<Semaphore>,
    per_owner: usize,
    wait: Duration,
    /// One semaphore per owner that has a request in flight or waiting. The entry is weak: the
    /// permits and the waiters keep the semaphore alive, and an owner with none is forgotten at the
    /// next request, so the map is as large as the number of owners active now.
    owners: Mutex<HashMap<UserId, Weak<Semaphore>>>,
}

/// The places a request holds until it is dropped.
#[derive(Debug)]
pub(super) struct Places {
    _owner: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}

/// The wait for a place ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Busy;

impl Limits {
    pub(super) fn new(global: usize, per_owner: usize, wait: Duration) -> Self {
        Limits {
            global: Arc::new(Semaphore::new(global)),
            per_owner,
            wait,
            owners: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn standard() -> Self {
        Self::new(MAX_IN_FLIGHT, MAX_PER_OWNER, PERMIT_WAIT)
    }

    /// The semaphore of `owner`, made if there is none alive.
    fn semaphore_of(&self, owner: &UserId) -> Arc<Semaphore> {
        let mut owners = self.owners.lock().unwrap_or_else(PoisonError::into_inner);
        owners.retain(|_, semaphore| semaphore.strong_count() > 0);
        if let Some(alive) = owners.get(owner).and_then(Weak::upgrade) {
            return alive;
        }
        let semaphore = Arc::new(Semaphore::new(self.per_owner));
        owners.insert(owner.clone(), Arc::downgrade(&semaphore));
        semaphore
    }

    /// The places for one request of `owner`: theirs first, then the process's, both within the
    /// wait. [`Busy`] when the wait runs out.
    pub(super) async fn acquire(&self, owner: &UserId) -> Result<Places, Busy> {
        let deadline = Instant::now() + self.wait;
        let semaphore = self.semaphore_of(owner);
        let own = tokio::time::timeout_at(deadline, semaphore.acquire_owned())
            .await
            .map_err(|_| Busy)?
            .map_err(|_| Busy)?;
        let global = tokio::time::timeout_at(deadline, Arc::clone(&self.global).acquire_owned())
            .await
            .map_err(|_| Busy)?
            .map_err(|_| Busy)?;
        Ok(Places {
            _owner: own,
            _global: global,
        })
    }

    /// How many owners the map remembers (a test).
    #[cfg(test)]
    fn owners_remembered(&self) -> usize {
        let mut owners = self.owners.lock().unwrap_or_else(PoisonError::into_inner);
        owners.retain(|_, semaphore| semaphore.strong_count() > 0);
        owners.len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn user(name: &str) -> UserId {
        UserId::new(&format!("{name}@example.com"))
    }

    #[tokio::test]
    async fn an_owner_is_held_to_their_share_and_the_process_to_its_own() {
        let limits = Limits::new(3, 2, Duration::from_millis(50));
        let (alice, bob) = (user("alice"), user("bob"));
        let a1 = limits.acquire(&alice).await.unwrap();
        let _a2 = limits.acquire(&alice).await.unwrap();
        // a third request of Alice's waits for her own place and is busy: she holds two
        assert_eq!(limits.acquire(&alice).await.unwrap_err(), Busy);
        // Bob has his own share, and takes the last global place
        let _b1 = limits.acquire(&bob).await.unwrap();
        // the process is full now: Bob's second request is busy though he holds one of two
        assert_eq!(limits.acquire(&bob).await.unwrap_err(), Busy);
        // a place freed is taken at once
        drop(a1);
        let _b2 = limits.acquire(&bob).await.unwrap();
    }

    #[tokio::test]
    async fn a_request_that_waits_gets_the_place_a_finished_one_frees() {
        let limits = Arc::new(Limits::new(8, 1, Duration::from_secs(5)));
        let alice = user("alice");
        let first = limits.acquire(&alice).await.unwrap();
        let waiting = {
            let limits = Arc::clone(&limits);
            let alice = alice.clone();
            tokio::spawn(async move { limits.acquire(&alice).await.map(|_| ()) })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!waiting.is_finished(), "the second request waits");
        drop(first);
        assert_eq!(waiting.await.unwrap(), Ok(()));
    }

    #[tokio::test]
    async fn an_owner_with_nothing_in_flight_is_forgotten() {
        let limits = Limits::new(8, 2, Duration::from_millis(50));
        let places = limits.acquire(&user("alice")).await.unwrap();
        let _bob = limits.acquire(&user("bob")).await.unwrap();
        assert_eq!(limits.owners_remembered(), 2);
        drop(places);
        assert_eq!(limits.owners_remembered(), 1);
    }
}
