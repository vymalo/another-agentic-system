//! Runs the `ArtifactStore` conformance testkit against the in-memory store, and pins what the
//! in-memory store adds to it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_ports::memory::MemoryArtifacts;

async fn make() -> Option<MemoryArtifacts> {
    Some(MemoryArtifacts::new())
}

orch_ports::artifact_store_conformance!(make);

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
