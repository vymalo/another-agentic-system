//! Runs the `ArtifactStore` conformance testkit against the in-memory store, and pins what the
//! in-memory store adds to it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_ports::memory::MemoryArtifacts;
use orch_ports::testkit::artifact_store::{Downloaded, PresignFixture};

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
    let browser = store.clone();
    Some(PresignFixture {
        store,
        upload: std::sync::Arc::new(move |put, bytes| {
            let bucket = bucket.clone();
            Box::pin(async move { bucket.upload(&put, bytes, jiff::Timestamp::now()) })
        }),
        download: std::sync::Arc::new(move |get| {
            let browser = browser.clone();
            Box::pin(async move {
                let got = browser.download(&get, jiff::Timestamp::now())?;
                Ok(Downloaded {
                    bytes: got.bytes,
                    content_disposition: got.content_disposition,
                    content_type: Some(got.content_type),
                    cache_control: got.cache_control,
                })
            })
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

/// The double plays the bucket's side of a presigned download: it refuses a URL once it has
/// expired, a URL that is not its own and a member nobody signed, and a store that does not presign
/// plays nothing.
#[tokio::test]
async fn the_memory_bucket_refuses_an_expired_url_and_one_that_is_not_its_own() {
    use std::time::Duration;

    use orch_core::ThreadId;
    use orch_ports::{ArtifactMeta, ArtifactStore, PresignedGet, ResponseOverrides};

    let store = MemoryArtifacts::presigning();
    let meta = ArtifactMeta::of("text/plain", Some("a.txt".into()), b"hello");
    let key = meta.key(ThreadId(uuid::Uuid::now_v7()));
    store
        .put(&key, bytes::Bytes::from_static(b"hello"), &meta)
        .await
        .unwrap();
    let get = store
        .presign_get(
            &key,
            &ResponseOverrides {
                content_disposition: Some("attachment; filename=\"a b.txt\"".into()),
                ..ResponseOverrides::default()
            },
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert!(get.url.starts_with("memory://bucket/threads/"), "{get:?}");

    let now = jiff::Timestamp::now();
    let served = store.download(&get, now).unwrap();
    assert_eq!(served.bytes, "hello");
    assert_eq!(
        served.content_disposition.as_deref(),
        Some("attachment; filename=\"a b.txt\"")
    );
    // the type falls back to the one the object was kept with; nothing else is invented
    assert_eq!(served.content_type, "text/plain");
    assert_eq!(served.cache_control, None);

    // at the second the URL stops working, and after it
    assert!(store.download(&get, get.expires_at).is_err());
    assert!(
        store
            .download(&get, get.expires_at + Duration::from_secs(1))
            .is_err()
    );
    // a URL that is not this bucket's, and one with a member nobody signed
    let foreign = PresignedGet {
        url: get.url.replace("memory://bucket/", "https://elsewhere/"),
        expires_at: get.expires_at,
    };
    assert!(store.download(&foreign, now).is_err());
    let extra = PresignedGet {
        url: format!("{}&x-amz-acl=public-read", get.url),
        expires_at: get.expires_at,
    };
    assert!(store.download(&extra, now).is_err());

    // a store that does not presign signs and plays nothing
    let plain = MemoryArtifacts::new();
    assert!(plain.download(&get, now).is_err());
    assert!(matches!(
        plain
            .presign_get(&key, &ResponseOverrides::default(), Duration::from_secs(60))
            .await,
        Err(orch_ports::ArtifactError::Unsupported)
    ));
}
