//! Runs the `ArtifactStore` conformance testkit against the in-memory store, and pins what the
//! in-memory store adds to it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_ports::memory::MemoryArtifacts;
use orch_ports::testkit::artifact_store::PresignFixture;

async fn make() -> Option<MemoryArtifacts> {
    Some(MemoryArtifacts::new())
}

orch_ports::artifact_store_conformance!(make);

mod without_presign {
    use super::*;

    orch_ports::artifact_no_presign_conformance!(make);
}

/// The in-memory store that presigns, with an agent that plays the bucket's side of the `PUT`.
async fn presigning() -> Option<PresignFixture<MemoryArtifacts>> {
    fixture(MemoryArtifacts::presigning())
}

fn fixture(store: MemoryArtifacts) -> Option<PresignFixture<MemoryArtifacts>> {
    let bucket = store.clone();
    Some(PresignFixture {
        store,
        upload: std::sync::Arc::new(move |put, bytes| {
            let bucket = bucket.clone();
            Box::pin(async move { bucket.upload(&put, bytes, jiff::Timestamp::now()) })
        }),
    })
}

/// A bucket that does not enforce the signed checksum takes the wrong bytes; the store must not
/// vouch for them (its `head` shows what is really there).
mod with_a_lenient_bucket {
    use super::*;

    async fn lenient() -> Option<PresignFixture<MemoryArtifacts>> {
        fixture(MemoryArtifacts::presigning().lenient())
    }

    orch_ports::artifact_presign_conformance!(lenient);
}

/// A bucket that does not say the checksum of an object, and does not enforce it either: no
/// checksum is vouched for, which the caller reads as "hash the file yourself".
mod with_a_bucket_that_says_no_checksum {
    use super::*;

    async fn silent() -> Option<PresignFixture<MemoryArtifacts>> {
        fixture(MemoryArtifacts::presigning().lenient().without_checksums())
    }

    orch_ports::artifact_presign_conformance!(silent);
}

mod with_presign {
    use super::*;

    orch_ports::artifact_presign_conformance!(presigning);

    async fn store() -> Option<MemoryArtifacts> {
        Some(MemoryArtifacts::presigning())
    }

    // a presigning store is still a store: the whole suite passes on it
    orch_ports::artifact_store_conformance!(store);
}

#[tokio::test]
async fn clones_share_the_files_and_the_store_can_be_looked_into() {
    use orch_core::ThreadId;
    use orch_ports::{ArtifactMeta, ArtifactStore};

    let store = MemoryArtifacts::new();
    let clone = store.clone();
    assert!(store.is_empty());
    let meta = ArtifactMeta::of("text/plain", None, b"hello");
    let key = meta.key(ThreadId(uuid::Uuid::now_v7()));
    clone
        .put(&key, bytes::Bytes::from_static(b"hello"), &meta)
        .await
        .unwrap();
    assert!(store.contains(&key));
    assert_eq!(store.len(), 1);
    store.delete(&key).await.unwrap();
    assert!(clone.is_empty());
}
