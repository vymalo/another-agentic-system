//! Single flight: one refresh at a time per `(owner, server)` in a process (ADR 0046, decision 5 of
//! the amendment of 2026-10-11). Across processes `SecretStore::rotate` settles it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use orch_core::UserId;
use orch_ports::UserToolServerId;
use tokio::sync::OwnedMutexGuard;

type Key = (UserId, UserToolServerId);

/// How many keys the table may hold before the entries nobody holds are dropped.
const PRUNE_AT: usize = 64;

/// A keyed async mutex. It remembers a key only while somebody holds or waits for it (the table
/// holds weak references), so it does not grow with the people it has seen.
#[derive(Debug, Default)]
pub(crate) struct Flights {
    table: Mutex<HashMap<Key, Weak<tokio::sync::Mutex<()>>>>,
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
