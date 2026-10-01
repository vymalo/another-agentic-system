//! The token of the thread-tools endpoint (`thread-tools/v1`, `docs/api/thread-tools-v1.md`).
//!
//! The orchestrator gives an agent that lists the extension one MCP endpoint for the thread it is
//! working on. What opens it is a **token**: a JWS in compact form, signed HS256 (HMAC-SHA-256)
//! with a key from the deployment's configuration, so that any replica can verify what any
//! replica minted (the processes stay stateless, ADR 0001). It is minted by the A2A adapter at
//! the moment it sends a message, from a non-secret [`ToolsGrant`], and is never written to the
//! event log, to the outbox or to a log line: the types that hold it print `[redacted]`.
//!
//! # What is here
//!
//! * [`ThreadToolsKeys`]: the current key (signs and verifies) and the previous one (verifies
//!   only, for a rotation); at least [`MIN_KEY_BYTES`] bytes each.
//! * [`Claims`], [`mint`] and [`verify`]: the token's contents, and the two functions over them.
//!   [`verify`] does the checks of the contract that need nothing but the key and the time (the
//!   size, the header, the key, the signature, the claims, the issuer and audience, the
//!   lifetime); that the token's thread is the path's, and that the thread exists and has this
//!   agent, are for the endpoint, which reads the store.
//! * [`ThreadToolsIssuer`]: the keys, the base URL agents reach the orchestrator at, and the
//!   lifetime; [`ThreadToolsIssuer::grant`] gives the `{url, token, expiresAt}` an agent
//!   receives ([`ThreadToolsGrant`]).
//!
//! # Pure
//!
//! No I/O, no async, no clock: every function that needs "now" takes it as an argument, so the
//! tests pin the exact bytes of a token (the known-answer vectors in `tests/vectors.rs` and in
//! `docs/api/thread-tools-v1.md`; they are computed by an independent implementation).
//!
//! # The token
//!
//! ```text
//! header   {"alg":"HS256","typ":"JWT","kid":"<first 16 hex of SHA-256(key)>"}
//! claims   {"iss":"orch","aud":"thread-tools","sub":"<thread>","job":<n>,"agt":"<agent>",
//!           "caller":"main"|"ask:<n>","depth":<0..255>,"jti":"<A2A message id>",
//!           "iat":<unix s>,"exp":<unix s>}
//! token    base64url(header) "." base64url(claims) "." base64url(HMAC-SHA-256(key, first two))
//! ```
//!
//! Base64url is the strict, unpadded alphabet: a token that is padded, uses another alphabet or
//! has non-canonical trailing bits is refused, so no two spellings stand for one token. The key
//! is the bytes of the string as written in the configuration (`openssl rand -hex 32` gives 64
//! characters).

use std::fmt;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use jiff::Timestamp;
use orch_core::{AgentId, Caller, ThreadId, ToolsGrant};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// The token's issuer (`iss`).
pub const ISSUER: &str = "orch";
/// The token's audience (`aud`).
pub const AUDIENCE: &str = "thread-tools";
/// The longest token [`verify`] reads, in bytes. A real token is about 400 bytes.
pub const MAX_TOKEN_BYTES: usize = 2048;
/// The shortest key, in bytes: HS256 wants a key as large as its hash output (RFC 7518 3.2).
pub const MIN_KEY_BYTES: usize = 32;
/// How many seconds of clock difference between the minting and the verifying replica are
/// forgiven, at both ends of the token's life.
pub const SKEW_SECS: i64 = 30;
/// The default lifetime of a token, in seconds: a task can run long, and an ask can last half an
/// hour inside it.
pub const DEFAULT_TTL_SECS: u64 = 7200;
/// The shortest lifetime an issuer accepts, in seconds.
pub const MIN_TTL_SECS: u64 = 60;
/// The longest lifetime an issuer accepts, in seconds: a day.
pub const MAX_TTL_SECS: u64 = 86_400;
/// The longest `jti` and `agt` [`verify`] accepts, in bytes.
const MAX_CLAIM_TEXT_BYTES: usize = 256;

// ---- keys --------------------------------------------------------------------------------

