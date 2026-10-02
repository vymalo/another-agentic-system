//! The `ArtifactStore` testkit against the S3 adapter, twice: over real HTTP against an in-process
//! S3 stub (always), and against an S3-compatible server when `ORCH_TEST_S3_URL` names one (CI or a
//! developer's MinIO); and what only this adapter says: the requests it makes, the metadata it
//! writes, how a refusal and a failing server are told apart, and that no credential is shown.
//!
//! The server for the second run, for example (the bucket has to exist; the objects the run leaves
//! are under a prefix of their own):
//!
//! ```text
//! docker run -d -p 9000:9000 -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
//!     minio/minio server /data
//! mc alias set local http://127.0.0.1:9000 minioadmin minioadmin && mc mb local/orch-test
//! ORCH_TEST_S3_URL=http://127.0.0.1:9000 cargo test -p orch-artifacts-s3
//! ```
//!
//! `ORCH_TEST_S3_BUCKET` (default `orch-test`), `ORCH_TEST_S3_REGION` (default `us-east-1`),
//! `ORCH_TEST_S3_ACCESS_KEY` and `ORCH_TEST_S3_SECRET_KEY` (default `minioadmin`) complete it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt as _;
use orch_artifacts_s3::{S3Artifacts, S3Config};
use orch_core::{Classify as _, ErrorClass, ThreadId};
use orch_ports::{ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore};
use secrecy::SecretString;
use support::{ACCESS_KEY_ID, BUCKET, Mode, SECRET_ACCESS_KEY, Stub};

fn config(stub: &Stub) -> S3Config {
    S3Config::new(BUCKET)
        .with_endpoint(stub.endpoint())
        .with_credentials(
            SecretString::from(ACCESS_KEY_ID),
            SecretString::from(SECRET_ACCESS_KEY),
        )
        .with_timeout(Duration::from_secs(20))
}

mod against_the_stub {
    use super::*;

    async fn make() -> Option<S3Artifacts> {
        let stub = Stub::start().await;
        Some(S3Artifacts::new(config(&stub)).unwrap())
    }

    orch_ports::artifact_store_conformance!(make);
}

mod against_a_server {
    use super::*;

    async fn make() -> Option<S3Artifacts> {
        let Ok(url) = std::env::var("ORCH_TEST_S3_URL") else {
            eprintln!("skipped: no S3-compatible server (ORCH_TEST_S3_URL unset)");
            return None;
        };
        let var =
            |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
        let config = S3Config::new(var("ORCH_TEST_S3_BUCKET", "orch-test"))
            .with_region(var("ORCH_TEST_S3_REGION", "us-east-1"))
            .with_endpoint(url)
            .with_prefix(format!("orch-test/{}", uuid::Uuid::now_v7()))
            .with_credentials(
                SecretString::from(var("ORCH_TEST_S3_ACCESS_KEY", "minioadmin")),
                SecretString::from(var("ORCH_TEST_S3_SECRET_KEY", "minioadmin")),
            );
        Some(S3Artifacts::new(config).unwrap())
    }

    orch_ports::artifact_store_conformance!(make);
}

fn thread() -> ThreadId {
    ThreadId(uuid::Uuid::now_v7())
}

fn file(name: Option<&str>) -> (ArtifactKey, ArtifactMeta, Bytes) {
    let bytes = b"<svg xmlns='http://www.w3.org/2000/svg'/>";
    let meta = ArtifactMeta::of("image/svg+xml", name.map(str::to_owned), bytes);
    (meta.key(thread()), meta, Bytes::from_static(bytes))
}

fn object_key(prefix: &str, key: &ArtifactKey) -> String {
    format!("{prefix}threads/{}/{}", key.thread(), key.sha256_hex())
}

async fn drain(store: &S3Artifacts, key: &ArtifactKey) -> Vec<u8> {
    let (_, mut stream) = store.get(key).await.unwrap().unwrap();
    let mut all = Vec::new();
    while let Some(piece) = stream.next().await {
        all.extend_from_slice(&piece.unwrap());
    }
    all
}

