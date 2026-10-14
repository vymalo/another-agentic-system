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
//!
//! Presigned uploads (ADR 0058) are run the same two ways: the stub checks that a presigned request
//! carries every header it signed and enforces the signed checksum as S3 does; the server (the URL
//! is also the public endpoint, `http://127.0.0.1:9000` for the stack of `compose.yaml`) is the
//! proof that a real one takes the declared bytes and refuses or never vouches for other ones.
//!
//! Presigned downloads (ADR 0058 A1) are the same: the stub answers a presigned `GET` with the
//! signed `response-*` members as the headers of the answer, and the server proves that it holds
//! the URL to them (a changed, removed or added member, another key, method or time is refused).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt as _;
use orch_artifacts_s3::{S3Artifacts, S3Config};
use orch_core::{Classify as _, ErrorClass, ThreadId};
use orch_ports::testkit::artifact_store::{Downloaded, Downloader, PresignFixture, Uploader};
use orch_ports::{ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ResponseOverrides};
use secrecy::SecretString;
use support::{ACCESS_KEY_ID, BUCKET, Checksums, Mode, SECRET_ACCESS_KEY, Stub};

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

/// An agent's `PUT`: the URL, every header the answer lists, the bytes. The bucket's answer is
/// `Ok` for a success and the status for anything else.
fn upload_over_http() -> Uploader {
    let client = reqwest::Client::new();
    Arc::new(move |put, bytes| {
        let client = client.clone();
        Box::pin(async move {
            let mut request = client.put(&put.url);
            for (name, value) in &put.headers {
                request = request.header(name, value);
            }
            let response = request
                .body(bytes)
                .send()
                .await
                .map_err(|e| e.without_url().to_string())?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(format!("HTTP {}", response.status()))
            }
        })
    })
}

/// A browser's `GET` of a presigned download: the URL alone (no credential, no header). The
/// bucket's answer is the bytes and the three headers the signature may have changed, or the status
/// of a refusal.
fn download_over_http() -> Downloader {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    Arc::new(move |get| {
        let client = client.clone();
        Box::pin(async move {
            let response = client
                .get(&get.url)
                .send()
                .await
                .map_err(|e| e.without_url().to_string())?;
            if !response.status().is_success() {
                return Err(format!("HTTP {}", response.status()));
            }
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let (content_disposition, content_type, cache_control) = (
                header("content-disposition"),
                header("content-type"),
                header("cache-control"),
            );
            let bytes = response
                .bytes()
                .await
                .map_err(|e| e.without_url().to_string())?;
            Ok(Downloaded {
                bytes,
                content_disposition,
                content_type,
                cache_control,
            })
        })
    })
}

mod against_the_stub_that_presigns {
    use super::*;

    async fn fixture() -> Option<PresignFixture<S3Artifacts>> {
        let stub = Stub::start().await;
        let store = S3Artifacts::new(config(&stub).with_public_endpoint(stub.endpoint())).unwrap();
        Some(PresignFixture {
            store,
            upload: upload_over_http(),
            download: download_over_http(),
        })
    }

    orch_ports::artifact_presign_conformance!(fixture);
}

/// A server that does not enforce the signed checksum but stores the one it computed: the bytes
/// are taken, and `head` does not vouch for them.
mod against_a_stub_that_does_not_enforce_checksums {
    use super::*;

    async fn fixture() -> Option<PresignFixture<S3Artifacts>> {
        let stub = Stub::start().await;
        stub.set_checksums(Checksums {
            enforce: false,
            report: true,
        });
        let store = S3Artifacts::new(config(&stub).with_public_endpoint(stub.endpoint())).unwrap();
        Some(PresignFixture {
            store,
            upload: upload_over_http(),
            download: download_over_http(),
        })
    }

    orch_ports::artifact_presign_conformance!(fixture);
}

/// A server that never says a checksum: `head` has none, which the caller reads as "hash the file".
mod against_a_stub_that_says_no_checksum {
    use super::*;

    async fn fixture() -> Option<PresignFixture<S3Artifacts>> {
        let stub = Stub::start().await;
        stub.set_checksums(Checksums {
            enforce: true,
            report: false,
        });
        let store = S3Artifacts::new(config(&stub).with_public_endpoint(stub.endpoint())).unwrap();
        Some(PresignFixture {
            store,
            upload: upload_over_http(),
            download: download_over_http(),
        })
    }

    orch_ports::artifact_presign_conformance!(fixture);
}

mod against_the_stub_without_a_public_endpoint {
    use super::*;

    async fn make() -> Option<S3Artifacts> {
        let stub = Stub::start().await;
        Some(S3Artifacts::new(config(&stub)).unwrap())
    }

    orch_ports::artifact_no_presign_conformance!(make);
}

mod against_a_server_that_presigns {
    use super::*;

    async fn fixture() -> Option<PresignFixture<S3Artifacts>> {
        let Ok(url) = std::env::var("ORCH_TEST_S3_URL") else {
            eprintln!("skipped: no S3-compatible server (ORCH_TEST_S3_URL unset)");
            return None;
        };
        let var =
            |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
        let config = S3Config::new(var("ORCH_TEST_S3_BUCKET", "orch-test"))
            .with_region(var("ORCH_TEST_S3_REGION", "us-east-1"))
            .with_endpoint(url.clone())
            // the URL a test reaches is the public one too
            .with_public_endpoint(url)
            .with_prefix(format!("orch-test/{}", uuid::Uuid::now_v7()))
            .with_credentials(
                SecretString::from(var("ORCH_TEST_S3_ACCESS_KEY", "minioadmin")),
                SecretString::from(var("ORCH_TEST_S3_SECRET_KEY", "minioadmin")),
            );
        Some(PresignFixture {
            store: S3Artifacts::new(config).unwrap(),
            upload: upload_over_http(),
            download: download_over_http(),
        })
    }

    orch_ports::artifact_presign_conformance!(fixture);

    /// What this server says on a `HEAD` that asks for the checksum, printed for the one who reads
    /// the run: the ingest uses it when the server says it and hashes the object when it does not.
    #[tokio::test]
    async fn what_the_server_says_of_a_checksum_is_the_right_one_or_nothing() {
        let Some(PresignFixture { store, upload, .. }) = fixture().await else {
            return;
        };
        let (key, meta, bytes) = file(Some("chart.svg"));
        let put = store
            .presign_put(&key, &meta, Duration::from_secs(300))
            .await
            .unwrap();
        upload(put, bytes).await.unwrap();
        let head = store.head(&key).await.unwrap().unwrap();
        eprintln!(
            "server head: size {}, type {:?}, checksum {}, last modified {:?}",
            head.size,
            head.media_type,
            head.sha256.map_or("not said", |_| "said"),
            head.last_modified
        );
        assert_eq!(head.size, meta.size);
        assert!(head.sha256.is_none_or(|h| h == meta.sha256));
    }
}

/// What a **real** server holds a presigned `PUT` to (ADR 0058): the signature binds the method, the
/// key, the headers it names and nothing else, and the server must refuse what the signature does
/// not cover. The stub cannot say this (it is ours); only a server that was written by somebody else
/// can, so these cases run only with `ORCH_TEST_S3_URL` and are the proof the design leans on:
/// `content-length` cannot be signed, so everything else must be.
///
/// Each case sends a request an agent could send with the slot it was given and one thing changed,
/// and asserts it is refused (a 4xx) **and** that it wrote nothing: an object is never left under
/// the key by a refused request, and the bytes of another object never end up under it.
mod a_real_server_holds_a_presigned_put_to_what_was_signed {
    use super::*;

