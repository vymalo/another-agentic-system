//! DPoP, proof of possession of a token's key (RFC 9449, ADR 0054), checked by the resource server.
//!
//! A DPoP request carries `Authorization: DPoP <access token>` and one `DPoP` header, a JWT the
//! client signed with its own key. This module checks the proof; [`crate::JwtAuth`] checks the
//! token as it does for a bearer, and binds the two (`cnf.jkt` is the thumbprint of the proof's key).
//!
//! # What is checked, in this order
//!
//! 1. **One proof**: exactly one `DPoP` header, holding one JWT of at most 8 KiB.
//! 2. **Header**: `typ` is `dpop+jwt`; `alg` is `ES256` or `EdDSA` (nothing else: not `none`, not the
//!    HMAC family, not RSA); `jwk` is the client's **public** key, of the algorithm's kind (P-256
//!    for `ES256`, Ed25519 for `EdDSA`), with its coordinates in their full length, and without a
//!    private member (`d`, `p`, `q`, `dp`, `dq`, `qi`, `oth`, `k`): a key that carries one is refused.
//! 3. **Signature** by that `jwk`.
//! 4. **Claims**: `htm` is the request's method; `htu`, without its query and fragment, is one of
//!    the configured public origins joined with the request's path (this process sits behind a
//!    proxy and cannot rebuild the public URL itself); `iat` is within [`DEFAULT_MAX_AGE`] past
//!    and [`DEFAULT_FUTURE_SKEW`] ahead; `ath` is the base64url of the SHA-256 of the access token;
//!    `jti` is present.
//! 5. **After the token is valid and bound** to the proof's key: the proof's `jti` has not been
//!    seen. Recording it last means a caller who holds no valid token cannot fill the cache.
//!
//! # Replay
//!
//! The `jti`s seen are kept **in memory** of this process, for as long as a proof stays valid (the
//! age and the skew), at most [`DEFAULT_REPLAY_CAPACITY`] of them: past the cap every DPoP request
//! is refused as unavailable until the oldest expire (a replay is never let in for want of room).
//! A deployment of several processes accepts a proof once per process within that window; the
//! chart runs one (ADR 0054, decision 6).

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::{Jwk, ThumbprintHash};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use orch_ports::{AuthError, DpopCredentials};
use serde::Deserialize;
use serde_json::{Map, Value};
use url::Url;

use crate::BuildError;

/// How old a proof's `iat` may be (60 seconds).
pub const DEFAULT_MAX_AGE: Duration = Duration::from_secs(60);

/// How far ahead of this process's clock a proof's `iat` may be (5 seconds).
pub const DEFAULT_FUTURE_SKEW: Duration = Duration::from_secs(5);

/// How many `jti`s are remembered at most (100 000).
pub const DEFAULT_REPLAY_CAPACITY: usize = 100_000;

/// The longest proof read (8 KiB). A proof is a header with one public key and six short claims.
pub const MAX_PROOF_BYTES: usize = 8 * 1024;

/// The longest `jti` read.
const MAX_JTI_BYTES: usize = 256;

/// The members of a JWK that only a private or symmetric key has (RFC 7518 §6).
const PRIVATE_MEMBERS: [&str; 8] = ["d", "p", "q", "dp", "dq", "qi", "oth", "k"];

/// The configuration of DPoP (`auth.dpop`).
#[derive(Debug, Clone)]
pub struct DpopConfig {
    /// The origins the clients call this API at, `https://chat.example.com`: a proof's `htu` must be
    /// one of them and the request's path. Each is an `http(s)` origin without a path.
    pub public_origins: Vec<String>,
    /// How old a proof's `iat` may be.
    pub max_age: Duration,
    /// How far ahead a proof's `iat` may be.
    pub future_skew: Duration,
    /// How many `jti`s are remembered at most.
    pub replay_capacity: usize,
}

impl DpopConfig {
    /// DPoP for requests made to `public_origins`: 60 seconds of age, 5 of skew.
    pub fn new(public_origins: impl IntoIterator<Item = impl Into<String>>) -> Self {
        DpopConfig {
            public_origins: public_origins.into_iter().map(Into::into).collect(),
            max_age: DEFAULT_MAX_AGE,
            future_skew: DEFAULT_FUTURE_SKEW,
            replay_capacity: DEFAULT_REPLAY_CAPACITY,
        }
    }

    /// Sets how old a proof may be and how far ahead.
    #[must_use]
    pub fn with_window(mut self, max_age: Duration, future_skew: Duration) -> Self {
        self.max_age = max_age;
        self.future_skew = future_skew;
        self
    }
}

