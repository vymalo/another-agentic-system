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
//! **Presigned uploads** (ADR 0058, [`uploads/v1`](../../../docs/api/uploads-v1.md)). With a
//! **public endpoint** ([`S3Config::with_public_endpoint`]) the store can presign: a SigV4 request
//! signs the `Host`, so a URL for the agent must be signed for the host the agent reaches, which is
//! not the Service the orchestrator reaches the bucket at. The store builds a second signer with the
//! same bucket and credentials and the public endpoint and uses it to **sign only**: no request
//! ever goes to the public host from here. [`presign_put`](ArtifactStore::presign_put) signs a `PUT`
//! for one key with four headers folded into the signature (the declared content's
//! `x-amz-checksum-sha256`, its `content-type`, and the two `x-amz-meta-*` the store writes with a
//! plain put), so the bucket refuses other bytes, another type or another key, and the object the
//! agent leaves reads back exactly like one this store put. [`presign_get`](ArtifactStore::presign_get)
//! signs a `GET` for the same host that a download is redirected to (ADR 0058 A1): the
//! `response-content-disposition`, `response-content-type` and `response-cache-control` query
//! parameters are folded into the signature, so the holder of the URL cannot change how the file is
//! sent. [`head`](ArtifactStore::head) is one
//! `HEAD` signed for the internal endpoint, asking for the checksum (`x-amz-checksum-mode`): the size,
//! the type, the file name, the time and, **when the server says it**, the SHA-256 it stored.
//!
//! Requests are bounded: a timeout on each, and a retry budget of two retries within 30 seconds
//! (`object_store` retries connection errors and 5xx itself), after which the caller sees a
//! transient [`ArtifactError::Unavailable`] and decides, as for any other port.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use futures::{StreamExt as _, TryStreamExt as _};
use jiff::Timestamp;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path;
use object_store::signer::{HeaderName, HeaderValue, Method, SignedUrlOptions, Signer as _};
use object_store::{
    Attribute, Attributes, ClientOptions, ObjectStore, ObjectStoreExt as _, PutOptions, PutPayload,
    RetryConfig,
};
use orch_core::ThreadId;
use orch_ports::{
    ArtifactError, ArtifactHead, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream,
    MAX_PRESIGN_TTL, PresignedGet, PresignedPut, ResponseOverrides,
};
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
/// The header that asks a `GET` or `HEAD` to say the object's checksum.
const CHECKSUM_MODE: &str = "x-amz-checksum-mode";
/// The header that carries the SHA-256 of a `PUT`'s body, base64, and says it on a `HEAD`.
const CHECKSUM_SHA256: &str = "x-amz-checksum-sha256";
/// Says the checksum is the whole object's (a multipart upload's is a checksum of checksums).
const CHECKSUM_TYPE: &str = "x-amz-checksum-type";
/// The query parameters of a signed `GetObject` that change the headers of the answer.
const RESPONSE_CACHE_CONTROL: &str = "response-cache-control";
const RESPONSE_CONTENT_DISPOSITION: &str = "response-content-disposition";
const RESPONSE_CONTENT_TYPE: &str = "response-content-type";
/// How long the signature of a `HEAD` this store sends to itself lives.
const HEAD_URL_TTL: Duration = Duration::from_secs(60);
/// How many times a `HEAD` is tried, and how long before the second try (doubled for the third).
const HEAD_ATTEMPTS: u32 = 3;
const HEAD_BACKOFF: Duration = Duration::from_millis(250);

/// What the store is built from.
#[derive(Clone)]
pub struct S3Config {
    bucket: String,
    region: String,
    endpoint: Option<String>,
    public_endpoint: Option<String>,
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
            public_endpoint: None,
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

