//! Signing one request: the claim object of `docs/api/signed-tool-requests-v1.md`.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cratestack_cose::{CwtClaims, sign_claims};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::key::{SigningKey, hex};

/// The name of the header that carries the object.
pub const HEADER: &str = "Tool-Request-Claims";

/// The `typ` of the object (RFC 9596), the domain separator. An incompatible change of the contract is a new `typ`.
pub const TYP: &str = "application/vnd.vymalo.tool-request.v1+cwt";

/// The longest lifetime of an object, `exp - iat`, in seconds.
pub const MAX_TTL_SECS: u64 = 60;

/// The lifetime when the configuration says none, in seconds.
pub const DEFAULT_TTL_SECS: u64 = 30;

/// The longest header value, in characters. A receiver refuses a longer one before it decodes it.
pub const MAX_HEADER_BYTES: usize = 4096;

/// The longest audience, in bytes (`cratestack-cose`'s `MAX_AUD_LEN`).
pub const MAX_AUDIENCE_BYTES: usize = 256;

/// The longest request path, in bytes.
pub const MAX_PATH_BYTES: usize = 512;

/// The longest owner, in bytes.
pub const MAX_OWNER_BYTES: usize = 320;

/// The length of a `cti`, in bytes.
pub const CTI_LEN: usize = 16;

/// The HTTP methods the orchestrator's MCP client sends: a closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Method {
    /// A JSON-RPC message.
    Post,
    /// The event stream of a session of an older MCP revision.
    Get,
    /// The end of such a session.
    Delete,
}

impl Method {
    /// The method as the HTTP request line spells it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Method::Post => "POST",
            Method::Get => "GET",
            Method::Delete => "DELETE",
        }
    }
}

/// One HTTP request to a tool server, as far as the signature binds it.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// The method.
    pub method: Method,
    /// The path of the request target, as sent: starts with `/`, percent-encoded, no query, ASCII.
    pub path: &'a str,
    /// The exact bytes of the body that will be sent (empty for a `GET` and a `DELETE`).
    pub body: &'a [u8],
    /// The thread the call is made for.
    pub thread: Uuid,
    /// The owner of that thread: the person the tool runs for.
    pub owner: &'a str,
}

/// Why a request could not be signed. Nothing is sent when one is returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SignError {
    /// The audience is empty, longer than [`MAX_AUDIENCE_BYTES`] or has a control character.
    #[error("the audience must be 1 to 256 bytes of text without control characters")]
    Audience,
    /// The lifetime is not 1 to [`MAX_TTL_SECS`] seconds.
    #[error("the lifetime must be 1 to 60 seconds")]
    Lifetime,
    /// The path does not start with `/`, is not ASCII, or is longer than [`MAX_PATH_BYTES`].
    #[error("the request path must be ASCII, start with / and be at most 512 bytes")]
    Path,
    /// The owner is empty, longer than [`MAX_OWNER_BYTES`] or has a control character.
    #[error("the owner must be 1 to 320 bytes of text without control characters")]
    Owner,
    /// The operating system's random source failed.
    #[error("the operating system's random source failed")]
    Random,
    /// `cratestack_cose::sign_claims` did not sign the object.
    #[error("the claim object could not be signed")]
    Cose,
    /// The object would be longer than a header may be.
    #[error("the claim object is longer than a header may be")]
    TooLarge,
}

/// Signs the requests to one tool server: one key, one audience, one lifetime.
pub struct Signer {
    key: SigningKey,
    audience: String,
    ttl_secs: u64,
}

impl fmt::Debug for Signer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Signer")
            .field("key", &self.key)
            .field("audience", &self.audience)
            .field("ttl_secs", &self.ttl_secs)
            .finish()
    }
}

fn plain_text(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && !text.chars().any(char::is_control)
}

impl Signer {
    /// A signer with `key`, for the tool server whose own name for itself is `audience`, whose objects live `ttl_secs`.
    ///
    /// # Errors
    /// [`SignError::Audience`] or [`SignError::Lifetime`].
    pub fn new(
        key: SigningKey,
        audience: impl Into<String>,
        ttl_secs: u64,
    ) -> Result<Self, SignError> {
        let audience = audience.into();
        if !plain_text(&audience, MAX_AUDIENCE_BYTES) {
            return Err(SignError::Audience);
        }
        if !(1..=MAX_TTL_SECS).contains(&ttl_secs) {
            return Err(SignError::Lifetime);
        }
        Ok(Signer {
            key,
            audience,
            ttl_secs,
        })
    }