/// Why a set of keys cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum KeyError {
    /// A key shorter than [`MIN_KEY_BYTES`].
    #[error("the {which} key is shorter than {MIN_KEY_BYTES} bytes")]
    TooShort {
        /// `current` or `previous`.
        which: &'static str,
    },
    /// The previous key is the current one: a token's `kid` could not say which to try, and the
    /// rotation is a mistake.
    #[error("the previous key is the same as the current one")]
    Same,
}

/// One key and the `kid` that names it.
#[derive(Clone)]
struct Key {
    secret: SecretString,
    kid: String,
}

impl Key {
    fn new(secret: SecretString, which: &'static str) -> Result<Self, KeyError> {
        if secret.expose_secret().len() < MIN_KEY_BYTES {
            return Err(KeyError::TooShort { which });
        }
        let kid = kid_of(secret.expose_secret().as_bytes());
        Ok(Key { secret, kid })
    }

    fn mac(&self) -> Result<HmacSha256, MintError> {
        // HMAC accepts a key of any length; this cannot fail, and is not unwrapped.
        <HmacSha256 as Mac>::new_from_slice(self.secret.expose_secret().as_bytes())
            .map_err(|_| MintError::Key)
    }
}

/// The first 16 hexadecimal characters of the SHA-256 of the key: what a token's `kid` says.
fn kid_of(key: &[u8]) -> String {
    use fmt::Write as _;
    let digest = Sha256::digest(key);
    let mut kid = String::with_capacity(16);
    for byte in &digest[..8] {
        let _ = write!(kid, "{byte:02x}");
    }
    kid
}

/// The keys the tokens are signed and verified with: the current key signs and verifies, the
/// previous one (set while a rotation is in progress) only verifies.
///
/// Rotate by moving the current key to the previous slot and setting a new current one; remove
/// the previous key once the longest lifetime in use has passed. `Debug` shows the `kid`s, never
/// a key.
#[derive(Clone)]
pub struct ThreadToolsKeys {
    current: Key,
    previous: Option<Key>,
}

impl fmt::Debug for ThreadToolsKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadToolsKeys")
            .field("current", &self.current.kid)
            .field("previous", &self.previous.as_ref().map(|k| &k.kid))
            .finish()
    }
}

impl ThreadToolsKeys {
    /// The keys, each at least [`MIN_KEY_BYTES`] bytes, the previous one not the current one.
    ///
    /// # Errors
    ///
    /// A [`KeyError`] naming what is wrong.
    pub fn new(current: SecretString, previous: Option<SecretString>) -> Result<Self, KeyError> {
        let current = Key::new(current, "current")?;
        let previous = previous.map(|k| Key::new(k, "previous")).transpose()?;
        if previous.as_ref().is_some_and(|p| p.kid == current.kid) {
            return Err(KeyError::Same);
        }
        Ok(ThreadToolsKeys { current, previous })
    }

    /// The `kid` of the current key.
    pub fn current_kid(&self) -> &str {
        &self.current.kid
    }

    /// The `kid` of the previous key, when there is one.
    pub fn previous_kid(&self) -> Option<&str> {
        self.previous.as_ref().map(|k| k.kid.as_str())
    }

    fn by_kid(&self, kid: &str) -> Option<&Key> {
        if kid == self.current.kid {
            Some(&self.current)
        } else {
            self.previous.as_ref().filter(|k| k.kid == kid)
        }
    }
}

// ---- claims -------------------------------------------------------------------------------

/// What a token says: who may call which thread's tools, as whom, until when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    /// `sub`: the one thread the token opens.
    pub thread: ThreadId,
    /// `job`: the number of the thread's job the message belongs to, from 1.
    pub job: u32,
    /// `agt`: the agent the token was minted for.
    pub agent: AgentId,
    /// `caller`: the addressed agent (`main`) or an agent it asked (`ask:<n>`).
    pub caller: Caller,
    /// `depth`: 0 for `main`; for an asked agent, its depth in the chain of asks.
    pub depth: u8,
    /// `jti`: the A2A message id the token was minted for; one token per message.
    pub message_id: String,
    /// `iat`: when it was minted, whole seconds.
    pub issued_at: Timestamp,
    /// `exp`: when it expires, whole seconds.
    pub expires_at: Timestamp,
}

/// The claims as they are written: the names of the contract, in the order of the contract (the
/// order is part of the bytes a known-answer vector pins).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    iss: String,
    aud: String,
    sub: ThreadId,
    job: u32,
    agt: String,
    caller: Caller,
    depth: u8,
    jti: String,
    iat: i64,
    exp: i64,
}