    pub(super) struct Server {
        pub(super) store: S3Artifacts,
        pub(super) bucket: String,
        pub(super) prefix: String,
        pub(super) client: reqwest::Client,
    }

    pub(super) fn server() -> Option<Server> {
        let Ok(url) = std::env::var("ORCH_TEST_S3_URL") else {
            eprintln!("skipped: no S3-compatible server (ORCH_TEST_S3_URL unset)");
            return None;
        };
        let var =
            |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.to_owned());
        let bucket = var("ORCH_TEST_S3_BUCKET", "orch-test");
        let prefix = format!("orch-test/{}", uuid::Uuid::now_v7());
        let config = S3Config::new(bucket.clone())
            .with_region(var("ORCH_TEST_S3_REGION", "us-east-1"))
            .with_endpoint(url.clone())
            .with_public_endpoint(url)
            .with_prefix(prefix.clone())
            .with_credentials(
                SecretString::from(var("ORCH_TEST_S3_ACCESS_KEY", "minioadmin")),
                SecretString::from(var("ORCH_TEST_S3_SECRET_KEY", "minioadmin")),
            );
        Some(Server {
            store: S3Artifacts::new(config).unwrap(),
            bucket,
            prefix,
            client: reqwest::Client::new(),
        })
    }

    /// What the server answered: the status and the body as text.
    struct Answer {
        status: u16,
        body: Vec<u8>,
    }

    impl Server {
        /// The object name of `key` as the server sees it, the way `x-amz-copy-source` names it.
        fn source_of(&self, key: &ArtifactKey) -> String {
            format!(
                "{}/{}/threads/{}/{}",
                self.bucket,
                self.prefix,
                key.thread(),
                key.sha256_hex()
            )
        }

        /// The agent's request with the slot as given, then `change` applied to it.
        async fn send(
            &self,
            method: reqwest::Method,
            url: &str,
            headers: Vec<(String, String)>,
            body: Bytes,
        ) -> Answer {
            let mut request = self.client.request(method, url);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let response = request.body(body).send().await.unwrap();
            let status = response.status().as_u16();
            let body = response.bytes().await.unwrap().to_vec();
            Answer { status, body }
        }

        async fn slot(&self, key: &ArtifactKey, meta: &ArtifactMeta) -> orch_ports::PresignedPut {
            self.store
                .presign_put(key, meta, Duration::from_secs(300))
                .await
                .unwrap()
        }
    }

    fn headers_of(put: &orch_ports::PresignedPut) -> Vec<(String, String)> {
        put.headers
            .iter()
            .map(|(n, v)| (n.clone(), v.clone()))
            .collect()
    }

    fn refused(answer: &Answer, what: &str) {
        eprintln!(
            "{what}: HTTP {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body[..answer.body.len().min(160)])
        );
        assert!(
            (400..500).contains(&answer.status),
            "{what}: the server must refuse it with a 4xx, it answered {}",
            answer.status
        );
    }

    /// The control: the slot as given, to the letter, is taken. Without it a refusal above could be
    /// a server that refuses everything.
    async fn the_slot_as_given_is_taken(
        server: &Server,
        key: &ArtifactKey,
        put: &orch_ports::PresignedPut,
        bytes: &Bytes,
    ) {
        let answer = server
            .send(
                reqwest::Method::PUT,
                &put.url,
                headers_of(put),
                bytes.clone(),
            )
            .await;
        assert!(
            (200..300).contains(&answer.status),
            "the control: the slot as given is taken, it answered {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let head = server.store.head(key).await.unwrap().expect("it is there");
        assert_eq!(head.size, bytes.len() as u64);
    }

    #[tokio::test]
    async fn a_header_the_signature_does_not_cover_is_refused() {
        let Some(server) = server() else { return };
        let (key, meta, bytes) = declared(Some("report.pdf"));
        let put = server.slot(&key, &meta).await;
        // `x-amz-*` headers an agent adds to the slot it was given: each changes what the object
        // is or where it is kept, and none is among the signed headers
        for (name, value) in [
            ("x-amz-acl", "public-read"),
            ("x-amz-meta-extra", "1"),
            ("x-amz-storage-class", "REDUCED_REDUNDANCY"),
            ("x-amz-server-side-encryption", "AES256"),
            ("x-amz-tagging", "a=b"),
        ] {
            let mut headers = headers_of(&put);
            headers.push((name.to_owned(), value.to_owned()));
            let answer = server
                .send(reqwest::Method::PUT, &put.url, headers, bytes.clone())
                .await;
            refused(&answer, name);
            assert!(
                server.store.head(&key).await.unwrap().is_none(),
                "{name}: a refused request leaves nothing under the key"
            );
        }
        the_slot_as_given_is_taken(&server, &key, &put, &bytes).await;
    }

    #[tokio::test]
    async fn a_copy_source_turns_nothing_under_the_key_into_another_object() {
        let Some(server) = server() else { return };
        // another thread's object, in the same bucket and prefix
        let other_bytes = Bytes::from_static(b"%PDF-1.7 another thread's secret report");
        let other_meta = ArtifactMeta::of("application/pdf", None, &other_bytes);
        let other = other_meta.key(thread());
        server
            .store
            .put(&other, other_bytes.clone(), &other_meta)
            .await
            .unwrap();

        let (key, meta, bytes) = declared(Some("report.pdf"));
        assert_ne!(key.sha256_hex(), other.sha256_hex());
        let put = server.slot(&key, &meta).await;
        // a PUT with `x-amz-copy-source` is S3's CopyObject: it reads the source with the
        // server's rights, so it must not be possible with a URL that does not sign it. Sent once
        // with the declared bytes and once with none (the shape a CopyObject has).
        for (what, body) in [
            ("with the bytes", bytes.clone()),
            ("with no body", Bytes::new()),
        ] {
            let mut headers = headers_of(&put);
            headers.push(("x-amz-copy-source".to_owned(), server.source_of(&other)));
            let answer = server
                .send(reqwest::Method::PUT, &put.url, headers, body)
                .await;
            refused(&answer, &format!("x-amz-copy-source {what}"));
            match server.store.head(&key).await.unwrap() {
                None => {}
                Some(head) => {
                    // whatever the server did, the key does not hold the other object
                    assert_ne!(
                        head.sha256,
                        Some(other_meta.sha256),
                        "{what}: the key holds another thread's object"
                    );
                    assert_ne!(head.size, other_meta.size, "{what}");
                }
            }
        }
        // the other object is untouched, and nothing under this key is the other object
        assert_eq!(drain(&server.store, &other).await, other_bytes.to_vec());
        if server.store.head(&key).await.unwrap().is_some() {
            assert_ne!(drain(&server.store, &key).await, other_bytes.to_vec());
        }
        the_slot_as_given_is_taken(&server, &key, &put, &bytes).await;
    }

    #[tokio::test]
    async fn another_content_type_than_the_one_signed_is_refused() {
        let Some(server) = server() else { return };
        let (key, meta, bytes) = declared(Some("report.pdf"));
        let put = server.slot(&key, &meta).await;
        // the type is what the file is served as: an agent that could change it could make the
        // object a page
        for other in [
            "text/html",
            "image/svg+xml",
            "application/pdf; charset=x",
            "",
        ] {
            let headers: Vec<_> = headers_of(&put)
                .into_iter()
                .map(|(n, v)| {
                    if n == "content-type" {
                        (n, other.to_owned())
                    } else {
                        (n, v)
                    }
                })
                .collect();
            let answer = server
                .send(reqwest::Method::PUT, &put.url, headers, bytes.clone())
                .await;
            refused(&answer, &format!("content-type {other:?}"));
            assert!(
                server.store.head(&key).await.unwrap().is_none(),
                "content-type {other:?}: a refused request leaves nothing under the key"
            );
        }
        the_slot_as_given_is_taken(&server, &key, &put, &bytes).await;
        let head = server.store.head(&key).await.unwrap().unwrap();
        assert_eq!(head.media_type.as_deref(), Some("application/pdf"));
    }

    #[tokio::test]
    async fn a_url_signed_for_put_opens_no_other_method() {
        let Some(server) = server() else { return };
        let (key, meta, bytes) = declared(Some("report.pdf"));
        let put = server.slot(&key, &meta).await;
        // before the object is there, and after it is: a GET that was taken for a read of an
        // object that exists would give the bytes away to whoever holds a URL that was made for
        // writing them
        for stage in ["before the upload", "after the upload"] {
            if stage == "after the upload" {
                the_slot_as_given_is_taken(&server, &key, &put, &bytes).await;
            }
            // with no header, and with the slot's own (a client that sends them on any method): the
            // refusal must be the method's, not only a header that was left out
            for method in [
                reqwest::Method::GET,
                reqwest::Method::HEAD,
                reqwest::Method::DELETE,
            ] {
                for (with, headers) in [
                    ("no header", Vec::new()),
                    ("the slot's headers", headers_of(&put)),
                ] {
                    let answer = server
                        .send(method.clone(), &put.url, headers, Bytes::new())
                        .await;
                    refused(&answer, &format!("{method} with {with} {stage}"));
                    assert!(
                        !String::from_utf8_lossy(&answer.body)
                            .contains("a report that is too large"),
                        "{method} with {with} {stage}: the bytes were given to a URL made for writing"
                    );
                }
            }
        }
        assert!(
            server.store.head(&key).await.unwrap().is_some(),
            "a refused DELETE leaves the object"
        );
    }

    #[tokio::test]
    async fn a_url_opens_one_key_and_not_the_next() {
        let Some(server) = server() else { return };
        let (key, meta, bytes) = declared(Some("report.pdf"));
        let put = server.slot(&key, &meta).await;
        // the same signature on another hash of the same thread, and on another thread
        let other_hash = "ab".repeat(32);
        for (what, url) in [
            (
                "another hash",
                put.url.replace(&key.sha256_hex(), &other_hash),
            ),
            (
                "another thread",
                put.url
                    .replace(&key.thread().to_string(), &thread().to_string()),
            ),
        ] {
            assert_ne!(url, put.url, "{what}: the test changed the URL");
            let answer = server
                .send(reqwest::Method::PUT, &url, headers_of(&put), bytes.clone())
                .await;
            refused(&answer, what);
        }
        assert!(server.store.head(&key).await.unwrap().is_none());
        the_slot_as_given_is_taken(&server, &key, &put, &bytes).await;
    }
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
async fn a_copy_is_one_server_side_request_that_sends_no_bytes_and_keeps_the_meta() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub).with_prefix("files/prod")).unwrap();
    let (from, meta, bytes) = file(Some("Résumé 100%.svg"));
    let to = meta.key(thread());
    store.put(&from, bytes.clone(), &meta).await.unwrap();
    let before = stub.seen().len();

    store.copy(&from, &to).await.unwrap();

    let seen = stub.seen();
    assert_eq!(seen.len(), before + 1, "{seen:?}");
    let copy = &seen[before];
    assert_eq!(copy.method, "PUT");
    assert_eq!(
        copy.path,
        format!("/{BUCKET}/{}", object_key("files/prod/", &to))
    );
    assert_eq!(copy.access_key_id.as_deref(), Some(ACCESS_KEY_ID));
    let source = copy.headers["x-amz-copy-source"].to_str().unwrap();
    assert!(
        source.contains(&object_key("files/prod/", &from)),
        "the source is named in the header: {source}"
    );
    let sent = copy
        .headers
        .get("content-length")
        .map_or("0", |v| v.to_str().unwrap());
    assert_eq!(sent, "0", "no bytes travel with a copy");

    // the copy has the bytes and the meta of the source, and the source is still there
    assert_eq!(drain(&store, &to).await, bytes.to_vec());
    let (got, _) = store.get(&to).await.unwrap().unwrap();
    assert_eq!(got, meta);
    store.delete(&from).await.unwrap();
    assert_eq!(drain(&store, &to).await, bytes.to_vec());
}