#[tokio::test]
async fn a_put_is_one_signed_request_with_the_meta_as_headers() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub).with_prefix("files/prod")).unwrap();
    let (key, meta, bytes) = file(Some("Résumé 100%.svg"));
    store.put(&key, bytes.clone(), &meta).await.unwrap();

    let seen = stub.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let put = &seen[0];
    assert_eq!(put.method, "PUT");
    assert_eq!(
        put.path,
        format!("/{BUCKET}/{}", object_key("files/prod/", &key))
    );
    assert_eq!(put.access_key_id.as_deref(), Some(ACCESS_KEY_ID));
    let header = |name: &str| {
        put.headers
            .get(name)
            .map(|v| v.to_str().unwrap().to_owned())
    };
    assert_eq!(header("content-type").as_deref(), Some("image/svg+xml"));
    assert_eq!(header("x-amz-meta-sha256"), Some(key.sha256_hex()));
    // a header value is ASCII: the name is percent-encoded
    let name = header("x-amz-meta-filename").unwrap();
    assert!(
        name.is_ascii() && !name.contains(' ') && name.contains("%C3%A9"),
        "{name}"
    );

    let stored = stub.object(&object_key("files/prod/", &key)).unwrap();
    assert_eq!(stored.body, bytes);
    assert_eq!(stub.keys(), [object_key("files/prod/", &key)]);
}

#[tokio::test]
async fn a_file_without_a_name_has_no_name_header() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    let (key, meta, bytes) = file(None);
    store.put(&key, bytes, &meta).await.unwrap();
    let stored = stub.object(&object_key("", &key)).unwrap();
    assert!(
        stored
            .headers
            .iter()
            .all(|(n, _)| n != "x-amz-meta-filename"),
        "{:?}",
        stored.headers
    );
    assert_eq!(store.get(&key).await.unwrap().unwrap().0.filename, None);
}

#[tokio::test]
async fn an_object_written_by_someone_else_reads_with_what_it_has() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    let (key, meta, bytes) = file(None);
    // content type and no user metadata at all
    stub.insert(
        &object_key("", &key),
        &bytes,
        &[("content-type", "image/svg+xml")],
    );
    let (got, _) = store.get(&key).await.unwrap().unwrap();
    assert_eq!(got, meta);
    assert_eq!(drain(&store, &key).await, bytes.to_vec());
}

#[tokio::test]
async fn an_object_that_is_not_what_its_key_says_is_corrupt_and_never_served() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    let (key, _, bytes) = file(None);
    let other = "0".repeat(64);
    let own = key.sha256_hex();
    for (headers, what) in [
        (
            vec![
                ("content-type", "image/svg+xml"),
                ("x-amz-meta-sha256", other.as_str()),
            ],
            "another hash",
        ),
        (vec![("x-amz-meta-sha256", own.as_str())], "no content type"),
        (
            vec![
                ("content-type", "image/svg+xml"),
                ("x-amz-meta-filename", "%FF%FE"),
            ],
            "a name that is not UTF-8",
        ),
    ] {
        stub.insert(&object_key("", &key), &bytes, &headers);
        let err = store.get(&key).await.err().expect(what);
        assert!(matches!(err, ArtifactError::Corrupt(_)), "{what}: {err}");
        assert_eq!(err.class(), ErrorClass::Corrupt);
    }
}

