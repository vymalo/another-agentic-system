//! The artifact store of this process (ADR 0032, ADR 0009): the one place that knows the two
//! implementations of the `ArtifactStore` port, [`orch_artifacts_fs`] and [`orch_artifacts_s3`], each
//! behind a Cargo feature of this binary, and that chooses between them from the configuration
//! (`artifacts.store`). The application sees only the port.
//!
//! The store is built, checked where it can be without a request, and handed to the `PortSet`; the
//! dispatcher keeps the files agents hand over in it (ADR 0032) and the API serves them. A
//! configuration that names no store gets [`NoArtifacts`], which refuses every call with "no
//! artifact store configured".

use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use orch_ports::{
    ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream, NoArtifacts,
};
use secrecy::SecretString;

/// What `artifacts` of the configuration file says, in the terms of this process. (A build with
/// neither store feature reads none of it: the configuration refuses a store it lacks.)
#[cfg_attr(
    not(any(feature = "artifacts-fs", feature = "artifacts-s3")),
    allow(dead_code)
)]
#[derive(Clone, Debug)]
pub struct ArtifactSettings {
    /// Which store, with what it needs.
    pub store: StoreSettings,
    /// `artifacts.maxFileBytes`: the largest file kept. The ingest checks it before it calls the
    /// port; the store enforces no limit.
    pub max_file_bytes: u64,
    /// `artifacts.maxPerJobBytes`: the most bytes of files one job keeps.
    pub max_per_job_bytes: u64,
    /// `artifacts.fetchHosts`: the hosts a `url` part of an artifact is fetched from (none: a
    /// `url` stays a link).
    pub fetch_hosts: Vec<String>,
}

/// The store `artifacts.store` names. (A build reads the fields of the stores it has.)
#[derive(Clone, Debug)]
pub enum StoreSettings {
    /// `artifacts.fs`: a directory.
    #[cfg_attr(not(feature = "artifacts-fs"), allow(dead_code))]
    Fs {
        /// `artifacts.fs.root`, resolved against the directory of the configuration file.
        root: PathBuf,
    },
    /// `artifacts.s3`: a bucket.
    #[cfg_attr(not(feature = "artifacts-s3"), allow(dead_code))]
    S3(S3Settings),
}

/// `artifacts.s3`. The two credentials are `SecretString`s and `Debug` shows neither.
#[cfg_attr(not(feature = "artifacts-s3"), allow(dead_code))]
#[derive(Clone)]
pub struct S3Settings {
    /// `bucket`.
    pub bucket: String,
    /// `region`.
    pub region: String,
    /// `endpoint`: a server other than AWS.
    pub endpoint: Option<String>,
    /// `prefix`: put every key under it.
    pub prefix: Option<String>,
    /// `accessKeyId`, resolved.
    pub access_key_id: SecretString,
    /// `secretAccessKey`, resolved.
    pub secret_access_key: SecretString,
    /// `timeoutSecs`.
    pub timeout: Duration,
}

impl std::fmt::Debug for S3Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Settings")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("prefix", &self.prefix)
            .field("credentials", &"<redacted>")
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// The one `ArtifactStore` type of the binary's `PortSet`: no store, or the one the configuration
/// names, as far as this build has it. Static dispatch over the variants (ADR 0009), so a
/// deployment with a directory, one with a bucket and one with neither are the same build.
#[derive(Debug, Clone)]
pub enum ConfiguredArtifacts {
    /// No store: every call is `ArtifactError::NotConfigured`.
    Off(NoArtifacts),
    /// A directory (`artifacts-fs`).
    #[cfg(feature = "artifacts-fs")]
    Fs(orch_artifacts_fs::FsArtifacts),
    /// A bucket (`artifacts-s3`).
    #[cfg(feature = "artifacts-s3")]
    S3(orch_artifacts_s3::S3Artifacts),
}