#[tokio::test]
async fn a_copy_of_what_is_not_there_is_not_found_and_one_to_itself_asks_nobody_to_copy() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    let (from, meta, bytes) = file(None);
    let to = meta.key(thread());
    let err = store.copy(&from, &to).await.unwrap_err();
    assert!(matches!(err, ArtifactError::NotFound), "{err}");
    let err = store.copy(&from, &from).await.unwrap_err();
    assert!(matches!(err, ArtifactError::NotFound), "{err}");
    assert!(stub.keys().is_empty());

    store.put(&from, bytes, &meta).await.unwrap();
    let before = stub.seen().len();
    store.copy(&from, &from).await.unwrap();
    let seen = stub.seen();
    assert!(
        seen[before..].iter().all(|s| s.method == "HEAD"),
        "{seen:?}"
    );
    assert_eq!(stub.keys().len(), 1);
}

#[tokio::test]
async fn a_copy_that_is_refused_or_fails_is_told_apart() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    let (from, meta, bytes) = file(None);
    store.put(&from, bytes, &meta).await.unwrap();

    let wrong = S3Artifacts::new(config(&stub).with_credentials(
        SecretString::from("AKIDWRONGEXAMPLE"),
        SecretString::from("wrong-secret-access-key-987654321"),
    ))
    .unwrap();
    let err = wrong.copy(&from, &meta.key(thread())).await.unwrap_err();
    assert!(matches!(err, ArtifactError::Unauthenticated), "{err}");

    stub.set_mode(Mode::Fail(axum::http::StatusCode::INTERNAL_SERVER_ERROR));
    let err = store.copy(&from, &meta.key(thread())).await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Transient, "{err}");
    let shown = format!("{err} {err:?}");
    assert!(
        !shown.contains(ACCESS_KEY_ID) && !shown.contains(SECRET_ACCESS_KEY),
        "{shown}"
    );

    // another hash is refused before anything is sent
    stub.set_mode(Mode::Up);
    let before = stub.seen().len();
    let (other, _, _) = file(None);
    let wrong_hash = ArtifactKey::new(other.thread(), [7u8; 32]);
    let err = store.copy(&from, &wrong_hash).await.unwrap_err();
    assert!(matches!(err, ArtifactError::Invalid(_)), "{err}");
    assert_eq!(stub.seen().len(), before);
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
async fn a_head_refuses_an_object_that_names_another_hash_as_a_get_does_and_tolerates_the_same_absences()
 {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    let (key, _, bytes) = file(None);
    let other = "0".repeat(64);
    let own = key.sha256_hex();

    // another hash than the key's: corrupt for the one as for the other, and told as such
    stub.insert(
        &object_key("", &key),
        &bytes,
        &[
            ("content-type", "image/svg+xml"),
            ("x-amz-meta-sha256", other.as_str()),
        ],
    );
    let err = store.get(&key).await.err().expect("get");
    assert!(matches!(err, ArtifactError::Corrupt(_)), "{err}");
    let err = store.head(&key).await.unwrap_err();
    assert!(matches!(err, ArtifactError::Corrupt(_)), "{err}");
    assert_eq!(err.class(), ErrorClass::Corrupt);
    // the text names neither hash
    let shown = format!("{err} {err:?}");
    assert!(!shown.contains(&other) && !shown.contains(&own), "{shown}");

    // its own hash, and none at all (an object put by someone else), are fine for both
    for (what, headers) in [
        (
            "its own hash",
            vec![
                ("content-type", "image/svg+xml"),
                ("x-amz-meta-sha256", own.as_str()),
            ],
        ),
        ("no hash", vec![("content-type", "image/svg+xml")]),
    ] {
        stub.insert(&object_key("", &key), &bytes, &headers);
        assert!(store.get(&key).await.unwrap().is_some(), "{what}");
        let head = store.head(&key).await.unwrap().expect(what);
        assert_eq!(head.size, bytes.len() as u64, "{what}");
    }
}