/// The `jti`s seen, oldest first. Every entry is kept for the same time, so the order of
/// insertion is the order of expiry and eviction only looks at the front.
#[derive(Default)]
struct Seen {
    order: VecDeque<(Instant, String)>,
    set: HashSet<String>,
}

/// What recording a `jti` came to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Admit {
    /// New: now remembered.
    Fresh,
    /// Seen within the window.
    Replayed,
    /// Not seen, but there is no room to remember it.
    Full,
}

impl Seen {
    fn admit(&mut self, key: String, now: Instant, keep: Duration, capacity: usize) -> Admit {
        while self
            .order
            .front()
            .is_some_and(|(expires, _)| *expires <= now)
        {
            if let Some((_, old)) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        if self.set.contains(&key) {
            return Admit::Replayed;
        }
        if self.order.len() >= capacity {
            return Admit::Full;
        }
        self.set.insert(key.clone());
        self.order.push_back((now + keep, key));
        Admit::Fresh
    }
}

/// A proof that passed every check that does not need the token to be valid.
pub(crate) struct Proven {
    /// The RFC 7638 SHA-256 thumbprint of the proof's key, which the token's `cnf.jkt` must equal.
    pub(crate) jkt: String,
    /// What the replay cache remembers: the key and the `jti`.
    replay_key: String,
}

/// The DPoP checks of one authenticator.
pub(crate) struct Dpop {
    origins: Vec<Url>,
    max_age: i64,
    future_skew: i64,
    keep: Duration,
    capacity: usize,
    seen: Mutex<Seen>,
}

impl std::fmt::Debug for Dpop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dpop")
            .field("origins", &self.origins.len())
            .finish_non_exhaustive()
    }
}