impl ConfiguredArtifacts {
    /// The store `settings` name, or none. A directory is made and checked writable here, so a
    /// root that cannot be used stops startup; a bucket is not contacted (a control plane does not
    /// wait for a bucket to start).
    ///
    /// # Errors
    /// The words for the operator: the settings name a store this build does not have, or the
    /// store cannot be built. Never a credential.
    pub async fn build(settings: Option<&ArtifactSettings>) -> Result<Self, String> {
        let Some(settings) = settings else {
            return Ok(ConfiguredArtifacts::Off(NoArtifacts));
        };
        match &settings.store {
            #[cfg(feature = "artifacts-fs")]
            StoreSettings::Fs { root } => {
                let store = orch_artifacts_fs::FsArtifacts::open(root)
                    .await
                    .map_err(|e| format!("artifacts.fs.root: {e}"))?;
                tracing::info!(
                    root = %root.display(),
                    max_file_bytes = settings.max_file_bytes,
                    "files from agents are kept in a directory"
                );
                Ok(ConfiguredArtifacts::Fs(store))
            }
            #[cfg(feature = "artifacts-s3")]
            StoreSettings::S3(s3) => {
                let mut config = orch_artifacts_s3::S3Config::new(&s3.bucket)
                    .with_region(&s3.region)
                    .with_credentials(s3.access_key_id.clone(), s3.secret_access_key.clone())
                    .with_timeout(s3.timeout);
                if let Some(endpoint) = &s3.endpoint {
                    config = config.with_endpoint(endpoint);
                    if endpoint.starts_with("http://") {
                        tracing::warn!(
                            "the S3 endpoint is plain http: files travel in the clear; use https outside development"
                        );
                    }
                }
                if let Some(prefix) = &s3.prefix {
                    config = config.with_prefix(prefix);
                }
                let store = orch_artifacts_s3::S3Artifacts::new(config)
                    .map_err(|e| format!("artifacts.s3: {e}"))?;
                tracing::info!(
                    bucket = %s3.bucket,
                    region = %s3.region,
                    endpoint = s3.endpoint.as_deref().unwrap_or("aws"),
                    max_file_bytes = settings.max_file_bytes,
                    "files from agents are kept in an S3 bucket"
                );
                Ok(ConfiguredArtifacts::S3(store))
            }
            // The store is named and this build lacks it: `config` refused that at load, so this
            // is only for a caller that built the settings by hand.
            #[allow(unreachable_patterns)]
            other => Err(format!(
                "artifacts.store: this build has no {} store (enable the Cargo feature \"{}\")",
                match other {
                    StoreSettings::Fs { .. } => "fs",
                    StoreSettings::S3(_) => "s3",
                },
                match other {
                    StoreSettings::Fs { .. } => "artifacts-fs",
                    StoreSettings::S3(_) => "artifacts-s3",
                },
            )),
        }
    }
}