#[tokio::test]
async fn erasing_a_thread_lists_its_own_part_of_the_bucket_and_deletes_each_object() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub).with_prefix("files/prod")).unwrap();
    let other_deployment = S3Artifacts::new(config(&stub).with_prefix("files/staging")).unwrap();
    let doomed = thread();
    let beside = thread();
    // more keys than a page of the stub, so the listing is followed through its tokens
    let count = support::PAGE * 2 + 5;
    for n in 0..count {
        let bytes = format!("<svg xmlns='http://www.w3.org/2000/svg'><!-- {n} --></svg>");
        let meta = ArtifactMeta::of("image/svg+xml", None, bytes.as_bytes());
        store
            .put(&meta.key(doomed), Bytes::from(bytes.clone()), &meta)
            .await
            .unwrap();
        // the same content in a thread beside it and under another prefix
        if n < 3 {
            store
                .put(&meta.key(beside), Bytes::from(bytes.clone()), &meta)
                .await
                .unwrap();
            other_deployment
                .put(&meta.key(doomed), Bytes::from(bytes), &meta)
                .await
                .unwrap();
        }
    }
    let before = stub.seen().len();

    let removed = store.delete_prefix(doomed).await.unwrap();

    assert_eq!(removed, count as u64);
    let keys = stub.keys();
    assert_eq!(keys.len(), 6, "{keys:?}");
    assert!(
        keys.iter()
            .all(|k| !k.starts_with(&format!("files/prod/threads/{doomed}/")))
    );
    assert_eq!(
        keys.iter()
            .filter(|k| k.starts_with(&format!("files/prod/threads/{beside}/")))
            .count(),
        3
    );
    assert_eq!(
        keys.iter()
            .filter(|k| k.starts_with(&format!("files/staging/threads/{doomed}/")))
            .count(),
        3
    );
    let seen = stub.seen();
    let calls = &seen[before..];
    let lists: Vec<_> = calls.iter().filter(|s| s.method == "GET").collect();
    assert_eq!(lists.len(), 3, "{calls:?}");
    let deletes = calls.iter().filter(|s| s.method == "DELETE").count();
    assert_eq!(deletes, count, "one DELETE per object, no bulk call");
    assert!(
        calls.iter().all(|s| s.method != "POST"),
        "the bulk call is turned off: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .all(|s| s.access_key_id.as_deref() == Some(ACCESS_KEY_ID))
    );

    // again, and for a thread of no file: nothing to remove
    assert_eq!(store.delete_prefix(doomed).await.unwrap(), 0);
    assert_eq!(store.delete_prefix(thread()).await.unwrap(), 0);
}

#[tokio::test]
async fn erasing_with_a_refused_key_or_a_failing_server_removes_nothing_and_shows_no_credential() {
    let stub = Stub::start().await;
    let (key, meta, bytes) = file(None);
    S3Artifacts::new(config(&stub))
        .unwrap()
        .put(&key, bytes, &meta)
        .await
        .unwrap();

    let wrong = config(&stub).with_credentials(
        SecretString::from("AKIDWRONGEXAMPLE"),
        SecretString::from("wrong-secret-access-key-987654321"),
    );
    // (the library reports a refused *listing* as a failed request, not as a refusal: the class is
    // not asserted, and the purge that retries it does not depend on it)
    let err = S3Artifacts::new(wrong)
        .unwrap()
        .delete_prefix(key.thread())
        .await
        .unwrap_err();
    let shown = format!("{err} {err:?}");
    assert!(
        !shown.contains("AKIDWRONGEXAMPLE") && !shown.contains("wrong-secret"),
        "{shown}"
    );

    stub.set_mode(Mode::Fail(axum::http::StatusCode::BAD_REQUEST));
    let err = S3Artifacts::new(config(&stub))
        .unwrap()
        .delete_prefix(key.thread())
        .await
        .unwrap_err();
    assert!(matches!(err, ArtifactError::Unavailable { .. }), "{err}");
    let shown = format!("{err} {err:?}");
    assert!(!shown.contains(SECRET_ACCESS_KEY), "{shown}");
    stub.set_mode(Mode::Up);
    assert_eq!(stub.keys().len(), 1, "nothing was removed");
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

// ---- presigned uploads -------------------------------------------------------------------------

fn declared(name: Option<&str>) -> (ArtifactKey, ArtifactMeta, Bytes) {
    let bytes = b"%PDF-1.7 a report that is too large for a message";
    let meta = ArtifactMeta::of("application/pdf", name.map(str::to_owned), bytes);
    (meta.key(thread()), meta, Bytes::from_static(bytes))
}

#[tokio::test]
async fn a_slot_is_signed_for_the_public_host_with_the_headers_the_signature_binds_and_calls_nobody()
 {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(
        config(&stub)
            .with_public_endpoint("https://agentic-files.example.com")
            .with_prefix("files/prod"),
    )
    .unwrap();
    assert!(store.can_presign());
    let (key, meta, _) = declared(Some("Résumé 100%.pdf"));

    let put = store
        .presign_put(&key, &meta, Duration::from_secs(300))
        .await
        .unwrap();

    // the host of the URL is the public one, the bucket is the first segment of the path, the
    // prefix and the key follow
    let url = reqwest::Url::parse(&put.url).unwrap();
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("agentic-files.example.com"));
    assert_eq!(
        url.path(),
        format!("/{BUCKET}/{}", object_key("files/prod/", &key))
    );
    let query: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["X-Amz-Expires"], "300");
    assert_eq!(query["X-Amz-Algorithm"], "AWS4-HMAC-SHA256");
    assert!(query["X-Amz-Credential"].starts_with(&format!("{ACCESS_KEY_ID}/")));
    assert!(!query["X-Amz-Signature"].is_empty());
    // the signature binds the host and every header of the answer, and nothing else
    let mut signed: Vec<_> = query["X-Amz-SignedHeaders"].split(';').collect();
    signed.sort_unstable();
    let mut expected: Vec<_> = put.headers.keys().map(String::as_str).collect();
    expected.push("host");
    expected.sort_unstable();
    assert_eq!(signed, expected);

    // those headers are the content's checksum (base64 of the SHA-256), its type and the metadata
    // a plain put writes
    use base64::Engine as _;
    assert_eq!(put.headers["content-type"], "application/pdf");
    assert_eq!(
        put.headers["x-amz-checksum-sha256"],
        base64::engine::general_purpose::STANDARD.encode(key.sha256())
    );
    assert_eq!(put.headers["x-amz-meta-sha256"], key.sha256_hex());
    let name = &put.headers["x-amz-meta-filename"];
    assert!(
        name.is_ascii() && !name.contains(' ') && name.contains("%C3%A9"),
        "{name}"
    );
    assert_eq!(put.headers.len(), 4);

    // a file with no name signs no name
    let (key, meta, _) = declared(None);
    let put = store
        .presign_put(&key, &meta, Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(put.headers.len(), 3);
    assert!(put.url.contains("X-Amz-Expires=60"));

    // signing is local: nothing was sent to the bucket, or to the public host
    assert!(stub.seen().is_empty(), "{:?}", stub.seen());
    // and the URL, which is a credential for its minutes, is shown by nothing
    let shown = format!("{put:?} {store:?}");
    assert!(
        !shown.contains("X-Amz-Signature") && !shown.contains(&put.url),
        "{shown}"
    );
}