    /// The URL agents reach the bucket at (`https://agentic-files.example.com`), when it is not the
    /// one this store does: **the host of every presigned URL** (SigV4 signs the `Host`, so a URL for
    /// the internal Service does not work at the public name). The store signs for it and never calls
    /// it. Without one nothing is presigned ([`ArtifactStore::can_presign`] is `false`). As for
    /// [`with_endpoint`](Self::with_endpoint), the bucket is addressed in the path.
    #[must_use]
    pub fn with_public_endpoint(mut self, public_endpoint: impl Into<String>) -> Self {
        self.public_endpoint = Some(public_endpoint.into());
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
            .field("public_endpoint", &self.public_endpoint)
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
    /// The bucket as this process reaches it: every call except signing for an agent.
    store: Arc<AmazonS3>,
    /// The same bucket and credentials at the public endpoint, **to sign with and nothing else**;
    /// `None` when there is no public endpoint, and then nothing is presigned.
    signer: Option<Arc<AmazonS3>>,
    /// For the one request `object_store` has no call for: a `HEAD` that asks for the checksum.
    http: reqwest::Client,
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
        let store = build_client(&config, config.endpoint.as_deref())?;
        // The signer for the agents: the same bucket and credentials at the public endpoint. It is
        // never called, only asked to sign.
        let signer = config
            .public_endpoint
            .as_deref()
            .map(|public| build_client(&config, Some(public)))
            .transpose()?
            .map(Arc::new);
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .connect_timeout(Duration::from_secs(10))
            // a redirect would send the signed request somewhere else
            .redirect(reqwest::redirect::Policy::none())
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
            signer,
            http,
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

/// One `object_store` client for `config`'s bucket and credentials, at `endpoint` (path style) or,
/// with none, at AWS (the bucket as a host name).
fn build_client(config: &S3Config, endpoint: Option<&str>) -> Result<AmazonS3, BuildError> {
    let bad = |what: &str| BuildError::Config(what.to_owned());
    let Some((access_key_id, secret_access_key)) = &config.credentials else {
        return Err(bad("an access key id and a secret access key are required"));
    };
    let allow_http = endpoint.is_some_and(|e| e.starts_with("http://"));
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
    if let Some(endpoint) = endpoint {
        builder = builder.with_endpoint(endpoint);
    } else {
        // AWS addresses a bucket as a host name.
        builder = builder.with_virtual_hosted_style_request(true);
    }
    builder
        .build()
        .map_err(|e| BuildError::Config(strip_secrets(&e.to_string(), config)))
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
            return Err(ArtifactError::another_hash());
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

    fn can_presign(&self) -> bool {
        self.signer.is_some()
    }

    async fn presign_put(
        &self,
        key: &ArtifactKey,
        meta: &ArtifactMeta,
        ttl: Duration,
    ) -> Result<PresignedPut, ArtifactError> {
        let Some(signer) = &self.signer else {
            return Err(ArtifactError::Unsupported);
        };
        meta.check_declared(key)?;
        if ttl.is_zero() || ttl > MAX_PRESIGN_TTL {
            return Err(ArtifactError::Invalid(
                "the lifetime of a presigned URL is from one second to seven days".to_owned(),
            ));
        }
        // What the signature binds, and so what the request must carry, exactly: the checksum of
        // the declared content (the server checks the body against it), the type, and the two user
        // metadata a plain `put` writes, so that the object the agent leaves reads back (the hash
        // and the name it is served under) like one this store put.
        let mut headers = BTreeMap::from([
            ("content-type".to_owned(), meta.media_type.clone()),
            (
                CHECKSUM_SHA256.to_owned(),
                base64::engine::general_purpose::STANDARD.encode(key.sha256()),
            ),
            (format!("x-amz-meta-{META_SHA256}"), key.sha256_hex()),
        ]);
        if let Some(name) = &meta.filename {
            headers.insert(
                format!("x-amz-meta-{META_FILENAME}"),
                utf8_percent_encode(name, NON_ALPHANUMERIC).to_string(),
            );
        }
        let mut options = SignedUrlOptions::default();
        for (name, value) in &headers {
            let invalid = || ArtifactError::Invalid("a header cannot carry that text".to_owned());
            options = options.with_signed_header(
                HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid())?,
                HeaderValue::from_str(value).map_err(|_| invalid())?,
            );
        }
        // Signing is local: no request goes to the public host (or to any host).
        let url = signer
            .signed_url_opts(Method::PUT, &self.path_of(key), ttl, &options)
            .await
            .map_err(|e| map_error(e, "signed"))?;
        Ok(PresignedPut {
            url: url.to_string(),
            headers,
        })
    }

    async fn presign_get(
        &self,
        key: &ArtifactKey,
        overrides: &ResponseOverrides,
        ttl: Duration,
    ) -> Result<PresignedGet, ArtifactError> {
        let Some(signer) = &self.signer else {
            return Err(ArtifactError::Unsupported);
        };
        overrides.check()?;
        // `X-Amz-Expires` is whole seconds, and S3 refuses 0
        if ttl.as_secs() == 0 || ttl > MAX_PRESIGN_TTL {
            return Err(ArtifactError::Invalid(
                "the lifetime of a presigned URL is from one second to seven days".to_owned(),
            ));
        }
        // The three members S3 takes on a signed `GetObject` to change the answer's headers. They
        // are query parameters of the signature, so the URL's holder cannot change or add one.
        let query: Vec<(&str, &str)> = [
            (RESPONSE_CACHE_CONTROL, &overrides.cache_control),
            (RESPONSE_CONTENT_DISPOSITION, &overrides.content_disposition),
            (RESPONSE_CONTENT_TYPE, &overrides.content_type),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.as_deref().map(|value| (name, value)))
        .collect();
        let options = SignedUrlOptions::default().with_query(query);
        let ttl = Duration::from_secs(ttl.as_secs());
        // `X-Amz-Date` is the second the signing began, floored, and the URL works for `ttl` seconds
        // from it. Taken before signing and floored the same way, this is never later than that.
        let expires_at = Timestamp::from_second(Timestamp::now().as_second())
            .and_then(|from| from.checked_add(ttl))
            .map_err(|_| ArtifactError::Invalid("the lifetime is out of range".to_owned()))?;
        // Signing is local: no request goes to the public host (or to any host).
        let url = signer
            .signed_url_opts(Method::GET, &self.path_of(key), ttl, &options)
            .await
            .map_err(|e| map_error(e, "signed"))?;
        Ok(PresignedGet {
            url: url.to_string(),
            expires_at,
        })
    }

    async fn head(&self, key: &ArtifactKey) -> Result<Option<ArtifactHead>, ArtifactError> {
        // One `HEAD`, signed for the internal endpoint with the header that asks for the checksum:
        // `object_store` has no call that returns it. **Every attempt is signed afresh**: a
        // signature lives `HEAD_URL_TTL` and an attempt may take the whole timeout, so the retry of
        // one that was slow must not carry a signature that is about to run out.
        let path = self.path_of(key);
        let mut backoff = HEAD_BACKOFF;
        for attempt in 1..=HEAD_ATTEMPTS {
            let last = attempt == HEAD_ATTEMPTS;
            let options = SignedUrlOptions::default().with_signed_header(
                HeaderName::from_static(CHECKSUM_MODE),
                HeaderValue::from_static("ENABLED"),
            );
            let url = self
                .store
                .signed_url_opts(Method::HEAD, &path, HEAD_URL_TTL, &options)
                .await
                .map_err(|e| map_error(e, "read"))?;
            match self
                .http
                .head(url)
                .header(CHECKSUM_MODE, "ENABLED")
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        return head_of(key, response.headers()).map(Some);
                    }
                    if status == reqwest::StatusCode::NOT_FOUND {
                        return Ok(None);
                    }
                    if matches!(
                        status,
                        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
                    ) {
                        return Err(ArtifactError::Unauthenticated);
                    }
                    if last || !status.is_server_error() {
                        return Err(ArtifactError::unavailable(format!(
                            "the file could not be read (HTTP {status})"
                        )));
                    }
                }
                // the URL carries the signature: it is not kept in the error
                Err(e) if last => {
                    return Err(ArtifactError::unavailable("the file could not be read")
                        .with_source(e.without_url()));
                }
                Err(_) => {}
            }
            tokio::time::sleep(backoff).await;
            backoff *= 2;
        }
        Err(ArtifactError::unavailable("the file could not be read"))
    }

    async fn read_prefix(
        &self,
        key: &ArtifactKey,
        len: usize,
    ) -> Result<Option<Bytes>, ArtifactError> {
        let path = self.path_of(key);
        // A range of nothing is not a range: say whether the file is there.
        let ranged = match u64::try_from(len) {
            Ok(len) if len > 0 => self.store.get_range(&path, 0..len).await,
            _ => self.store.head(&path).await.map(|_| Bytes::new()),
        };
        match ranged {
            Ok(bytes) => Ok(Some(bytes)),
            Err(object_store::Error::NotFound { .. }) => Ok(None),
            // A server answers a range of an empty object with an error; an empty file is a file.
            Err(error) => match self.store.head(&path).await {
                Ok(meta) if meta.size == 0 => Ok(Some(Bytes::new())),
                Err(object_store::Error::NotFound { .. }) => Ok(None),
                Ok(_) | Err(_) => Err(map_error(error, "read")),
            },
        }
    }
}