impl ArtifactStore for ConfiguredArtifacts {
    async fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
        match self {
            ConfiguredArtifacts::Off(off) => off.put(key, bytes, meta).await,
            #[cfg(feature = "artifacts-fs")]
            ConfiguredArtifacts::Fs(fs) => fs.put(key, bytes, meta).await,
            #[cfg(feature = "artifacts-s3")]
            ConfiguredArtifacts::S3(s3) => s3.put(key, bytes, meta).await,
        }
    }

    async fn get(
        &self,
        key: &ArtifactKey,
    ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
        match self {
            ConfiguredArtifacts::Off(off) => off.get(key).await,
            #[cfg(feature = "artifacts-fs")]
            ConfiguredArtifacts::Fs(fs) => fs.get(key).await,
            #[cfg(feature = "artifacts-s3")]
            ConfiguredArtifacts::S3(s3) => s3.get(key).await,
        }
    }

    async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
        match self {
            ConfiguredArtifacts::Off(off) => off.delete(key).await,
            #[cfg(feature = "artifacts-fs")]
            ConfiguredArtifacts::Fs(fs) => fs.delete(key).await,
            #[cfg(feature = "artifacts-s3")]
            ConfiguredArtifacts::S3(s3) => s3.delete(key).await,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    #[cfg(feature = "artifacts-fs")]
    use futures::StreamExt as _;
    use orch_core::ThreadId;

    use super::*;

    fn settings(store: StoreSettings) -> ArtifactSettings {
        ArtifactSettings {
            store,
            max_file_bytes: 1024,
            max_per_job_bytes: 4096,
            fetch_hosts: Vec::new(),
        }
    }

    fn s3() -> S3Settings {
        S3Settings {
            bucket: "files".into(),
            region: "us-east-1".into(),
            endpoint: Some("http://127.0.0.1:1".into()),
            prefix: Some("prod".into()),
            access_key_id: SecretString::from("AKIDEXAMPLE"),
            secret_access_key: SecretString::from("s3cr3t-access-key"),
            timeout: Duration::from_secs(5),
        }
    }

    fn file() -> (ArtifactKey, ArtifactMeta, Bytes) {
        let meta = ArtifactMeta::of("text/plain", Some("note.txt".into()), b"kept");
        (
            meta.key(ThreadId(uuid::Uuid::now_v7())),
            meta,
            Bytes::from_static(b"kept"),
        )
    }

    #[tokio::test]
    async fn no_settings_is_no_store_that_says_so() {
        let store = ConfiguredArtifacts::build(None).await.unwrap();
        assert!(matches!(store, ConfiguredArtifacts::Off(_)));
        let (key, meta, bytes) = file();
        let err = store.put(&key, bytes, &meta).await.unwrap_err();
        assert_eq!(err.to_string(), "no artifact store configured");
        assert!(matches!(
            store.get(&key).await,
            Err(ArtifactError::NotConfigured)
        ));
        assert!(matches!(
            store.delete(&key).await,
            Err(ArtifactError::NotConfigured)
        ));
    }

    #[test]
    fn debug_shows_no_credential() {
        let shown = format!("{:?}", settings(StoreSettings::S3(s3())));
        assert!(
            shown.contains("files") && shown.contains("<redacted>"),
            "{shown}"
        );
        assert!(
            !shown.contains("AKIDEXAMPLE") && !shown.contains("s3cr3t"),
            "{shown}"
        );
    }

    #[cfg(feature = "artifacts-fs")]
    #[tokio::test]
    async fn a_directory_store_is_built_and_keeps_a_file() {
        let dir = std::env::temp_dir().join(format!("orch-bin-artifacts-{}", uuid::Uuid::now_v7()));
        let root = dir.join("a").join("b");
        let store =
            ConfiguredArtifacts::build(Some(&settings(StoreSettings::Fs { root: root.clone() })))
                .await
                .unwrap();
        assert!(matches!(store, ConfiguredArtifacts::Fs(_)));
        assert!(root.is_dir(), "the root is made at startup");
        let (key, meta, bytes) = file();
        store.put(&key, bytes, &meta).await.unwrap();
        let (got, mut stream) = store.get(&key).await.unwrap().unwrap();
        assert_eq!(got, meta);
        assert_eq!(stream.next().await.unwrap().unwrap(), &b"kept"[..]);
        store.delete(&key).await.unwrap();
        assert!(store.get(&key).await.unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(feature = "artifacts-fs")]
    #[tokio::test]
    async fn a_root_that_cannot_be_used_stops_startup_naming_the_key() {
        let dir = std::env::temp_dir().join(format!("orch-bin-artifacts-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let blocked = dir.join("a-file");
        std::fs::write(&blocked, b"x").unwrap();
        let err = ConfiguredArtifacts::build(Some(&settings(StoreSettings::Fs { root: blocked })))
            .await
            .unwrap_err();
        assert!(err.starts_with("artifacts.fs.root: cannot use "), "{err}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(feature = "artifacts-s3")]
    #[tokio::test]
    async fn a_bucket_store_is_built_without_a_connection_and_shows_no_credential() {
        // nobody listens on port 1: building must not try
        let store = ConfiguredArtifacts::build(Some(&settings(StoreSettings::S3(s3()))))
            .await
            .unwrap();
        assert!(matches!(store, ConfiguredArtifacts::S3(_)));
        let (key, meta, bytes) = file();
        let err = store.put(&key, bytes, &meta).await.unwrap_err();
        let shown = format!("{err} {err:?} {store:?}");
        assert!(
            !shown.contains("AKIDEXAMPLE") && !shown.contains("s3cr3t"),
            "{shown}"
        );
    }

    /// A store this build does not have is refused, not quietly turned into none.
    #[cfg(not(feature = "artifacts-fs"))]
    #[tokio::test]
    async fn a_directory_store_without_its_feature_is_refused() {
        let err =
            ConfiguredArtifacts::build(Some(&settings(StoreSettings::Fs { root: "x".into() })))
                .await
                .unwrap_err();
        assert!(err.contains("artifacts-fs"), "{err}");
    }

    #[cfg(not(feature = "artifacts-s3"))]
    #[tokio::test]
    async fn a_bucket_store_without_its_feature_is_refused() {
        let err = ConfiguredArtifacts::build(Some(&settings(StoreSettings::S3(s3()))))
            .await
            .unwrap_err();
        assert!(err.contains("artifacts-s3"), "{err}");
    }
}