#[tokio::test]
async fn the_object_an_agent_leaves_reads_back_like_one_the_store_put() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub).with_public_endpoint(stub.endpoint())).unwrap();
    let (key, meta, bytes) = declared(Some("Résumé 100%.pdf"));
    let put = store
        .presign_put(&key, &meta, Duration::from_secs(300))
        .await
        .unwrap();
    upload_over_http()(put, bytes.clone()).await.unwrap();

    // the same meta, name included, and the same bytes
    let (got, _) = store.get(&key).await.unwrap().unwrap();
    assert_eq!(got, meta);
    assert_eq!(drain(&store, &key).await, bytes.to_vec());
    // and the same headers on the object as a plain put writes
    let stored = stub.object(&object_key("", &key)).unwrap();
    let names: Vec<_> = stored.headers.iter().map(|(n, _)| n.as_str()).collect();
    for expected in ["content-type", "x-amz-meta-sha256", "x-amz-meta-filename"] {
        assert!(names.contains(&expected), "{names:?}");
    }
}

#[tokio::test]
async fn the_bucket_refuses_a_request_that_lacks_a_signed_header_or_carries_other_bytes() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub).with_public_endpoint(stub.endpoint())).unwrap();
    let (key, meta, bytes) = declared(None);
    let put = store
        .presign_put(&key, &meta, Duration::from_secs(300))
        .await
        .unwrap();
    let client = reqwest::Client::new();
    let send = |headers: Vec<(String, String)>, body: Bytes| {
        let client = client.clone();
        let url = put.url.clone();
        async move {
            let mut request = client.put(url);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            request.body(body).send().await.unwrap().status().as_u16()
        }
    };
    let all: Vec<_> = put
        .headers
        .iter()
        .map(|(n, v)| (n.clone(), v.clone()))
        .collect();

    // a header left out: the signature does not match
    for left_out in put.headers.keys() {
        let without: Vec<_> = all.iter().filter(|(n, _)| n != left_out).cloned().collect();
        assert_eq!(
            send(without, bytes.clone()).await,
            403,
            "without {left_out}"
        );
    }
    // other bytes under the signed checksum: refused, and nothing is left under the key
    assert_eq!(
        send(all.clone(), Bytes::from_static(b"other bytes")).await,
        400
    );
    assert!(stub.object(&object_key("", &key)).is_none());
    // the declared bytes: taken
    assert_eq!(send(all, bytes).await, 200);
    assert!(stub.object(&object_key("", &key)).is_some());
}

#[tokio::test]
async fn a_head_is_one_signed_request_that_asks_for_the_checksum_and_reads_what_comes_back() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(
        config(&stub)
            .with_public_endpoint(stub.endpoint())
            .with_prefix("files/prod"),
    )
    .unwrap();
    let (key, meta, bytes) = declared(Some("report.pdf"));
    let put = store
        .presign_put(&key, &meta, Duration::from_secs(300))
        .await
        .unwrap();
    upload_over_http()(put, bytes).await.unwrap();
    let before = stub.seen().len();

    let head = store.head(&key).await.unwrap().unwrap();

    assert_eq!(head.size, meta.size);
    assert_eq!(head.media_type.as_deref(), Some("application/pdf"));
    assert_eq!(head.sha256, Some(meta.sha256));
    let modified = head.last_modified.expect("the time of the object");
    let age = jiff::Timestamp::now().duration_since(modified);
    assert!(age.as_secs().abs() < 60, "{modified}");

    let seen = stub.seen();
    assert_eq!(seen.len(), before + 1, "{seen:?}");
    let request = &seen[before];
    assert_eq!(request.method, "HEAD");
    assert_eq!(
        request.path,
        format!("/{BUCKET}/{}", object_key("files/prod/", &key))
    );
    // signed for the internal endpoint, in the query: no Authorization header
    assert!(request.headers.get("authorization").is_none());
    assert_eq!(request.access_key_id.as_deref(), Some(ACCESS_KEY_ID));
    assert_eq!(
        request.headers["x-amz-checksum-mode"].to_str().unwrap(),
        "ENABLED"
    );
    assert!(
        request.query.contains("x-amz-checksum-mode"),
        "{}",
        request.query
    );
}

#[tokio::test]
async fn a_server_that_says_no_checksum_gives_none_and_a_missing_file_is_none() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    let (key, meta, bytes) = file(None);
    assert!(store.head(&key).await.unwrap().is_none());
    // an object put by someone else, with no checksum at all
    stub.insert(
        &object_key("", &key),
        &bytes,
        &[("content-type", "image/svg+xml")],
    );
    let head = store.head(&key).await.unwrap().unwrap();
    assert_eq!(head.size, meta.size);
    assert_eq!(head.sha256, None);
}

#[tokio::test]
async fn a_head_that_is_refused_or_fails_is_told_apart_and_shows_no_signature() {
    let stub = Stub::start().await;
    let (key, meta, bytes) = file(None);
    let store = S3Artifacts::new(config(&stub)).unwrap();
    store.put(&key, bytes, &meta).await.unwrap();

    let wrong = S3Artifacts::new(config(&stub).with_credentials(
        SecretString::from("AKIDWRONGEXAMPLE"),
        SecretString::from("wrong-secret-access-key-987654321"),
    ))
    .unwrap();
    let err = wrong.head(&key).await.unwrap_err();
    assert!(matches!(err, ArtifactError::Unauthenticated), "{err}");

    stub.set_mode(Mode::Fail(axum::http::StatusCode::INTERNAL_SERVER_ERROR));
    let before = stub.seen().len();
    let err = store.head(&key).await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Transient, "{err}");
    assert_eq!(
        stub.seen().len() - before,
        3,
        "tried three times, then told"
    );
    let shown = format!("{err} {err:?}");
    assert!(
        !shown.contains("X-Amz-Signature") && !shown.contains(SECRET_ACCESS_KEY),
        "{shown}"
    );

    stub.set_mode(Mode::Up);
    assert!(store.head(&key).await.unwrap().is_some(), "it recovers");

    // a server nobody listens on is transient, and its error shows no signed URL either
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let nobody = S3Artifacts::new(
        S3Config::new(BUCKET)
            .with_endpoint(format!("http://127.0.0.1:{port}"))
            .with_credentials(
                SecretString::from(ACCESS_KEY_ID),
                SecretString::from(SECRET_ACCESS_KEY),
            )
            .with_timeout(Duration::from_secs(5)),
    )
    .unwrap();
    let err = nobody.head(&key).await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Transient, "{err}");
    let mut shown = format!("{err} {err:?}");
    let mut source = std::error::Error::source(&err);
    while let Some(cause) = source {
        shown.push_str(&format!(" {cause} {cause:?}"));
        source = cause.source();
    }
    assert!(!shown.contains("X-Amz-Signature"), "{shown}");
}

