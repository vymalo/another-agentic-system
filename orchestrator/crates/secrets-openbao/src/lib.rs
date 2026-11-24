//! [`SecretStore`] in a self-hosted OpenBao (or Vault) **KV v2** mount (ADR 0046, ADR 0009).
//!
//! One path per key, named by a hash of `(owner, server)`. A secret is the field `m` (base64 of the
//! material) beside `k`, the same hash, which `get` checks so a path moved to another key reads as
//! corrupt. OpenBao seals what it keeps; nothing here seals twice.
//!
//! - **Versions are KV's own.** They count every write of a path and are never reset, because the
//!   adapter never deletes a path's metadata.
//! - **`put`** is a plain write; KV serializes the writers of a path and numbers them as they land.
//! - **`rotate`** is a write with `cas` set to the expected version, which KV refuses when the
//!   path moved.
//! - **`delete`** writes a tombstone as a new version (so the version is never given twice and no
//!   caller can hold it), then destroys the older versions. It is idempotent.
//!
//! No error holds a URL, a path, a token, an owner or material: reqwest errors are reduced to a
//! phrase here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use orch_core::UserId;
use orch_ports::{SecretError, SecretMaterial, SecretServer, SecretStore, SecretVersion};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

/// How to reach the KV v2 mount.
#[derive(Debug)]
pub struct OpenBaoConfig {
    /// The server's address, e.g. `https://bao.internal:8200`.
    pub address: String,
    /// The KV v2 mount, e.g. `secret`.
    pub mount: String,
    /// A prefix under the mount for this deployment's secrets, e.g. `orchestrator`.
    pub prefix: String,
    /// The token the adapter uses (read from a Secret by the caller, never a literal in a file).
    pub token: SecretString,
    /// The per-request timeout.
    pub timeout: Duration,
}

/// The adapter.
pub struct OpenBaoSecretStore {
    http: reqwest::Client,
    base: String,
    prefix: String,
    token: SecretString,
    closed: AtomicBool,
}

impl std::fmt::Debug for OpenBaoSecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OpenBaoSecretStore")
    }
}

#[derive(Deserialize)]
struct WriteReply {
    data: WriteData,
}
#[derive(Deserialize)]
struct WriteData {
    version: u64,
}
#[derive(Deserialize)]
struct ReadReply {
    data: ReadData,
}
#[derive(Deserialize)]
struct ReadData {
    data: Option<Value>,
    metadata: ReadMeta,
}
#[derive(Deserialize)]
struct ReadMeta {
    version: u64,
}
#[derive(Deserialize)]
struct MetaReply {
    data: MetaData,
}
#[derive(Deserialize)]
struct MetaData {
    versions: std::collections::BTreeMap<String, VersionMeta>,
}
#[derive(Deserialize)]
struct VersionMeta {
    #[serde(default)]
    destroyed: bool,
}

impl OpenBaoSecretStore {
    /// Builds the client.
    ///
    /// # Errors
    /// [`SecretError::Unavailable`] when the HTTP client cannot be built.
    pub fn new(config: OpenBaoConfig) -> Result<Self, SecretError> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| SecretError::unavailable("the OpenBao client could not be built"))?;
        let trim = |s: &str| s.trim_matches('/').to_owned();
        Ok(Self {
            http,
            base: format!(
                "{}/v1/{}",
                config.address.trim_end_matches('/'),
                trim(&config.mount)
            ),
            prefix: trim(&config.prefix),
            token: config.token,
            closed: AtomicBool::new(false),
        })
    }

    /// Makes every later call fail as an unreachable server does (for the testkit's breaker).
    pub async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    fn name(owner: &UserId, server: &SecretServer) -> String {
        let mut h = Sha256::new();
        h.update(b"orch-secret-v1\0");
        h.update(owner.to_string().as_bytes());
        h.update(b"\0");
        h.update(server.to_string().as_bytes());
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    fn url(&self, kind: &str, name: &str) -> String {
        if self.prefix.is_empty() {
            format!("{}/{kind}/{name}", self.base)
        } else {
            format!("{}/{kind}/{}/{name}", self.base, self.prefix)
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        url: String,
        body: Option<Value>,
    ) -> Result<(reqwest::StatusCode, Option<Value>), SecretError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(SecretError::unavailable("the OpenBao client is closed"));
        }
        let mut req = self
            .http
            .request(method, url)
            .header("X-Vault-Token", self.token.expose_secret());
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req
            .send()
            .await
            .map_err(|_| SecretError::unavailable("OpenBao could not be reached"))?;
        let status = resp.status();
        if status.is_server_error()
            || status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Err(SecretError::unavailable(format!(
                "OpenBao answered {}",
                status.as_u16()
            )));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|_| SecretError::unavailable("OpenBao's answer could not be read"))?;
        let value = serde_json::from_slice::<Value>(&bytes).ok();
        Ok((status, value))
    }

    async fn write(
        &self,
        name: &str,
        data: Value,
        cas: Option<u64>,
    ) -> Result<Result<u64, ()>, SecretError> {
        let mut body = json!({ "data": data });
        if let Some(c) = cas {
            body["options"] = json!({ "cas": c });
        }
        let (status, value) = self
            .send(reqwest::Method::POST, self.url("data", name), Some(body))
            .await?;
        if status.is_success() {
            let reply: WriteReply =
                serde_json::from_value(value.unwrap_or(Value::Null)).map_err(|_| {
                    SecretError::corrupt("OpenBao's write answer has an unexpected shape")
                })?;
            return Ok(Ok(reply.data.version));
        }
        if cas.is_some() && status == reqwest::StatusCode::BAD_REQUEST {
            return Ok(Err(()));
        }
        Err(SecretError::unavailable(format!(
            "OpenBao refused a write with {}",
            status.as_u16()
        )))
    }

    fn payload(name: &str, material: &SecretMaterial) -> Value {
        json!({ "k": name, "m": B64.encode(material.expose()) })
    }
}