/// The header, exactly: any other member is refused.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    alg: String,
    typ: String,
    kid: String,
}

impl Claims {
    fn to_wire(&self) -> Wire {
        Wire {
            iss: ISSUER.to_owned(),
            aud: AUDIENCE.to_owned(),
            sub: self.thread,
            job: self.job,
            agt: self.agent.as_str().to_owned(),
            caller: self.caller,
            depth: self.depth,
            jti: self.message_id.clone(),
            iat: self.issued_at.as_second(),
            exp: self.expires_at.as_second(),
        }
    }

    /// Whether the fields agree (what [`verify`] requires of every token and [`mint`] of every
    /// token it writes): a job from 1, a depth of 0 exactly for `main` (and at least 1 for an
    /// ask, which is `ask:<n>` with n from 1), a non-empty agent and message id of at most 256
    /// bytes each, whole-second times, and an expiry that is not before the issue.
    pub fn is_well_formed(&self) -> bool {
        let text_ok = |s: &str| !s.is_empty() && s.len() <= MAX_CLAIM_TEXT_BYTES;
        let depth_ok = match self.caller {
            Caller::Main => self.depth == 0,
            Caller::Ask(n) => n >= 1 && self.depth >= 1,
        };
        self.job >= 1
            && depth_ok
            && text_ok(self.agent.as_str())
            && text_ok(&self.message_id)
            && self.issued_at.subsec_nanosecond() == 0
            && self.expires_at.subsec_nanosecond() == 0
            && self.expires_at >= self.issued_at
    }
}

// ---- mint ---------------------------------------------------------------------------------

/// Why a token could not be written. Both are bugs of the caller, not conditions of a request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MintError {
    /// The claims do not agree ([`Claims::is_well_formed`]).
    #[error("the claims of the token are not consistent")]
    Claims,
    /// The key could not start a MAC (HMAC takes any length, so this does not happen).
    #[error("the key cannot sign")]
    Key,
    /// The claims could not be written as JSON (they are strings and integers, so this does not
    /// happen).
    #[error("the claims cannot be written")]
    Encode,
}

fn encode_json(value: &impl Serialize) -> Result<String, MintError> {
    serde_json::to_vec(value)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| MintError::Encode)
}

/// Writes the token for `claims`, signed with the current key. It never fails for claims that
/// are well formed ([`Claims::is_well_formed`]).
///
/// # Errors
///
/// [`MintError::Claims`] when the claims do not agree.
pub fn mint(keys: &ThreadToolsKeys, claims: &Claims) -> Result<SecretString, MintError> {
    if !claims.is_well_formed() {
        return Err(MintError::Claims);
    }
    let header = encode_json(&Header {
        alg: "HS256".to_owned(),
        typ: "JWT".to_owned(),
        kid: keys.current.kid.clone(),
    })?;
    let payload = encode_json(&claims.to_wire())?;
    let signing_input = format!("{header}.{payload}");
    let mut mac = keys.current.mac()?;
    mac.update(signing_input.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(SecretString::from(format!("{signing_input}.{signature}")))
}

// ---- verify -------------------------------------------------------------------------------

/// Why a token was refused. The endpoint answers every one of them with the same `401`, and says
/// nothing of which; the variants are for the operator's log and the tests, and never display a
/// byte of the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TokenError {
    /// More than [`MAX_TOKEN_BYTES`].
    #[error("the token is too long")]
    TooLong,
    /// Not three segments of strict base64url.
    #[error("the token is not a compact JWS")]
    Malformed,
    /// A header that is not exactly `{alg: HS256, typ: JWT, kid}` (`none` and every other
    /// algorithm included).
    #[error("the token header is not the one of this endpoint")]
    Header,
    /// A `kid` that is neither the current nor the previous key.
    #[error("the token was signed with an unknown key")]
    UnknownKey,
    /// The signature does not match.
    #[error("the token signature does not match")]
    Signature,
    /// The claims are not the claims of this endpoint, or do not agree.
    #[error("the token claims are not valid")]
    Claims,
    /// `iss` is not [`ISSUER`].
    #[error("the token issuer is not this orchestrator")]
    Issuer,
    /// `aud` is not [`AUDIENCE`].
    #[error("the token audience is not the thread tools")]
    Audience,
    /// `exp` is more than [`SKEW_SECS`] seconds in the past.
    #[error("the token has expired")]
    Expired,
    /// `iat` is more than [`SKEW_SECS`] seconds in the future.
    #[error("the token was issued in the future")]
    NotYetValid,
}