#[tokio::test]
async fn every_attempt_of_a_head_is_signed_afresh_so_a_slow_first_one_does_not_expire_the_retry() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    let (key, meta, bytes) = file(None);
    store.put(&key, bytes, &meta).await.unwrap();

    // The signature of a `HEAD` lives a minute and a request may take as long: a first attempt that
    // takes its time and fails must be retried with a signature of its own, not the one that is
    // about to run out. A signature is dated in whole seconds, so the first attempt takes two.
    stub.set_mode(Mode::FailFirst {
        remaining: 1,
        status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
        delay: Duration::from_millis(2100),
    });
    let before = stub.seen().len();
    let head = store.head(&key).await.unwrap().expect("the retry works");
    assert_eq!(head.size, meta.size);

    let seen = stub.seen();
    let heads: Vec<_> = seen[before..]
        .iter()
        .filter(|request| request.method == "HEAD")
        .collect();
    assert_eq!(heads.len(), 2, "{heads:?}");
    let signed = |query: &str, name: &str| {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")).map(str::to_owned))
            .unwrap_or_default()
    };
    let (first, second) = (&heads[0].query, &heads[1].query);
    assert_ne!(
        signed(first, "X-Amz-Date"),
        signed(second, "X-Amz-Date"),
        "the retry is dated when it is sent"
    );
    assert_ne!(
        signed(first, "X-Amz-Signature"),
        signed(second, "X-Amz-Signature")
    );
}

#[tokio::test]
async fn the_first_bytes_are_one_ranged_read() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    let (key, meta, bytes) = file(None);
    store.put(&key, bytes.clone(), &meta).await.unwrap();
    let before = stub.seen().len();

    let got = store.read_prefix(&key, 10).await.unwrap().unwrap();

    assert_eq!(got.as_ref(), &bytes[..10]);
    let seen = stub.seen();
    assert_eq!(seen.len(), before + 1, "{seen:?}");
    assert_eq!(seen[before].method, "GET");
    assert_eq!(seen[before].headers["range"].to_str().unwrap(), "bytes=0-9");
    // a request for more than there is gets all there is
    let all = store.read_prefix(&key, 1_000_000).await.unwrap().unwrap();
    assert_eq!(all, bytes);
}

#[tokio::test]
async fn a_store_without_a_public_endpoint_signs_nothing() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    assert!(!store.can_presign());
    let (key, meta, _) = declared(None);
    let err = store
        .presign_put(&key, &meta, Duration::from_secs(300))
        .await
        .unwrap_err();
    assert!(matches!(err, ArtifactError::Unsupported), "{err}");
    assert!(stub.seen().is_empty());
    // the public endpoint is part of the configuration's debug text, and the credentials still are not
    let with = config(&stub).with_public_endpoint("https://files.example.com");
    let shown = format!("{with:?}");
    assert!(
        shown.contains("files.example.com") && !shown.contains(SECRET_ACCESS_KEY),
        "{shown}"
    );
}

// ---- presigned downloads -----------------------------------------------------------------------

/// The headers a download is signed to carry, for a file with an awkward name.
fn overrides() -> ResponseOverrides {
    ResponseOverrides {
        content_disposition: Some(
            "attachment; filename=\"r&d = 50+50? #1 (final) _.zip\"; \
             filename*=UTF-8''r%26d%20%3D%2050%2B50%3F%20%231%20%28final%29%20%C3%A9.zip"
                .to_owned(),
        ),
        content_type: Some("application/octet-stream".to_owned()),
        cache_control: Some("private, max-age=300".to_owned()),
    }
}

fn zip(name: Option<&str>) -> (ArtifactKey, ArtifactMeta, Bytes) {
    let bytes = b"PK\x03\x04 a report that is large enough to be worth a redirect";
    let meta = ArtifactMeta::of("application/zip", name.map(str::to_owned), bytes);
    (meta.key(thread()), meta, Bytes::from_static(bytes))
}

#[tokio::test]
async fn a_download_is_signed_for_the_public_host_with_the_overrides_in_the_signature_and_calls_nobody()
 {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(
        config(&stub)
            .with_public_endpoint("https://agentic-files.example.com")
            .with_prefix("files/prod"),
    )
    .unwrap();
    let (key, _, _) = zip(Some("r&d.zip"));
    let asked = overrides();

    let before = jiff::Timestamp::now();
    let get = store
        .presign_get(&key, &asked, Duration::from_secs(60))
        .await
        .unwrap();

    // the host of the URL is the public one, the bucket is the first segment of the path, the
    // prefix and the key follow
    let url = reqwest::Url::parse(&get.url).unwrap();
    assert_eq!(url.scheme(), "https");
    assert_eq!(url.host_str(), Some("agentic-files.example.com"));
    assert_eq!(
        url.path(),
        format!("/{BUCKET}/{}", object_key("files/prod/", &key))
    );
    let query: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["X-Amz-Expires"], "60");
    assert_eq!(query["X-Amz-Algorithm"], "AWS4-HMAC-SHA256");
    assert!(query["X-Amz-Credential"].starts_with(&format!("{ACCESS_KEY_ID}/")));
    assert!(!query["X-Amz-Signature"].is_empty());
    // the signature binds the host and nothing else: the browser sends no other header
    assert_eq!(query["X-Amz-SignedHeaders"], "host");
    // the overrides are query members of the signature, exactly as asked
    assert_eq!(
        query["response-content-disposition"],
        asked.content_disposition.clone().unwrap()
    );
    assert_eq!(
        query["response-content-type"],
        asked.content_type.clone().unwrap()
    );
    assert_eq!(
        query["response-cache-control"],
        asked.cache_control.clone().unwrap()
    );
    // and a space is `%20` in the query that was signed, never `+`
    assert!(!get.url.contains('+'), "{}", get.url);
    // the expiry is the lifetime from now, in whole seconds, and never later than the signature's
    // own (`X-Amz-Date` is floored to the second, and so is `expires_at`)
    assert_eq!(
        get.expires_at.subsec_nanosecond(),
        0,
        "{:?}",
        get.expires_at
    );
    let signed_at = jiff::civil::DateTime::strptime("%Y%m%dT%H%M%SZ", &query["X-Amz-Date"])
        .unwrap()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .unwrap()
        .timestamp();
    assert!(
        get.expires_at <= signed_at + Duration::from_secs(60),
        "{:?} is past the signature's {signed_at}",
        get.expires_at
    );
    assert!(
        get.expires_at > before + Duration::from_secs(59)
            && get.expires_at <= jiff::Timestamp::now() + Duration::from_secs(60),
        "{:?}",
        get.expires_at
    );

    // with nothing to override nothing is signed beyond the request
    let plain = store
        .presign_get(&key, &ResponseOverrides::default(), Duration::from_secs(60))
        .await
        .unwrap();
    assert!(!plain.url.contains("response-"), "{}", plain.url);

    // signing is local: nothing was sent to the bucket, or to the public host
    assert!(stub.seen().is_empty(), "{:?}", stub.seen());
    // and the URL, which is a credential for its minute, is shown by nothing
    let shown = format!("{get:?} {store:?}");
    assert!(
        !shown.contains("X-Amz-Signature") && !shown.contains(&get.url),
        "{shown}"
    );
}