impl SecretStore for OpenBaoSecretStore {
    async fn put(
        &self,
        owner: &UserId,
        server: &SecretServer,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        let name = Self::name(owner, server);
        match self
            .write(&name, Self::payload(&name, material), None)
            .await?
        {
            Ok(v) => Ok(SecretVersion::new(v)),
            Err(()) => Err(SecretError::unavailable("OpenBao refused a write")),
        }
    }

    async fn get(
        &self,
        owner: &UserId,
        server: &SecretServer,
    ) -> Result<Option<(SecretVersion, SecretMaterial)>, SecretError> {
        let name = Self::name(owner, server);
        let (status, value) = self
            .send(reqwest::Method::GET, self.url("data", &name), None)
            .await?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(SecretError::unavailable(format!(
                "OpenBao refused a read with {}",
                status.as_u16()
            )));
        }
        let reply: ReadReply = serde_json::from_value(value.unwrap_or(Value::Null))
            .map_err(|_| SecretError::corrupt("OpenBao's read answer has an unexpected shape"))?;
        let Some(data) = reply.data.data else {
            return Ok(None);
        };
        if data.get("t").and_then(Value::as_bool) == Some(true) {
            return Ok(None);
        }
        if data.get("k").and_then(Value::as_str) != Some(name.as_str()) {
            return Err(SecretError::corrupt(
                "a stored secret is not under its own key",
            ));
        }
        let bytes = data
            .get("m")
            .and_then(Value::as_str)
            .and_then(|m| B64.decode(m).ok())
            .ok_or_else(|| SecretError::corrupt("a stored secret does not decode"))?;
        let material = SecretMaterial::new(bytes)
            .map_err(|_| SecretError::corrupt("a stored secret is over the size limit"))?;
        Ok(Some((
            SecretVersion::new(reply.data.metadata.version),
            material,
        )))
    }

    async fn delete(&self, owner: &UserId, server: &SecretServer) -> Result<(), SecretError> {
        let name = Self::name(owner, server);
        // Nothing there: nothing to do (and no tombstone to keep).
        let (status, _) = self
            .send(reqwest::Method::GET, self.url("metadata", &name), None)
            .await?;
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        let Ok(tomb) = self.write(&name, json!({ "t": true }), None).await? else {
            return Err(SecretError::unavailable("OpenBao refused a write"));
        };
        // Destroy what the tombstone replaced. Best effort on the listing; a failure is reported.
        let (status, value) = self
            .send(reqwest::Method::GET, self.url("metadata", &name), None)
            .await?;
        if !status.is_success() {
            return Ok(());
        }
        let Ok(meta) = serde_json::from_value::<MetaReply>(value.unwrap_or(Value::Null)) else {
            return Ok(());
        };
        let old: Vec<u64> = meta
            .data
            .versions
            .iter()
            .filter(|(_, m)| !m.destroyed)
            .filter_map(|(v, _)| v.parse::<u64>().ok())
            .filter(|v| *v < tomb)
            .collect();
        if old.is_empty() {
            return Ok(());
        }
        let (status, _) = self
            .send(
                reqwest::Method::PUT,
                self.url("destroy", &name),
                Some(json!({ "versions": old })),
            )
            .await?;
        if status.is_success() {
            Ok(())
        } else {
            Err(SecretError::unavailable(
                "OpenBao refused to destroy old versions",
            ))
        }
    }

    async fn rotate(
        &self,
        owner: &UserId,
        server: &SecretServer,
        expected: SecretVersion,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        // cas 0 would mean "only if absent", which is a create and not a rotate.
        if expected.get() == 0 {
            return Err(SecretError::Conflict);
        }
        let name = Self::name(owner, server);
        match self
            .write(&name, Self::payload(&name, material), Some(expected.get()))
            .await?
        {
            Ok(v) => Ok(SecretVersion::new(v)),
            Err(()) => Err(SecretError::Conflict),
        }
    }
}
