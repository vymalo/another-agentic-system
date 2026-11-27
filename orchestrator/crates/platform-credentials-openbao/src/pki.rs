//! The two calls to OpenBao (a Kubernetes login and a PKI signing), the in-memory key and CSR, and
//! the errors of an issue. Nothing here keeps a secret longer than one issue, and nothing here puts
//! one in an error: an error is a stage and a class, a phrase and never a body.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use jiff::SignedDuration;
use orch_ports::{Clock, PlatformClientCertificate, PlatformCredentialsError};
use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, HeaderValue};
use serde::Deserialize;
use serde_json::json;
use url::Url;
use zeroize::Zeroizing;

use crate::leaf::{self, LeafError};
use crate::{BuildError, OpenBaoPkiConfig};

/// The most an answer of OpenBao may weigh: a chain is a few kilobytes.
const MAX_BODY: usize = 256 * 1024;
/// The most a service-account token file may weigh (a JWT is a few kilobytes).
const MAX_TOKEN_FILE: usize = 64 * 1024;

/// Where an issue went wrong. A phrase, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    ServiceAccountToken,
    Login,
    Key,
    Sign,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Stage::ServiceAccountToken => "reading the service-account token",
            Stage::Login => "the Kubernetes login to OpenBao",
            Stage::Key => "making the key and the request",
            Stage::Sign => "the signing request to OpenBao",
        })
    }
}

/// Why no certificate was issued. Each case is a class: [`Refused`](IssueError::Refused) is a
/// fault of the deployment's setup (a role OpenBao does not grant), everything else is "try
/// later" or "what came back is unusable".
#[derive(Debug, thiserror::Error)]
pub(crate) enum IssueError {
    #[error("OpenBao refused {0}")]
    Refused(Stage),
    #[error("OpenBao is unavailable: {0}")]
    Unavailable(Stage),
    #[error("the issued certificate is unusable: {0}")]
    Unusable(#[from] LeafError),
}

impl From<IssueError> for PlatformCredentialsError {
    fn from(error: IssueError) -> Self {
        match error {
            IssueError::Refused(_) => PlatformCredentialsError::Refused,
            IssueError::Unavailable(_) | IssueError::Unusable(_) => {
                PlatformCredentialsError::Unavailable
            }
        }
    }
}

#[derive(Deserialize)]
struct LoginReply {
    auth: LoginAuth,
}

#[derive(Deserialize)]
struct LoginAuth {
    client_token: Zeroizing<String>,
}

#[derive(Deserialize)]
struct SignReply {
    data: SignData,
}

#[derive(Deserialize)]
struct SignData {
    certificate: String,
    #[serde(default)]
    ca_chain: Vec<String>,
    #[serde(default)]
    issuing_ca: Option<String>,
}

/// A key made for one issue: the private half is wiped with the buffer, and only the public point
/// and the request leave the function that made it.
struct Generated {
    pkcs8: Zeroizing<Vec<u8>>,
    public_point: Vec<u8>,
    csr_pem: String,
}

/// Makes an ECDSA P-256 key and a CSR with no subject and no names: the SPIFFE id is the PKI
/// role's to put in the leaf (`uri_sans`), never something the CSR can ask for.
fn generate() -> Result<Generated, IssueError> {
    let fail = || IssueError::Unavailable(Stage::Key);
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).map_err(|_| fail())?;
    let mut params = rcgen::CertificateParams::default();
    // The default has a common name; the request carries none.
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.subject_alt_names = Vec::new();
    let csr = params.serialize_request(&key).map_err(|_| fail())?;
    let csr_pem = csr.pem().map_err(|_| fail())?;
    Ok(Generated {
        pkcs8: Zeroizing::new(key.serialized_der().to_vec()),
        public_point: key.public_key_raw().to_vec(),
        csr_pem,
    })
}

/// What an issue is made of: the HTTP client, the two URLs and the settings that go in the bodies.
pub(crate) struct Issuer {
    http: reqwest::Client,
    login_url: Url,
    sign_url: Url,
    auth_role: String,
    token_file: PathBuf,
    spiffe_id: String,
    ttl: Duration,
}

impl Issuer {
    pub(crate) fn new(address: &Url, config: &OpenBaoPkiConfig) -> Result<Self, BuildError> {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let mut builder = reqwest::Client::builder()
            .timeout(config.timeout)
            .connect_timeout(config.timeout)
            // The service-account token and the client token go to the configured address only.
            .redirect(reqwest::redirect::Policy::none());
        if let Some(pem) = &config.ca_pem {
            let roots = reqwest::Certificate::from_pem_bundle(pem.as_bytes())
                .map_err(|_| BuildError::BadCa)?;
            if roots.is_empty() {
                return Err(BuildError::BadCa);
            }
            for root in roots {
                builder = builder.add_root_certificate(root);
            }
        }
        let http = builder.build().map_err(|_| BuildError::Http)?;
        let base = address.as_str().trim_end_matches('/');
        let url = |path: String| {
            Url::parse(&format!("{base}/v1/{path}")).map_err(|_| BuildError::BadAddress)
        };
        Ok(Self {
            http,
            login_url: url(format!(
                "auth/{}/login",
                config.kubernetes_mount.trim_matches('/')
            ))?,
            sign_url: url(format!(
                "{}/sign/{}",
                config.pki_mount.trim_matches('/'),
                config.pki_role
            ))?,
            auth_role: config.auth_role.clone(),
            token_file: config.service_account_token_file.clone(),
            spiffe_id: config.spiffe_id.clone(),
            ttl: config.ttl,
        })
    }

