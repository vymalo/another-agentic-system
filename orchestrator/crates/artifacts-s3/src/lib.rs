//! [`ArtifactStore`] over an S3 bucket (ADR 0032, ADR 0009): AWS S3 or any server that speaks its
//! API (MinIO, Ceph, R2, and so on), through the S3 backend of the [`object_store`] crate. The
//! production store.
//!
//! Each file is one object, `[<prefix>/]threads/<thread uuid>/<sha256>`, with the **media type as
//! its `Content-Type`** and the rest of the meta as user metadata (`x-amz-meta-sha256`, and
//! `x-amz-meta-filename` percent-encoded, because a metadata value is ASCII). The size is the
//! object's own. There is no sidecar object: a put is one request, atomic on the server's side.
//!
//! The credentials are the two secrets of the configuration, used as given: this store does not
//! read `AWS_*` variables, the instance profile or a web-identity token (it is built with static
//! credentials only). No error and no `Debug` text holds them.
//!
//! A copy ([`ArtifactStore::copy`], a fork's files) is one server-side `CopyObject`: the bytes do
//! not leave the bucket, and the object's content type and metadata go with it.
//!
//! Requests are bounded: a timeout on each, and a retry budget of two retries within 30 seconds
//! (`object_store` retries connection errors and 5xx itself), after which the caller sees a
//! transient [`ArtifactError::Unavailable`] and decides, as for any other port.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt as _};
use object_store::aws::AmazonS3Builder;
use object_store::path::Path;
use object_store::{
    Attribute, Attributes, ClientOptions, ObjectStore, ObjectStoreExt as _, PutOptions, PutPayload,
    RetryConfig,
};
use orch_core::ThreadId;
use orch_ports::{ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use secrecy::{ExposeSecret as _, SecretString};

/// The default of [`S3Config::with_region`]: what S3-compatible servers that have no regions expect.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The default of [`S3Config::with_timeout`], 60 seconds for one request (a file is at most a few
/// MiB, `artifacts.maxFileBytes`).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// The user metadata key of the content hash.
const META_SHA256: &str = "sha256";
/// The user metadata key of the file name (percent-encoded).
const META_FILENAME: &str = "filename";

/// What the store is built from.
#[derive(Clone)]
pub struct S3Config {
    bucket: String,
    region: String,
    endpoint: Option<String>,
    prefix: Option<String>,
    credentials: Option<(SecretString, SecretString)>,
    timeout: Duration,
}

impl S3Config {
    /// A store in `bucket`, in the region [`DEFAULT_REGION`], at AWS, with no credentials yet.
    pub fn new(bucket: impl Into<String>) -> Self {
        S3Config {
            bucket: bucket.into(),
            region: DEFAULT_REGION.to_owned(),
            endpoint: None,
            prefix: None,
            credentials: None,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// The region of the bucket.
    #[must_use]
    pub fn with_region(mut self, region: impl Into<String>) -> Self {
        self.region = region.into();
        self
    }

    /// The server's endpoint (`https://minio.example.com:9000`) instead of AWS's. The bucket is
    /// then addressed in the path (`<endpoint>/<bucket>/<key>`), which every compatible server
    /// takes. An `http://` endpoint is allowed; the credentials sign requests and are not sent,
    /// but the files are, in the clear.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// A prefix every key is put under, `files/prod` for `files/prod/threads/...`: one bucket for
    /// several deployments.
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// The access key id and the secret access key.
    #[must_use]
    pub fn with_credentials(
        mut self,
        access_key_id: SecretString,
        secret_access_key: SecretString,
    ) -> Self {
        self.credentials = Some((access_key_id, secret_access_key));
        self
    }

    /// How long one request may take (default [`DEFAULT_TIMEOUT`]).
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl fmt::Debug for S3Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Config")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("prefix", &self.prefix)
            .field(
                "credentials",
                &self.credentials.as_ref().map(|_| "<redacted>"),
            )
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// The store could not be built.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    /// The bucket, the region, the endpoint, the prefix or the credentials are not usable. The
    /// text never holds a credential.
    #[error("the S3 artifact store is not configured correctly: {0}")]
    Config(String),
}

/// The store: a bucket.
#[derive(Clone)]
pub struct S3Artifacts {
    store: Arc<dyn ObjectStore>,
    prefix: Vec<String>,
    bucket: String,
}

impl fmt::Debug for S3Artifacts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3Artifacts")
            .field("bucket", &self.bucket)
            .field("prefix", &self.prefix.join("/"))
            .finish_non_exhaustive()
    }
}

impl S3Artifacts {
    /// Builds the store. It does not connect: a bucket that is unreachable is found by the first
    /// call, so a control plane does not wait for a bucket to start.
    ///
    /// # Errors
    /// [`BuildError`] when the bucket is empty, there are no credentials, or the endpoint is not a URL.
    pub fn new(config: S3Config) -> Result<Self, BuildError> {
        // `object_store` signs and connects with the rustls provider of the process.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let bad = |what: &str| BuildError::Config(what.to_owned());
        let Some((access_key_id, secret_access_key)) = &config.credentials else {
            return Err(bad("an access key id and a secret access key are required"));
        };
        let allow_http = config
            .endpoint
            .as_deref()
            .is_some_and(|e| e.starts_with("http://"));
        let mut builder = AmazonS3Builder::new()
            .with_bucket_name(&config.bucket)
            .with_region(&config.region)
            .with_access_key_id(access_key_id.expose_secret())
            .with_secret_access_key(secret_access_key.expose_secret())
            .with_virtual_hosted_style_request(false)
            // one file is one `DELETE`, which every compatible server has (the bulk call does not)
            .with_disable_bulk_delete(true)
            .with_client_options(
                ClientOptions::new()
                    .with_allow_http(allow_http)
                    .with_timeout(config.timeout)
                    .with_connect_timeout(Duration::from_secs(10)),
            )
            .with_retry(RetryConfig {
                max_retries: 2,
                retry_timeout: Duration::from_secs(30),
                ..RetryConfig::default()
            });
        if let Some(endpoint) = &config.endpoint {
            builder = builder.with_endpoint(endpoint);
        } else {
            // AWS addresses a bucket as a host name.
            builder = builder.with_virtual_hosted_style_request(true);
        }
        let store = builder
            .build()
            .map_err(|e| BuildError::Config(strip_secrets(&e.to_string(), &config)))?;
        let prefix = config
            .prefix
            .as_deref()
            .unwrap_or_default()
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(S3Artifacts {
            store: Arc::new(store),
            prefix,
            bucket: config.bucket,
        })
    }

    /// The object path of a key: the prefix, then `threads/<uuid>/<hash>`.
    fn path_of(&self, key: &ArtifactKey) -> Path {
        let thread = key.thread().to_string();
        let hash = key.sha256_hex();
        self.prefix
            .iter()
            .map(String::as_str)
            .chain(["threads", thread.as_str(), hash.as_str()])
            .collect()
    }
}

/// The text of a build error, with the two credentials taken out in case a library quoted one.
fn strip_secrets(text: &str, config: &S3Config) -> String {
    let mut text = text.to_owned();
    if let Some((id, secret)) = &config.credentials {
        for value in [id.expose_secret(), secret.expose_secret()] {
            if !value.is_empty() {
                text = text.replace(value, "<redacted>");
            }
        }
    }
    text
}

/// The port's error for one of the library's.
fn map_error(error: object_store::Error, doing: &str) -> ArtifactError {
    use object_store::Error;
    match error {
        Error::PermissionDenied { .. } | Error::Unauthenticated { .. } => {
            ArtifactError::Unauthenticated
        }
        other => {
            ArtifactError::unavailable(format!("the file could not be {doing}")).with_source(other)
        }
    }
}

impl ArtifactStore for S3Artifacts {
    async fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
        meta.check(key, &bytes)?;
        let mut attributes = Attributes::new();
        attributes.insert(Attribute::ContentType, meta.media_type.clone().into());
        attributes.insert(
            Attribute::Metadata(META_SHA256.into()),
            key.sha256_hex().into(),
        );
        if let Some(name) = &meta.filename {
            attributes.insert(
                Attribute::Metadata(META_FILENAME.into()),
                utf8_percent_encode(name, NON_ALPHANUMERIC)
                    .to_string()
                    .into(),
            );
        }
        let options = PutOptions {
            attributes,
            ..PutOptions::default()
        };
        self.store
            .put_opts(&self.path_of(key), PutPayload::from_bytes(bytes), options)
            .await
            .map(|_| ())
            .map_err(|e| map_error(e, "written"))
    }

    async fn get(
        &self,
        key: &ArtifactKey,
    ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
        let found = match self.store.get(&self.path_of(key)).await {
            Ok(found) => found,
            Err(object_store::Error::NotFound { .. }) => return Ok(None),
            Err(e) => return Err(map_error(e, "read")),
        };
        let corrupt = |what: &str| ArtifactError::Corrupt(what.to_owned());
        let media_type = match found.attributes.get(&Attribute::ContentType) {
            Some(value) => value.as_ref().to_owned(),
            None => return Err(corrupt("the object has no content type")),
        };
        let metadata = |name: &'static str| {
            found
                .attributes
                .get(&Attribute::Metadata(name.into()))
                .map(|v| v.as_ref().to_owned())
        };
        if metadata(META_SHA256).is_some_and(|hash| hash != key.sha256_hex()) {
            return Err(corrupt("the object names another hash than its key"));
        }
        let filename = match metadata(META_FILENAME) {
            Some(encoded) => Some(
                percent_decode_str(&encoded)
                    .decode_utf8()
                    .map_err(|_| corrupt("the object's file name is not text"))?
                    .into_owned(),
            ),
            None => None,
        };
        let meta = ArtifactMeta {
            media_type,
            size: found.meta.size,
            sha256: *key.sha256(),
            filename,
        };
        let pieces = found
            .into_stream()
            .map(|piece| piece.map_err(|e| map_error(e, "read")));
        Ok(Some((meta, Box::pin(pieces))))
    }

    async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
        match self.store.delete(&self.path_of(key)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(e) => Err(map_error(e, "removed")),
        }
    }

    async fn copy(&self, from: &ArtifactKey, to: &ArtifactKey) -> Result<(), ArtifactError> {
        from.check_copy_to(to)?;
        if from == to {
            // S3 refuses a copy of an object onto itself that changes nothing; there is nothing to
            // do but to say whether the file is there.
            return match self.store.head(&self.path_of(from)).await {
                Ok(_) => Ok(()),
                Err(object_store::Error::NotFound { .. }) => Err(ArtifactError::NotFound),
                Err(e) => Err(map_error(e, "read")),
            };
        }
        // One `CopyObject` on the server's side: the bytes do not come here, and the object's
        // content type and user metadata (the hash and the file name) are copied with it. The key
        // is the content's hash and the server holds the object whole, so there is nothing to
        // re-hash: a source whose own hash metadata names another hash is what `get` reports.
        match self
            .store
            .copy(&self.path_of(from), &self.path_of(to))
            .await
        {
            Ok(()) => Ok(()),
            Err(object_store::Error::NotFound { .. }) => Err(ArtifactError::NotFound),
            Err(e) => Err(map_error(e, "copied")),
        }
    }

    async fn delete_prefix(&self, thread: ThreadId) -> Result<u64, ArtifactError> {
        // The thread's own part of the bucket: `[<prefix>/]threads/<uuid>/`, which a listing under
        // the path (it lists whole path segments, so no other thread's) pages through. Each object
        // is one `DELETE` (the bulk call is turned off, see `new`), twenty at a time.
        let thread = thread.to_string();
        let prefix: Path = self
            .prefix
            .iter()
            .map(String::as_str)
            .chain(["threads", thread.as_str()])
            .collect();
        let objects = self
            .store
            .list(Some(&prefix))
            .map_ok(|meta| meta.location)
            .boxed();
        self.store
            .delete_stream(objects)
            .try_fold(0u64, |n, _| futures::future::ready(Ok(n + 1)))
            .await
            .map_err(|e| map_error(e, "removed"))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use orch_core::ThreadId;

    use super::*;

    fn credentials(config: S3Config) -> S3Config {
        config.with_credentials(
            SecretString::from("AKIDEXAMPLE"),
            SecretString::from("s3cr3t-access-key"),
        )
    }

    #[test]
    fn a_store_needs_credentials_and_says_nothing_of_them() {
        let err = S3Artifacts::new(S3Config::new("bucket")).unwrap_err();
        assert!(err.to_string().contains("access key"), "{err}");
        let config = credentials(S3Config::new("bucket").with_endpoint("http://minio:9000"));
        let shown = format!("{config:?}");
        assert!(
            !shown.contains("AKIDEXAMPLE") && !shown.contains("s3cr3t"),
            "{shown}"
        );
        assert!(shown.contains("<redacted>"), "{shown}");
        let store = S3Artifacts::new(config).unwrap();
        let shown = format!("{store:?}");
        assert!(
            shown.contains("bucket") && !shown.contains("s3cr3t"),
            "{shown}"
        );
    }

    #[test]
    fn a_key_is_an_object_under_the_prefix_made_of_a_uuid_and_hex_alone() {
        let hash = [0xabu8; 32];
        let key = ArtifactKey::new(ThreadId(uuid::Uuid::from_u128(1)), hash);
        let path = |prefix: Option<&str>| {
            let mut config =
                credentials(S3Config::new("bucket").with_endpoint("http://minio:9000"));
            if let Some(prefix) = prefix {
                config = config.with_prefix(prefix);
            }
            S3Artifacts::new(config).unwrap().path_of(&key).to_string()
        };
        let tail = format!("threads/{}/{}", key.thread(), key.sha256_hex());
        assert_eq!(path(None), tail);
        assert_eq!(path(Some("")), tail);
        assert_eq!(path(Some("files/prod")), format!("files/prod/{tail}"));
        assert_eq!(path(Some("/files//prod/")), format!("files/prod/{tail}"));
    }
}
