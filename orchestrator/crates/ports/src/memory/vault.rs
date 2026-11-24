//! The in-memory [`VaultStore`]: the port's contract, and its grace, without a database.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use jiff::Timestamp;
use orch_core::UserId;

use crate::Clock;
use crate::memory::FixedClock;
use crate::{
    SUPERSEDED_GRACE, SecretMaterial, SecretVersion, VaultError, VaultName, VaultRef, VaultStore,
};

/// One version of a name.
struct Entry {
    material: SecretMaterial,
    /// When a later write of the name superseded it; `None` while it is the current version.
    superseded: Option<Timestamp>,
}

/// A name, and every version of it still kept.
#[derive(Default)]
struct Versions {
    /// The greatest version the name has had: the next write is this plus one, and this survives a
    /// purge, so a version is never given twice.
    greatest: u64,
    kept: BTreeMap<u64, Entry>,
}

/// The secrets of every owner.
#[derive(Default)]
struct State {
    secrets: BTreeMap<(UserId, String), Versions>,
}

/// The in-memory [`VaultStore`]. Cheap to clone: clones share the secrets and the clock, so a test
/// keeps one and moves the clock under the store it gave away.
#[derive(Clone)]
pub struct MemoryVault {
    clock: FixedClock,
    state: Arc<Mutex<State>>,
}

impl MemoryVault {
    /// An empty vault whose grace runs on `clock`.
    pub fn new(clock: FixedClock) -> Self {
        MemoryVault {
            clock,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// The clock this vault's grace runs on, for a test that moves it.
    pub fn clock(&self) -> &FixedClock {
        &self.clock
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl VaultStore for MemoryVault {
    async fn put(
        &self,
        owner: &UserId,
        name: &VaultName,
        material: &SecretMaterial,
    ) -> Result<VaultRef, VaultError> {
        let now = self.clock.now();
        let mut state = self.state();
        let versions = state
            .secrets
            .entry((owner.clone(), name.as_str().to_owned()))
            .or_default();
        let version = versions
            .greatest
            .checked_add(1)
            .ok_or_else(|| VaultError::unavailable("the versions of that name ran out"))?;
        versions.greatest = version;
        for entry in versions.kept.values_mut() {
            if entry.superseded.is_none() {
                entry.superseded = Some(now);
            }
        }
        versions.kept.insert(
            version,
            Entry {
                material: material.clone(),
                superseded: None,
            },
        );
        Ok(VaultRef::new(name.clone(), SecretVersion::new(version)))
    }

    async fn get(
        &self,
        owner: &UserId,
        reference: &VaultRef,
    ) -> Result<SecretMaterial, VaultError> {
        let now = self.clock.now();
        let state = self.state();
        let Some(versions) = state
            .secrets
            .get(&(owner.clone(), reference.name().as_str().to_owned()))
        else {
            return Err(VaultError::UnknownName);
        };
        let Some(entry) = versions.kept.get(&reference.version().get()) else {
            return Err(VaultError::UnknownVersion);
        };
        if entry
            .superseded
            .is_some_and(|at| now.duration_since(at) >= SUPERSEDED_GRACE)
        {
            // still in the map, and deliberately: the answer is the same whether the row is there
            return Err(VaultError::UnknownVersion);
        }
        Ok(entry.material.clone())
    }

    async fn purge(&self) -> Result<u64, VaultError> {
        let now = self.clock.now();
        let mut state = self.state();
        let mut removed = 0_u64;
        for versions in state.secrets.values_mut() {
            let expired: Vec<u64> = versions
                .kept
                .iter()
                .filter(|(_, entry)| {
                    entry
                        .superseded
                        .is_some_and(|at| now.duration_since(at) >= SUPERSEDED_GRACE)
                })
                .map(|(version, _)| *version)
                .collect();
            for version in expired {
                versions.kept.remove(&version);
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl std::fmt::Debug for MemoryVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state();
        let names: usize = state.secrets.values().map(|v| v.kept.len()).sum();
        f.debug_struct("MemoryVault")
            .field("names", &state.secrets.len())
            .field("versions", &names)
            .field("now", &self.clock.now())
            .finish()
    }
}