    /// The `kid` of the key.
    pub fn kid(&self) -> [u8; crate::KID_LEN] {
        self.key.kid()
    }

    /// Whether `request` is one this signer can sign, without signing it: the limits of the contract.
    ///
    /// # Errors
    /// [`SignError::Path`] or [`SignError::Owner`].
    pub fn check(request: &Request<'_>) -> Result<(), SignError> {
        let path = request.path;
        if !path.starts_with('/')
            || path.len() > MAX_PATH_BYTES
            || !path.is_ascii()
            || path.chars().any(char::is_control)
        {
            return Err(SignError::Path);
        }
        if !plain_text(request.owner, MAX_OWNER_BYTES) {
            return Err(SignError::Owner);
        }
        Ok(())
    }

    /// The value of the header for `request`, at the time `iat` (whole seconds since the Unix epoch), with a fresh random
    /// `cti`.
    ///
    /// # Errors
    /// [`SignError`]; nothing was signed.
    pub async fn sign(&self, request: &Request<'_>, iat: u64) -> Result<String, SignError> {
        let mut cti = [0_u8; CTI_LEN];
        getrandom::fill(&mut cti).map_err(|_| SignError::Random)?;
        self.sign_with_cti(request, iat, cti).await
    }

    /// [`sign`](Self::sign) with the `cti` given: for the known-answer test and nothing else. A `cti` used twice is a replay.
    ///
    /// # Errors
    /// [`SignError`]; nothing was signed.
    pub async fn sign_with_cti(
        &self,
        request: &Request<'_>,
        iat: u64,
        cti: [u8; CTI_LEN],
    ) -> Result<String, SignError> {
        let exp = iat.checked_add(self.ttl_secs).ok_or(SignError::Lifetime)?;
        self.sign_object(request, iat, exp, cti).await
    }

    /// An object with any `exp`, including one a signer refuses to make (longer than [`MAX_TTL_SECS`], or before `iat`): for the
    /// tests of the receiving side, which must refuse it. Only with the feature `verify`.
    #[cfg(feature = "verify")]
    #[doc(hidden)]
    pub async fn sign_for_tests(
        &self,
        request: &Request<'_>,
        iat: u64,
        exp: u64,
        cti: [u8; CTI_LEN],
    ) -> Result<String, SignError> {
        self.sign_object(request, iat, exp, cti).await
    }

    /// The object for any `exp`: [`sign_with_cti`](Self::sign_with_cti) with the lifetime of the signer.
    async fn sign_object(
        &self,
        request: &Request<'_>,
        iat: u64,
        exp: u64,
        cti: [u8; CTI_LEN],
    ) -> Result<String, SignError> {
        Self::check(request)?;
        let payload = serde_json::to_vec(&Binding {
            method: request.method.as_str(),
            path: request.path,
            body_sha256: hex(&Sha256::digest(request.body)),
            thread: request.thread.hyphenated().to_string(),
            owner: request.owner,
        })
        .map_err(|_| SignError::TooLarge)?;
        // `sign_claims` writes the claims in the order of the contract and leaves out one that is `None`: all four are
        // always there.
        let claims = CwtClaims {
            aud: Some(self.audience.clone()),
            exp: Some(exp),
            iat: Some(iat),
            cti: Some(cti.to_vec()),
        };
        // `external_aad` is empty, as the contract says.
        let object = sign_claims(self.key.cose(), TYP, &claims, &payload, b"")
            .await
            .map_err(|_| SignError::Cose)?;
        let value = URL_SAFE_NO_PAD.encode(&object);
        if value.len() > MAX_HEADER_BYTES {
            return Err(SignError::TooLarge);
        }
        Ok(value)
    }
}

/// The payload. The order of the members is the contract's, and serde writes them in the order they are declared.
#[derive(Serialize)]
struct Binding<'a> {
    method: &'a str,
    path: &'a str,
    body_sha256: String,
    thread: String,
    owner: &'a str,
}