#[tokio::test]
async fn refused_credentials_are_unauthenticated_and_shown_nowhere() {
    let stub = Stub::start().await;
    let wrong = config(&stub).with_credentials(
        SecretString::from("AKIDWRONGEXAMPLE"),
        SecretString::from("wrong-secret-access-key-987654321"),
    );
    let store = S3Artifacts::new(wrong.clone()).unwrap();
    let (key, meta, bytes) = file(None);

    let put = store.put(&key, bytes, &meta).await.unwrap_err();
    let get = store.get(&key).await.err().unwrap();
    let delete = store.delete(&key).await.unwrap_err();
    for err in [put, get, delete] {
        assert!(matches!(err, ArtifactError::Unauthenticated), "{err}");
        assert_eq!(err.class(), ErrorClass::Unauthenticated);
        assert!(!err.is_retryable());
        let shown = format!("{err} {err:?}");
        for secret in [
            "AKIDWRONGEXAMPLE",
            "wrong-secret-access-key",
            SECRET_ACCESS_KEY,
        ] {
            assert!(!shown.contains(secret), "{shown}");
        }
    }
    let shown = format!("{wrong:?} {store:?}");
    assert!(
        !shown.contains("AKIDWRONGEXAMPLE") && !shown.contains("wrong-secret"),
        "{shown}"
    );
    assert!(stub.keys().is_empty());
}

#[tokio::test]
async fn a_server_that_fails_is_transient_and_the_error_has_no_credential() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    let (key, meta, bytes) = file(None);
    store.put(&key, bytes.clone(), &meta).await.unwrap();

    stub.set_mode(Mode::Fail(axum::http::StatusCode::INTERNAL_SERVER_ERROR));
    let started = std::time::Instant::now();
    let put = store.put(&key, bytes, &meta).await.unwrap_err();
    let get = store.get(&key).await.err().unwrap();
    for err in [put, get] {
        assert!(matches!(err, ArtifactError::Unavailable { .. }), "{err}");
        assert_eq!(err.class(), ErrorClass::Transient);
        assert!(err.is_retryable());
        let shown = format!("{err} {err:?}");
        assert!(
            !shown.contains(ACCESS_KEY_ID) && !shown.contains(SECRET_ACCESS_KEY),
            "{shown}"
        );
        // the text of the error does not carry the endpoint: it goes to a log with the source
        assert!(!err.to_string().contains("127.0.0.1"), "{err}");
    }
    assert!(
        started.elapsed() < Duration::from_secs(40),
        "the retries are bounded"
    );

    stub.set_mode(Mode::Up);
    assert!(store.get(&key).await.unwrap().is_some(), "it recovers");
}

#[tokio::test]
async fn a_server_nobody_listens_on_is_transient() {
    // a port that was free a moment ago
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let store = S3Artifacts::new(
        S3Config::new(BUCKET)
            .with_endpoint(format!("http://127.0.0.1:{port}"))
            .with_credentials(
                SecretString::from(ACCESS_KEY_ID),
                SecretString::from(SECRET_ACCESS_KEY),
            )
            .with_timeout(Duration::from_secs(5)),
    )
    .unwrap();
    let (key, meta, bytes) = file(None);
    let err = store.put(&key, bytes, &meta).await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Transient, "{err}");
    let err = store.get(&key).await.err().unwrap();
    assert_eq!(err.class(), ErrorClass::Transient, "{err}");
}

#[tokio::test]
async fn a_put_that_is_refused_before_it_is_sent_sends_nothing() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    let (key, meta, _) = file(None);
    let err = store
        .put(&key, Bytes::from_static(b"not the content"), &meta)
        .await
        .unwrap_err();
    assert!(matches!(err, ArtifactError::Invalid(_)), "{err}");
    assert!(stub.seen().is_empty(), "{:?}", stub.seen());
}

#[tokio::test]
async fn a_store_is_built_without_connecting_and_needs_credentials() {
    // an endpoint that does not exist: building does not touch the network
    let config = S3Config::new("bucket")
        .with_endpoint("http://does-not-exist.invalid:9000")
        .with_credentials(SecretString::from("a"), SecretString::from("b"));
    S3Artifacts::new(config).unwrap();
    assert!(S3Artifacts::new(S3Config::new("bucket")).is_err());
}