/// What the headers of a `HEAD` say of the object. The size is required; the rest is whatever the
/// server says, and a header it says badly is not said at all. An object that names **another hash
/// than its key** is [`ArtifactError::Corrupt`], as for [`ArtifactStore::get`]: one that names none
/// (an object put by someone else) is not.
fn head_of(
    key: &ArtifactKey,
    headers: &reqwest::header::HeaderMap,
) -> Result<ArtifactHead, ArtifactError> {
    use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE, LAST_MODIFIED};
    let text = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    let size = text(CONTENT_LENGTH.as_str())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .ok_or_else(|| ArtifactError::Corrupt("the object has no size".to_owned()))?;
    if text(&format!("x-amz-meta-{META_SHA256}")).is_some_and(|hash| hash != key.sha256_hex()) {
        return Err(ArtifactError::another_hash());
    }
    // the checksum of the whole object, not of its parts
    let whole = text(CHECKSUM_TYPE).is_none_or(|t| t.eq_ignore_ascii_case("FULL_OBJECT"));
    let sha256 = text(CHECKSUM_SHA256)
        .filter(|_| whole)
        .and_then(|v| {
            base64::engine::general_purpose::STANDARD
                .decode(v.trim())
                .ok()
        })
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
    // the name a plain put writes: percent-encoded, because a metadata value is ASCII; a name that
    // is not text is not said
    let filename = text(&format!("x-amz-meta-{META_FILENAME}")).and_then(|encoded| {
        percent_decode_str(encoded)
            .decode_utf8()
            .ok()
            .map(std::borrow::Cow::into_owned)
    });
    Ok(ArtifactHead {
        size,
        media_type: text(CONTENT_TYPE.as_str()).map(str::to_owned),
        filename,
        sha256,
        last_modified: text(LAST_MODIFIED.as_str())
            .and_then(|v| jiff::fmt::rfc2822::parse(v).ok())
            .map(|zoned| zoned.timestamp()),
    })
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
    fn a_time_the_server_does_not_say_or_says_badly_is_not_said_and_is_never_a_default() {
        use reqwest::header::{HeaderMap, HeaderValue};
        let key = ArtifactKey::new(ThreadId(uuid::Uuid::from_u128(1)), [0xab; 32]);
        let head = |last_modified: Option<&str>| {
            let mut headers = HeaderMap::new();
            headers.insert("content-length", HeaderValue::from_static("7"));
            if let Some(v) = last_modified {
                headers.insert("last-modified", HeaderValue::from_str(v).unwrap());
            }
            head_of(&key, &headers).unwrap().last_modified
        };
        let said = head(Some("Wed, 14 Oct 2026 02:35:12 GMT")).expect("a time");
        assert_eq!(said.as_second(), 1_791_945_312);
        // none, or one that is not a date: the caller is told "unknown" and decides (a caller that
        // checks the time reads `None` as late once the URL has stopped working)
        for given in [
            None,
            Some(""),
            Some("yesterday"),
            Some("2026-10-14T02:35:12Z"),
            Some("Wed, 32 Oct 2026 02:35:12 GMT"),
        ] {
            assert_eq!(head(given), None, "{given:?}");
        }
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