#[tokio::test]
async fn a_download_that_is_wrong_is_never_signed() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub).with_public_endpoint(stub.endpoint())).unwrap();
    let (key, _, _) = zip(None);
    let ok = overrides();
    let mut bad = ok.clone();
    bad.content_disposition = Some("attachment\r\nx-evil: 1".to_owned());
    assert!(matches!(
        store.presign_get(&key, &bad, Duration::from_secs(60)).await,
        Err(ArtifactError::Invalid(_))
    ));
    // a lifetime of nothing, of half a second (S3 reads whole seconds and refuses 0), and one over
    // seven days
    for ttl in [
        Duration::ZERO,
        Duration::from_millis(500),
        orch_ports::MAX_PRESIGN_TTL + Duration::from_secs(1),
    ] {
        assert!(matches!(
            store.presign_get(&key, &ok, ttl).await,
            Err(ArtifactError::Invalid(_))
        ));
    }
    // a store with no public endpoint signs nothing
    let internal = S3Artifacts::new(config(&stub)).unwrap();
    assert!(matches!(
        internal
            .presign_get(&key, &ok, Duration::from_secs(60))
            .await,
        Err(ArtifactError::Unsupported)
    ));
    assert!(stub.seen().is_empty());
}

#[tokio::test]
async fn a_head_says_the_name_a_download_is_given_and_one_that_is_not_text_is_not_said() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub)).unwrap();
    // put by this store: the name is percent-encoded metadata and comes back whole
    let (key, meta, bytes) = file(Some("Résumé 100%.svg"));
    store.put(&key, bytes.clone(), &meta).await.unwrap();
    let head = store.head(&key).await.unwrap().unwrap();
    assert_eq!(head.filename.as_deref(), Some("Résumé 100%.svg"));
    assert_eq!(head.media_type.as_deref(), Some("image/svg+xml"));
    // a file with no name has none
    let (key, meta, bytes) = file(None);
    store.put(&key, bytes.clone(), &meta).await.unwrap();
    assert_eq!(store.head(&key).await.unwrap().unwrap().filename, None);
    // an object written by someone else with a name that is not UTF-8: not said, and not an error
    stub.insert(
        &object_key("", &key),
        &bytes,
        &[
            ("content-type", "image/svg+xml"),
            ("x-amz-meta-filename", "%FF%FE"),
        ],
    );
    let head = store.head(&key).await.unwrap().unwrap();
    assert_eq!(head.filename, None);
}

#[tokio::test]
async fn the_stub_answers_a_presigned_get_with_the_signed_overrides_and_not_after_they_expire() {
    let stub = Stub::start().await;
    let store = S3Artifacts::new(config(&stub).with_public_endpoint(stub.endpoint())).unwrap();
    let (key, meta, bytes) = zip(Some("r&d.zip"));
    store.put(&key, bytes.clone(), &meta).await.unwrap();
    let asked = overrides();
    let download = download_over_http();

    let get = store
        .presign_get(&key, &asked, Duration::from_secs(60))
        .await
        .unwrap();
    let got = download(get).await.unwrap();
    assert_eq!(got.bytes, bytes);
    assert_eq!(got.content_disposition, asked.content_disposition);
    assert_eq!(got.content_type, asked.content_type);
    assert_eq!(got.cache_control, asked.cache_control);

    // a URL whose lifetime is over is refused. This waits for real: the clock of a signature is the
    // wall clock, in whole seconds
    let short = store
        .presign_get(&key, &asked, Duration::from_secs(1))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let err = download(short).await.unwrap_err();
    assert_eq!(err, "HTTP 403 Forbidden");
}

/// What a **real** server holds a presigned `GET` to (ADR 0058 A1): the signature binds the
/// method, the key, the time and the three `response-*` members, so the holder of a URL cannot
/// change how the file is sent, and an anonymous request opens nothing. The stub does not check a
/// signature, so these cases run only with `ORCH_TEST_S3_URL`.
mod a_real_server_holds_a_presigned_get_to_what_was_signed {
    use super::a_real_server_holds_a_presigned_put_to_what_was_signed::{Server, server};
    use super::*;

    /// A file in the bucket and a URL for it with the awkward overrides, signed for `ttl`.
    async fn kept(
        server: &Server,
        ttl: Duration,
    ) -> (ArtifactKey, Bytes, orch_ports::PresignedGet) {
        let (key, meta, bytes) = zip(Some("r&d.zip"));
        server.store.put(&key, bytes.clone(), &meta).await.unwrap();
        let get = server
            .store
            .presign_get(&key, &overrides(), ttl)
            .await
            .unwrap();
        (key, bytes, get)
    }

    struct Answer {
        status: u16,
        headers: reqwest::header::HeaderMap,
        body: Bytes,
    }

    async fn send(
        server: &Server,
        method: reqwest::Method,
        url: &str,
        range: Option<&str>,
        body: Bytes,
    ) -> Answer {
        let mut request = server.client.request(method, url);
        if let Some(range) = range {
            request = request.header("range", range);
        }
        let response = request.body(body).send().await.unwrap();
        Answer {
            status: response.status().as_u16(),
            headers: response.headers().clone(),
            body: response.bytes().await.unwrap(),
        }
    }

    fn refused(answer: &Answer, what: &str, bytes: &Bytes) {
        eprintln!(
            "{what}: HTTP {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body[..answer.body.len().min(120)])
        );
        assert!(
            (400..500).contains(&answer.status),
            "{what}: the server must refuse it with a 4xx, it answered {}",
            answer.status
        );
        assert!(
            !answer
                .body
                .windows(8)
                .any(|w| bytes.windows(8).any(|b| b == w)),
            "{what}: the bytes of the file were given away"
        );
    }

