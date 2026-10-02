//! An in-memory artifact store: the reference implementation of the conformance suite, and what a
//! test of the application puts in the bundle when it needs files to be kept.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;

use crate::{ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream, stream_of};

type Objects = HashMap<ArtifactKey, (ArtifactMeta, Bytes)>;

/// The in-memory [`ArtifactStore`]. Cheap to clone: clones share the files, so a test keeps one to
/// look into while the application holds another.
#[derive(Debug, Clone, Default)]
pub struct MemoryArtifacts {
    objects: Arc<Mutex<Objects>>,
}

impl MemoryArtifacts {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Objects> {
        // A test that panicked while holding the lock left a map that is still whole.
        self.objects.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many files are kept.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether no file is kept.
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    /// Whether a file is kept under `key`.
    pub fn contains(&self, key: &ArtifactKey) -> bool {
        self.lock().contains_key(key)
    }
}

impl ArtifactStore for MemoryArtifacts {
    async fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
        meta.check(key, &bytes)?;
        self.lock().insert(*key, (meta.clone(), bytes));
        Ok(())
    }

    async fn get(
        &self,
        key: &ArtifactKey,
    ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
        let found = self.lock().get(key).cloned();
        Ok(found.map(|(meta, bytes)| (meta, stream_of(bytes))))
    }

    async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
        self.lock().remove(key);
        Ok(())
    }
}