/// A public origin as a URL: `http(s)://host[:port]`, no credentials, path, query or fragment.
fn origin(text: &str) -> Option<Url> {
    let url = Url::parse(text.trim()).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn refused(detail: &'static str) -> AuthError {
    AuthError::invalid_dpop_proof(detail)
}

fn b64_exact(text: &str, len: usize) -> bool {
    URL_SAFE_NO_PAD
        .decode(text)
        .is_ok_and(|bytes| bytes.len() == len)
}

/// The claims of a proof this reads. `nonce` and the rest are ignored.
#[derive(Deserialize)]
struct Claims {
    jti: String,
    htm: String,
    htu: String,
    iat: i64,
    ath: String,
}

/// The proof's key, read from the header's `jwk`: only the members of a public key of the kind
/// `alg` signs with, whatever else the header says, and the key to verify with.
fn read_key(alg: Algorithm, raw: &Value) -> Result<(Jwk, DecodingKey), AuthError> {
    let Some(raw) = raw.as_object() else {
        return Err(refused("the proof has no public key (jwk)"));
    };
    if PRIVATE_MEMBERS.iter().any(|m| raw.contains_key(*m)) {
        return Err(refused("the proof's key must be a public key"));
    }
    let text = |name: &str| raw.get(name).and_then(Value::as_str);
    let (members, key) = match (alg, text("kty"), text("crv")) {
        (Algorithm::ES256, Some("EC"), Some("P-256")) => {
            let (Some(x), Some(y)) = (text("x"), text("y")) else {
                return Err(refused("the proof's key is incomplete"));
            };
            if !b64_exact(x, 32) || !b64_exact(y, 32) {
                return Err(refused("the proof's key is malformed"));
            }
            let key = DecodingKey::from_ec_components(x, y)
                .map_err(|_| refused("the proof's key is malformed"))?;
            (
                serde_json::json!({"kty": "EC", "crv": "P-256", "x": x, "y": y}),
                key,
            )
        }
        (Algorithm::EdDSA, Some("OKP"), Some("Ed25519")) => {
            let Some(x) = text("x") else {
                return Err(refused("the proof's key is incomplete"));
            };
            if !b64_exact(x, 32) {
                return Err(refused("the proof's key is malformed"));
            }
            let key = DecodingKey::from_ed_components(x)
                .map_err(|_| refused("the proof's key is malformed"))?;
            (
                serde_json::json!({"kty": "OKP", "crv": "Ed25519", "x": x}),
                key,
            )
        }
        _ => return Err(refused("the proof's key is not of its algorithm's kind")),
    };
    let jwk = serde_json::from_value::<Jwk>(members)
        .map_err(|_| refused("the proof's key is malformed"))?;
    Ok((jwk, key))
}

impl Dpop {
    pub(crate) fn new(cfg: DpopConfig) -> Result<Self, BuildError> {
        let origins = cfg
            .public_origins
            .iter()
            .map(|text| origin(text).ok_or(BuildError::BadDpopOrigin))
            .collect::<Result<Vec<_>, _>>()?;
        if origins.is_empty() {
            return Err(BuildError::NoDpopOrigin);
        }
        if cfg.max_age.is_zero() || cfg.replay_capacity == 0 {
            return Err(BuildError::BadDpopWindow);
        }
        let secs = |d: Duration| i64::try_from(d.as_secs()).unwrap_or(i64::MAX);
        Ok(Dpop {
            origins,
            max_age: secs(cfg.max_age),
            future_skew: secs(cfg.future_skew),
            // A proof is good until `iat + max_age`, and its `iat` may be `future_skew` ahead of the
            // clock: it can be presented for `max_age + future_skew` after it is first seen, and its
            // `jti` is remembered that long.
            keep: cfg.max_age + cfg.future_skew,
            capacity: cfg.replay_capacity,
            seen: Mutex::new(Seen::default()),
        })
    }

    /// Whether `htu` is this request's public URL, ignoring its query and fragment.
    fn htu_matches(&self, htu: &str, path: &str) -> bool {
        let Ok(mut given) = Url::parse(htu) else {
            return false;
        };
        if !matches!(given.scheme(), "http" | "https")
            || !given.username().is_empty()
            || given.password().is_some()
        {
            return false;
        }
        given.set_query(None);
        given.set_fragment(None);
        self.origins.iter().any(|origin| {
            let mut expected = origin.clone();
            expected.set_path(if path.is_empty() { "/" } else { path });
            expected == given
        })
    }

    /// Every check of the proof that needs neither the token to be valid nor any state, at
    /// `now` (seconds since the epoch).
    pub(crate) fn check_proof(
        &self,
        request: &DpopCredentials<'_>,
        now: i64,
    ) -> Result<Proven, AuthError> {
        let [proof] = request.proofs else {
            return Err(refused("exactly one DPoP proof is needed"));
        };
        if proof.is_empty() || proof.len() > MAX_PROOF_BYTES {
            return Err(refused("the proof is malformed"));
        }
        // The header twice: as JSON, to see the members a typed key would drop (a private `d`), and
        // as the library reads it.
        let raw_header = proof
            .split('.')
            .next()
            .and_then(|part| URL_SAFE_NO_PAD.decode(part).ok())
            .and_then(|bytes| serde_json::from_slice::<Map<String, Value>>(&bytes).ok())
            .ok_or_else(|| refused("the proof is malformed"))?;
        let header = decode_header(proof).map_err(|_| refused("the proof is malformed"))?;
        if header.typ.as_deref() != Some("dpop+jwt") {
            return Err(refused("the proof's typ is not dpop+jwt"));
        }
        if !matches!(header.alg, Algorithm::ES256 | Algorithm::EdDSA) {
            return Err(refused("the proof's algorithm is not allowed"));
        }
        let (jwk, key) = read_key(header.alg, raw_header.get("jwk").unwrap_or(&Value::Null))?;
        let mut validation = Validation::new(header.alg);
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.validate_aud = false;
        validation.set_required_spec_claims::<&str>(&[]);
        let data = decode::<Claims>(proof, &key, &validation).map_err(|_| {
            refused("the proof's signature does not verify, or its claims are malformed")
        })?;
        let claims = data.claims;

        if claims.htm != request.method {
            return Err(refused("the proof is for another method (htm)"));
        }
        if !self.htu_matches(&claims.htu, request.path) {
            return Err(refused("the proof is for another URL (htu)"));
        }
        if claims.iat < now.saturating_sub(self.max_age) {
            return Err(refused("the proof is too old (iat)"));
        }
        if claims.iat > now.saturating_add(self.future_skew) {
            return Err(refused("the proof is from the future (iat)"));
        }
        if claims.jti.is_empty() || claims.jti.len() > MAX_JTI_BYTES {
            return Err(refused("the proof's jti is missing or too long"));
        }
        let hash = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, request.token.as_bytes());
        if claims.ath != URL_SAFE_NO_PAD.encode(hash.as_ref()) {
            return Err(refused("the proof is for another token (ath)"));
        }
        let jkt = jwk
            .thumbprint(ThumbprintHash::SHA256)
            .map_err(|_| refused("the proof's key is malformed"))?;
        Ok(Proven {
            replay_key: format!("{jkt}:{}", claims.jti),
            jkt,
        })
    }

    /// Remembers the proof's `jti`, at `now`, and says it was not seen within the window.
    ///
    /// # Errors
    /// A refusal for a proof that was seen; `Unavailable` when there is no room to remember one.
    pub(crate) fn admit(&self, proven: &Proven, now: Instant) -> Result<(), AuthError> {
        // No lock is held across an await, and a poisoned lock holds a cache that is still a set.
        let mut seen = self
            .seen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match seen.admit(proven.replay_key.clone(), now, self.keep, self.capacity) {
            Admit::Fresh => Ok(()),
            Admit::Replayed => Err(refused("the proof was already used (jti)")),
            Admit::Full => Err(AuthError::unavailable(
                "too many DPoP proofs are being tracked",
            )),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const KEEP: Duration = Duration::from_secs(65);

    #[test]
    fn a_jti_is_fresh_once_and_then_replayed_until_it_expires() {
        let mut seen = Seen::default();
        let t0 = Instant::now();
        assert_eq!(seen.admit("a".into(), t0, KEEP, 10), Admit::Fresh);
        assert_eq!(
            seen.admit("a".into(), t0 + Duration::from_secs(64), KEEP, 10),
            Admit::Replayed
        );
        // Once its window has passed it is forgotten, and a proof that old is refused by `iat`.
        assert_eq!(seen.admit("a".into(), t0 + KEEP, KEEP, 10), Admit::Fresh);
    }

    #[test]
    fn expired_entries_are_evicted_so_the_cache_stays_bounded() {
        let mut seen = Seen::default();
        let t0 = Instant::now();
        for i in 0..100 {
            let at = t0 + Duration::from_secs(i);
            assert_eq!(seen.admit(format!("k{i}"), at, KEEP, 1000), Admit::Fresh);
        }
        assert_eq!(seen.order.len(), 65, "only the last 65 s are kept");
        assert_eq!(seen.set.len(), seen.order.len());
    }

    #[test]
    fn a_full_cache_refuses_and_never_forgets_a_live_entry() {
        let mut seen = Seen::default();
        let t0 = Instant::now();
        assert_eq!(seen.admit("a".into(), t0, KEEP, 2), Admit::Fresh);
        assert_eq!(seen.admit("b".into(), t0, KEEP, 2), Admit::Fresh);
        assert_eq!(seen.admit("c".into(), t0, KEEP, 2), Admit::Full);
        // `a` is still remembered: a replay is not let in for want of room.
        assert_eq!(seen.admit("a".into(), t0, KEEP, 2), Admit::Replayed);
        // Room again once the oldest expire.
        assert_eq!(seen.admit("c".into(), t0 + KEEP, KEEP, 2), Admit::Fresh);
    }

    #[test]
    fn an_origin_has_a_scheme_a_host_and_nothing_after() {
        for ok in [
            "https://chat.example.com",
            "http://localhost:3000",
            "https://a.b:8443/",
        ] {
            assert!(origin(ok).is_some(), "{ok}");
        }
        for bad in [
            "chat.example.com",
            "ftp://chat.example.com",
            "https://u:p@chat.example.com",
            "https://chat.example.com/app",
            "https://chat.example.com?x=1",
            "https://chat.example.com#x",
            "",
        ] {
            assert!(origin(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn htu_is_compared_by_origin_and_path_without_query_or_fragment() {
        let dpop = Dpop::new(DpopConfig::new([
            "https://chat.example.com",
            "http://localhost:3000",
        ]))
        .unwrap();
        let ok = |htu: &str, path: &str| dpop.htu_matches(htu, path);
        assert!(ok("https://chat.example.com/api/threads", "/api/threads"));
        assert!(ok(
            "https://CHAT.example.com:443/api/threads",
            "/api/threads"
        ));
        assert!(ok(
            "https://chat.example.com/api/threads?x=1#y",
            "/api/threads"
        ));
        assert!(ok("http://localhost:3000/agui", "/agui"));
        assert!(!ok("https://chat.example.com/api/threads", "/api/other"));
        assert!(!ok("https://evil.example.com/api/threads", "/api/threads"));
        assert!(!ok("http://chat.example.com/api/threads", "/api/threads"));
        assert!(!ok(
            "https://chat.example.com:8443/api/threads",
            "/api/threads"
        ));
        assert!(!ok(
            "https://u@chat.example.com/api/threads",
            "/api/threads"
        ));
        assert!(!ok("not a url", "/api/threads"));
        assert!(!ok("javascript:alert(1)", "/api/threads"));
    }

    #[test]
    fn a_configuration_needs_origins_and_a_window() {
        assert!(matches!(
            Dpop::new(DpopConfig::new(Vec::<String>::new())),
            Err(BuildError::NoDpopOrigin)
        ));
        assert!(matches!(
            Dpop::new(DpopConfig::new(["https://a.example/path"])),
            Err(BuildError::BadDpopOrigin)
        ));
        assert!(matches!(
            Dpop::new(
                DpopConfig::new(["https://a.example"]).with_window(Duration::ZERO, Duration::ZERO)
            ),
            Err(BuildError::BadDpopWindow)
        ));
    }
}