    fn header<'a>(answer: &'a Answer, name: &str) -> Option<&'a str> {
        answer.headers.get(name).and_then(|v| v.to_str().ok())
    }

    #[tokio::test]
    async fn the_signed_overrides_come_back_as_the_headers_of_the_answer_and_the_bytes_are_the_files()
     {
        let Some(server) = server() else { return };
        let (_, bytes, get) = kept(&server, Duration::from_secs(60)).await;
        let asked = overrides();
        let answer = send(&server, reqwest::Method::GET, &get.url, None, Bytes::new()).await;
        assert_eq!(answer.status, 200);
        assert_eq!(answer.body, bytes);
        assert_eq!(
            header(&answer, "content-disposition"),
            asked.content_disposition.as_deref()
        );
        assert_eq!(
            header(&answer, "content-type"),
            asked.content_type.as_deref()
        );
        assert_eq!(
            header(&answer, "cache-control"),
            asked.cache_control.as_deref()
        );
        eprintln!(
            "server headers of a presigned GET: nosniff {:?}, csp {:?}, acao {:?}",
            header(&answer, "x-content-type-options"),
            header(&answer, "content-security-policy"),
            header(&answer, "access-control-allow-origin"),
        );
        // what the server says to a browser of another origin, and to its preflight, printed for the
        // one who reads the run (the files host's Ingress adds the headers and the CORS: 6c)
        let origin = server
            .client
            .get(&get.url)
            .header("origin", "https://app.example.com")
            .send()
            .await
            .unwrap();
        eprintln!(
            "with an Origin: HTTP {}, acao {:?}, vary {:?}",
            origin.status(),
            origin.headers().get("access-control-allow-origin"),
            origin.headers().get("vary"),
        );
        let preflight = server
            .client
            .request(reqwest::Method::OPTIONS, &get.url)
            .header("origin", "https://app.example.com")
            .header("access-control-request-method", "GET")
            .header("access-control-request-headers", "dpop")
            .send()
            .await
            .unwrap();
        eprintln!(
            "preflight: HTTP {}, acao {:?}, acah {:?}, acam {:?}",
            preflight.status(),
            preflight.headers().get("access-control-allow-origin"),
            preflight.headers().get("access-control-allow-headers"),
            preflight.headers().get("access-control-allow-methods"),
        );
    }

    /// The browser may ask for a range of a large file or a video; it is not a signed header.
    #[tokio::test]
    async fn a_range_of_the_file_is_served_under_the_same_url() {
        let Some(server) = server() else { return };
        let (_, bytes, get) = kept(&server, Duration::from_secs(60)).await;
        let answer = send(
            &server,
            reqwest::Method::GET,
            &get.url,
            Some("bytes=3-9"),
            Bytes::new(),
        )
        .await;
        assert_eq!(answer.status, 206, "a range is a partial answer");
        assert_eq!(answer.body, bytes.slice(3..10));
        // the signed overrides still apply to the part
        assert_eq!(
            header(&answer, "content-type"),
            overrides().content_type.as_deref()
        );
    }

    #[tokio::test]
    async fn a_signed_override_that_is_changed_removed_or_added_to_is_refused() {
        let Some(server) = server() else { return };
        let (_, bytes, get) = kept(&server, Duration::from_secs(60)).await;
        let control = send(&server, reqwest::Method::GET, &get.url, None, Bytes::new()).await;
        assert_eq!(
            control.status, 200,
            "the control: the URL as given is served"
        );

        let url = reqwest::Url::parse(&get.url).unwrap();
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        let rebuilt = |pairs: &[(String, String)]| {
            let mut changed = url.clone();
            changed.query_pairs_mut().clear().extend_pairs(pairs);
            changed.to_string()
        };
        let mut cases: Vec<(String, String)> = Vec::new();
        for name in [
            "response-content-disposition",
            "response-content-type",
            "response-cache-control",
        ] {
            // changed: the type that would make the file a page, the name of another file, a
            // cache that keeps it for a year
            let other = match name {
                "response-content-disposition" => "inline",
                "response-content-type" => "text/html",
                _ => "public, max-age=31536000",
            };
            let mut changed = pairs.clone();
            for (n, v) in &mut changed {
                if n == name {
                    other.clone_into(v);
                }
            }
            cases.push((format!("{name} changed"), rebuilt(&changed)));
            // removed
            let removed: Vec<_> = pairs.iter().filter(|(n, _)| n != name).cloned().collect();
            cases.push((format!("{name} removed"), rebuilt(&removed)));
        }
        // added: members S3 takes that nobody signed
        for (name, value) in [
            ("response-content-encoding", "gzip"),
            ("response-content-language", "de"),
            ("response-expires", "Thu, 01 Jan 2099 00:00:00 GMT"),
        ] {
            let mut added = pairs.clone();
            added.push((name.to_owned(), value.to_owned()));
            cases.push((format!("{name} added"), rebuilt(&added)));
        }
        for (what, url) in cases {
            let answer = send(&server, reqwest::Method::GET, &url, None, Bytes::new()).await;
            refused(&answer, &what, &bytes);
        }
    }

    #[tokio::test]
    async fn a_url_signed_for_get_opens_one_key_and_one_method_and_nothing_without_a_signature() {
        let Some(server) = server() else { return };
        let (key, bytes, get) = kept(&server, Duration::from_secs(60)).await;
        let control = send(&server, reqwest::Method::GET, &get.url, None, Bytes::new()).await;
        assert_eq!(
            control.status, 200,
            "the control: the URL as given is served"
        );

        // another hash of the thread, another thread
        let other_hash = "ab".repeat(32);
        for (what, url) in [
            (
                "another hash",
                get.url.replace(&key.sha256_hex(), &other_hash),
            ),
            (
                "another thread",
                get.url
                    .replace(&key.thread().to_string(), &thread().to_string()),
            ),
        ] {
            assert_ne!(url, get.url, "{what}: the test changed the URL");
            let answer = send(&server, reqwest::Method::GET, &url, None, Bytes::new()).await;
            refused(&answer, what, &bytes);
        }
        // every other method: the signature binds `GET`
        for method in [
            reqwest::Method::HEAD,
            reqwest::Method::PUT,
            reqwest::Method::DELETE,
            reqwest::Method::POST,
        ] {
            let answer = send(
                &server,
                method.clone(),
                &get.url,
                None,
                Bytes::from_static(b"other bytes"),
            )
            .await;
            refused(&answer, &format!("{method}"), &bytes);
        }
        let head = server.store.head(&key).await.unwrap().expect("still there");
        assert_eq!(
            head.size,
            bytes.len() as u64,
            "a refused PUT or DELETE changed nothing"
        );

        // no signature at all: the key is not public, with or without the members
        let plain = format!(
            "{}/{}/{}/threads/{}/{}",
            std::env::var("ORCH_TEST_S3_URL")
                .unwrap()
                .trim_end_matches('/'),
            server.bucket,
            server.prefix,
            key.thread(),
            key.sha256_hex()
        );
        for (what, url) in [
            ("anonymous", plain.clone()),
            (
                "anonymous with the members",
                format!("{plain}?response-content-type=text%2Fhtml"),
            ),
        ] {
            let answer = send(&server, reqwest::Method::GET, &url, None, Bytes::new()).await;
            refused(&answer, what, &bytes);
        }
    }

    /// A short lifetime, and a real wait: the signature's clock is the wall clock in whole seconds
    /// and `object_store` takes no clock of ours.
    #[tokio::test]
    async fn an_expired_url_is_refused() {
        let Some(server) = server() else { return };
        let (_, bytes, get) = kept(&server, Duration::from_secs(2)).await;
        let control = send(&server, reqwest::Method::GET, &get.url, None, Bytes::new()).await;
        assert_eq!(
            control.status, 200,
            "the control: it works before it expires"
        );
        tokio::time::sleep(Duration::from_millis(3500)).await;
        let answer = send(&server, reqwest::Method::GET, &get.url, None, Bytes::new()).await;
        refused(&answer, "after the lifetime", &bytes);
    }
}