    /// One issue: login, a fresh key, signing, and the check of what came back.
    pub(crate) async fn issue(
        &self,
        clock: &dyn Clock,
        margin: SignedDuration,
    ) -> Result<PlatformClientCertificate, IssueError> {
        let jwt = self.service_account_token().await?;
        let client_token = self.login(&jwt).await?;
        drop(jwt);
        let key = generate()?;
        let signed = self.sign(&client_token, &key.csr_pem).await?;
        drop(client_token);
        let extra: Vec<&str> = if signed.ca_chain.is_empty() {
            signed.issuing_ca.iter().map(String::as_str).collect()
        } else {
            signed.ca_chain.iter().map(String::as_str).collect()
        };
        let verified = leaf::verify(
            &signed.certificate,
            &extra,
            &leaf::Expected {
                public_point: &key.public_point,
                spiffe_id: &self.spiffe_id,
                now: clock.now(),
                margin,
            },
        )?;
        Ok(PlatformClientCertificate::new(
            verified.chain,
            key.pkcs8,
            verified.serial,
            verified.not_after,
        ))
    }

    /// The pod's token, read now: the kubelet rotates the file, so no read is kept.
    async fn service_account_token(&self) -> Result<Zeroizing<String>, IssueError> {
        let fail = || IssueError::Unavailable(Stage::ServiceAccountToken);
        let bytes = Zeroizing::new(
            tokio::fs::read(&self.token_file)
                .await
                .map_err(|_| fail())?,
        );
        if bytes.len() > MAX_TOKEN_FILE {
            return Err(fail());
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| fail())?.trim();
        if text.is_empty() {
            return Err(fail());
        }
        Ok(Zeroizing::new(text.to_owned()))
    }

    async fn login(&self, jwt: &Zeroizing<String>) -> Result<Zeroizing<String>, IssueError> {
        let fail = || IssueError::Unavailable(Stage::Login);
        let body = Zeroizing::new(
            serde_json::to_vec(&json!({ "role": self.auth_role, "jwt": jwt.as_str() }))
                .map_err(|_| fail())?,
        );
        let response = self
            .http
            .post(self.login_url.clone())
            .header(CONTENT_TYPE, "application/json")
            .body(body.to_vec())
            .send()
            .await
            .map_err(|_| fail())?;
        let bytes = Zeroizing::new(read_answer(response, Stage::Login).await?);
        let reply: LoginReply = serde_json::from_slice(&bytes).map_err(|_| fail())?;
        if reply.auth.client_token.is_empty() {
            return Err(fail());
        }
        Ok(reply.auth.client_token)
    }

    async fn sign(
        &self,
        client_token: &Zeroizing<String>,
        csr_pem: &str,
    ) -> Result<SignData, IssueError> {
        let fail = || IssueError::Unavailable(Stage::Sign);
        let mut token = HeaderValue::from_str(client_token.as_str()).map_err(|_| fail())?;
        token.set_sensitive(true);
        let response = self
            .http
            .post(self.sign_url.clone())
            .header("X-Vault-Token", token)
            .json(&json!({
                "csr": csr_pem,
                "uri_sans": self.spiffe_id,
                "ttl": format!("{}s", self.ttl.as_secs()),
                "format": "pem",
                "remove_roots_from_chain": true,
            }))
            .send()
            .await
            .map_err(|_| fail())?;
        let bytes = read_answer(response, Stage::Sign).await?;
        let reply: SignReply = serde_json::from_slice(&bytes).map_err(|_| fail())?;
        Ok(reply.data)
    }
}

/// The body of a successful answer, bounded; a refusal (400, 401, 403) is `Refused` and any other
/// status that is not a success is `Unavailable`.
async fn read_answer(mut response: reqwest::Response, stage: Stage) -> Result<Vec<u8>, IssueError> {
    let status = response.status();
    if matches!(
        status,
        StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ) {
        return Err(IssueError::Refused(stage));
    }
    if !status.is_success() {
        return Err(IssueError::Unavailable(stage));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| IssueError::Unavailable(stage))?
    {
        if body.len() + chunk.len() > MAX_BODY {
            return Err(IssueError::Unavailable(stage));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