/// The length of the signing input: `token` without its last segment and the dot before it.
fn signing_input_len(token: &str, signature: &str) -> usize {
    token.len() - signature.len() - 1
}

fn decode(segment: &str) -> Result<Vec<u8>, TokenError> {
    URL_SAFE_NO_PAD
        .decode(segment)
        .map_err(|_| TokenError::Malformed)
}

/// Checks a token at `now`, in the order of the contract, and returns what it says.
///
/// 1. at most [`MAX_TOKEN_BYTES`], three segments of strict base64url;
/// 2. the header is exactly HS256 and JWT, and `kid` names the current or the previous key;
/// 3. the signature matches, compared in constant time;
/// 4. the claims parse, every one present and well formed ([`Claims::is_well_formed`]);
/// 5. `iss` and `aud` are the expected ones;
/// 6. `exp` is later than `now` minus [`SKEW_SECS`] seconds, and `iat` is not later than `now`
///    plus [`SKEW_SECS`].
///
/// # Errors
///
/// The [`TokenError`] of the first check that fails.
pub fn verify(keys: &ThreadToolsKeys, token: &str, now: Timestamp) -> Result<Claims, TokenError> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(TokenError::TooLong);
    }
    let mut segments = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(TokenError::Malformed);
    };
    let (header_bytes, payload_bytes, signature_bytes) =
        (decode(header)?, decode(payload)?, decode(signature)?);

    let header: Header = serde_json::from_slice(&header_bytes).map_err(|_| TokenError::Header)?;
    if header.alg != "HS256" || header.typ != "JWT" {
        return Err(TokenError::Header);
    }
    let key = keys.by_kid(&header.kid).ok_or(TokenError::UnknownKey)?;

    // The signature covers the text of the first two segments, as written, and the dot.
    let signing_input = &token[..signing_input_len(token, signature)];
    let mut mac = key.mac().map_err(|_| TokenError::UnknownKey)?;
    mac.update(signing_input.as_bytes());
    mac.verify_slice(&signature_bytes)
        .map_err(|_| TokenError::Signature)?;

    let wire: Wire = serde_json::from_slice(&payload_bytes).map_err(|_| TokenError::Claims)?;
    let claims = Claims {
        thread: wire.sub,
        job: wire.job,
        agent: AgentId::new(wire.agt),
        caller: wire.caller,
        depth: wire.depth,
        message_id: wire.jti,
        issued_at: Timestamp::from_second(wire.iat).map_err(|_| TokenError::Claims)?,
        expires_at: Timestamp::from_second(wire.exp).map_err(|_| TokenError::Claims)?,
    };
    if !claims.is_well_formed() {
        return Err(TokenError::Claims);
    }
    if wire.iss != ISSUER {
        return Err(TokenError::Issuer);
    }
    if wire.aud != AUDIENCE {
        return Err(TokenError::Audience);
    }
    let now = now.as_second();
    if wire.exp.saturating_add(SKEW_SECS) <= now {
        return Err(TokenError::Expired);
    }
    if wire.iat > now.saturating_add(SKEW_SECS) {
        return Err(TokenError::NotYetValid);
    }
    Ok(claims)
}

// ---- the issuer ---------------------------------------------------------------------------

/// Why an issuer cannot be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum IssuerError {
    /// The base URL is not an `http` or `https` URL with a host and nothing else of an
    /// authority (no credentials), query or fragment.
    #[error(
        "the base URL {0:?} is not an http(s) URL with a host, without credentials, query or fragment"
    )]
    BadUrl(String),
    /// The lifetime is outside [`MIN_TTL_SECS`] to [`MAX_TTL_SECS`] or not whole seconds.
    #[error("the token lifetime must be {MIN_TTL_SECS} to {MAX_TTL_SECS} whole seconds")]
    BadTtl,
}

/// What an agent receives under the extension's metadata key: where the endpoint is, the token
/// that opens it, and until when. `Debug` shows `[redacted]` for the token.
#[derive(Clone)]
pub struct ThreadToolsGrant {
    /// The thread's endpoint: the base URL, `/thread-tools/<threadId>/mcp`.
    pub url: String,
    /// The token. Not for the log, the outbox or the event log.
    pub token: SecretString,
    /// When the token expires (its `exp`).
    pub expires_at: Timestamp,
}

impl fmt::Debug for ThreadToolsGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadToolsGrant")
            .field("url", &self.url)
            .field("token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Mints the grants of the A2A adapter: the keys, the base URL agents reach the orchestrator at,
/// and the lifetime of a token.
#[derive(Clone)]
pub struct ThreadToolsIssuer {
    keys: ThreadToolsKeys,
    base_url: String,
    host: String,
    ttl: Duration,
}

impl fmt::Debug for ThreadToolsIssuer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ThreadToolsIssuer")
            .field("keys", &self.keys)
            .field("base_url", &self.base_url)
            .field("ttl", &self.ttl)
            .finish()
    }
}

impl ThreadToolsIssuer {
    /// An issuer. `base_url` is the address agents reach the orchestrator at (`http` or `https`,
    /// a host, optionally a port and a path prefix); `ttl` is a whole number of seconds from
    /// [`MIN_TTL_SECS`] to [`MAX_TTL_SECS`].
    ///
    /// # Errors
    ///
    /// An [`IssuerError`] for an unusable URL or lifetime.
    pub fn new(keys: ThreadToolsKeys, base_url: &str, ttl: Duration) -> Result<Self, IssuerError> {
        let bad_url = || IssuerError::BadUrl(base_url.to_owned());
        let parsed = url::Url::parse(base_url.trim()).map_err(|_| bad_url())?;
        let ok = matches!(parsed.scheme(), "http" | "https")
            && parsed.has_host()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.query().is_none()
            && parsed.fragment().is_none();
        if !ok {
            return Err(bad_url());
        }
        let host = match (parsed.host_str(), parsed.port()) {
            (Some(host), Some(port)) => format!("{host}:{port}"),
            (Some(host), None) => host.to_owned(),
            (None, _) => return Err(bad_url()),
        };
        let secs = ttl.as_secs();
        if ttl.subsec_nanos() != 0 || !(MIN_TTL_SECS..=MAX_TTL_SECS).contains(&secs) {
            return Err(IssuerError::BadTtl);
        }
        Ok(ThreadToolsIssuer {
            keys,
            base_url: parsed.as_str().trim_end_matches('/').to_owned(),
            host,
            ttl,
        })
    }

    /// The base URL, as agents are told it, without a trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// The host of the base URL as a `Host` header carries it: the name, and the port when the URL
    /// names one. The endpoint accepts it by default.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The keys.
    pub fn keys(&self) -> &ThreadToolsKeys {
        &self.keys
    }

    /// The lifetime of a token.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// The endpoint of `thread`.
    pub fn url_for(&self, thread: ThreadId) -> String {
        format!("{}/thread-tools/{thread}/mcp", self.base_url)
    }

    /// The grant for the message `message_id` of `grant`'s thread, minted at `now` (whole
    /// seconds): the endpoint's URL, the token, and when it expires.
    ///
    /// # Errors
    ///
    /// [`MintError::Claims`] for a grant whose fields do not agree ([`ToolsGrant::is_consistent`])
    /// or an empty message id.
    pub fn grant(
        &self,
        grant: &ToolsGrant,
        message_id: &str,
        now: Timestamp,
    ) -> Result<ThreadToolsGrant, MintError> {
        if !grant.is_consistent() {
            return Err(MintError::Claims);
        }
        let ttl = i64::try_from(self.ttl.as_secs()).map_err(|_| MintError::Claims)?;
        let issued_at = Timestamp::from_second(now.as_second()).map_err(|_| MintError::Claims)?;
        let expires_at = issued_at
            .checked_add(jiff::SignedDuration::from_secs(ttl))
            .map_err(|_| MintError::Claims)?;
        let claims = Claims {
            thread: grant.thread,
            job: grant.job,
            agent: grant.agent.clone(),
            caller: grant.caller,
            depth: grant.depth,
            message_id: message_id.to_owned(),
            issued_at,
            expires_at,
        };
        let token = mint(&self.keys, &claims)?;
        Ok(ThreadToolsGrant {
            url: self.url_for(grant.thread),
            token,
            expires_at,
        })
    }
}
