//! The receiving side, as `docs/api/signed-tool-requests-v1.md` ("What a tool server must check") describes it, checks 1 to 5.
//!
//! The orchestrator never verifies. This is the reference of the contract: the tests of the orchestrator's client verify what it
//! sends with it, and a Rust tool server that does not use `cratestack-cose` can use it. It is **not** a second implementation
//! of `cratestack_cose::open_claims` to trust in its place: the interoperability crate (`interop/`) runs the real one against
//! the same bytes.
//!
//! What it leaves to the caller, because it needs state or policy: check 6, that the `cti` was not seen (it returns the pair to
//! record and the time to keep it until), and check 7, that the owner may do what the call asks.

use aws_lc_rs::signature::{ED25519, UnparsedPublicKey};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq as _;
use uuid::Uuid;

use crate::cbor::{self, Reader, Reject};
use crate::key::{KID_LEN, hex, kid_of};
use crate::sign::{CTI_LEN, MAX_HEADER_BYTES, MAX_TTL_SECS, TYP, to_be_signed};

/// The most public keys a verifier pins: the current one and the next (a rotation).
pub const MAX_PINNED_KEYS: usize = 2;

/// The request as the tool server received it.
#[derive(Debug, Clone, Copy)]
pub struct Incoming<'a> {
    /// The method of the request line.
    pub method: &'a str,
    /// The path the server publishes for itself (the path of the URL it gave the orchestrator).
    pub path: &'a str,
    /// The query string, if the request had one: the orchestrator sends none.
    pub query: Option<&'a str>,
    /// The body bytes, exactly as received and before any parsing.
    pub body: &'a [u8],
    /// The `Content-Encoding` of the request, if it had one: the orchestrator sends none.
    pub content_encoding: Option<&'a str>,
}

/// What a verified object says. **Verified is not accepted**: the caller still records the `cti` (check 6) and authorises
/// the owner (check 7).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Verified {
    /// The `kid` of the key that verified.
    pub kid: [u8; KID_LEN],
    /// The `cti`: record `(kid, cti)` until [`keep_until`](Self::keep_until), and refuse the request if it is already there.
    pub cti: [u8; CTI_LEN],
    /// When the object was issued.
    pub iat: u64,
    /// When it expires.
    pub exp: u64,
    /// The time until which the `cti` must be remembered: `exp` plus the skew.
    pub keep_until: u64,
    /// The thread the call is made for.
    pub thread: Uuid,
    /// The owner of the thread: the person the tool runs for.
    pub owner: String,
}

/// Check 1 failed: the header, or the shape of the request around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Shape {
    /// No header.
    Missing,
    /// More than one.
    Repeated,
    /// Longer than [`MAX_HEADER_BYTES`].
    TooLong,
    /// A character outside the unpadded base64url alphabet, or not canonical base64.
    NotBase64Url,
    /// The request has a query string.
    Query,
    /// The request has a `Content-Encoding`.
    ContentEncoding,
}

/// Check 2 failed: the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Object {
    /// Not the claim-object profile: wrong tag, a head that is not the shortest, an entry out of order or unknown, a
    /// non-empty unprotected header, trailing bytes, an algorithm other than Ed25519.
    Malformed,
    /// A well-formed object of another `typ`.
    WrongType,
    /// No pinned key has the `kid`, or no key with it verifies the signature (the two are not told apart).
    Unverified,
}

/// Check 4 failed: the time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Time {
    /// `iat`, `exp` or `cti` is absent.
    Missing,
    /// `exp` is before `iat`, or more than [`MAX_TTL_SECS`] after it.
    Lifetime,
    /// `iat` is more than the skew in the future.
    NotYet,
    /// `exp` passed more than the skew ago.
    Expired,
}

/// Check 5 failed: the payload does not describe this request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Mismatch {
    /// The payload is not exactly the five members, of the right types (or the thread is not a canonical UUID).
    Payload,
    /// `method`.
    Method,
    /// `path`.
    Path,
    /// `body_sha256`.
    Body,
}

/// A check that failed. The numbers are the contract's. A server answers 401 for every one of them and says nothing of which.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum VerifyError {
    /// Check 1.
    #[error("check 1: the header ({0:?})")]
    Header(Shape),
    /// Check 2.
    #[error("check 2: the object ({0:?})")]
    Object(Object),
    /// Check 3.
    #[error("check 3: the audience")]
    Audience,
    /// Check 4.
    #[error("check 4: the time ({0:?})")]
    Time(Time),
    /// Check 5.
    #[error("check 5: the request ({0:?})")]
    Request(Mismatch),
    /// More than [`MAX_PINNED_KEYS`] keys were pinned.
    #[error("at most two keys are pinned")]
    TooManyKeys,
}

impl VerifyError {
    /// The number of the check, 1 to 5 (0 for a verifier that was not set up).
    pub const fn check(&self) -> u8 {
        match self {
            VerifyError::Header(_) => 1,
            VerifyError::Object(_) => 2,
            VerifyError::Audience => 3,
            VerifyError::Time(_) => 4,
            VerifyError::Request(_) => 5,
            VerifyError::TooManyKeys => 0,
        }
    }
}

/// Checks 1 to 5 for one tool server: its own name, its pinned keys and its allowance for clock skew.
#[derive(Debug, Clone)]
pub struct Verifier {
    audience: String,
    skew_secs: u64,
    keys: Vec<([u8; KID_LEN], [u8; 32])>,
}

impl Verifier {
    /// A verifier for the server that calls itself `audience`, allowing `skew_secs` of difference between the clocks (30 in
    /// the contract). Pin the orchestrator's public key with [`pin`](Self::pin).
    pub fn new(audience: impl Into<String>, skew_secs: u64) -> Self {
        Verifier {
            audience: audience.into(),
            skew_secs,
            keys: Vec::new(),
        }
    }

    /// Pins an Ed25519 public key.
    ///
    /// # Errors
    /// [`VerifyError::TooManyKeys`] past [`MAX_PINNED_KEYS`].
    pub fn pin(mut self, public_key: [u8; 32]) -> Result<Self, VerifyError> {
        if self.keys.len() >= MAX_PINNED_KEYS {
            return Err(VerifyError::TooManyKeys);
        }
        self.keys.push((kid_of(&public_key), public_key));
        Ok(self)
    }

    /// Checks 1 to 5. `headers` are all the values of `Tool-Request-Claims` the request had, `now` is whole seconds since the Unix
    /// epoch.
    ///
    /// # Errors
    /// The first check that fails.
    pub fn verify(
        &self,
        headers: &[&str],
        request: &Incoming<'_>,
        now: u64,
    ) -> Result<Verified, VerifyError> {
        // 1
        let value = match headers {
            [] => return Err(VerifyError::Header(Shape::Missing)),
            [one] => *one,
            _ => return Err(VerifyError::Header(Shape::Repeated)),
        };
        if request.query.is_some() {
            return Err(VerifyError::Header(Shape::Query));
        }
        if request.content_encoding.is_some() {
            return Err(VerifyError::Header(Shape::ContentEncoding));
        }
        if value.len() > MAX_HEADER_BYTES {
            return Err(VerifyError::Header(Shape::TooLong));
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|_| VerifyError::Header(Shape::NotBase64Url))?;
        // 2
        let object = parse(&bytes).map_err(|_| VerifyError::Object(Object::Malformed))?;
        if object.typ != TYP {
            return Err(VerifyError::Object(Object::WrongType));
        }
        let message = to_be_signed(object.protected, object.payload);
        let verified = self
            .keys
            .iter()
            .filter(|(kid, _)| *kid == object.kid)
            .any(|(_, public)| {
                UnparsedPublicKey::new(&ED25519, public)
                    .verify(&message, object.signature)
                    .is_ok()
            });
        if !verified {
            return Err(VerifyError::Object(Object::Unverified));
        }
        // 3
        if object.claims.aud.as_deref() != Some(self.audience.as_str()) {
            return Err(VerifyError::Audience);
        }
        // 4
        let (Some(iat), Some(exp), Some(cti)) =
            (object.claims.iat, object.claims.exp, object.claims.cti)
        else {
            return Err(VerifyError::Time(Time::Missing));
        };
        let cti: [u8; CTI_LEN] = cti
            .try_into()
            .map_err(|_| VerifyError::Time(Time::Missing))?;
        if exp < iat || exp - iat > MAX_TTL_SECS {
            return Err(VerifyError::Time(Time::Lifetime));
        }
        if iat > now.saturating_add(self.skew_secs) {
            return Err(VerifyError::Time(Time::NotYet));
        }
        if now > exp.saturating_add(self.skew_secs) {
            return Err(VerifyError::Time(Time::Expired));
        }
        // 5
        let binding: Binding = serde_json::from_slice(object.payload)
            .map_err(|_| VerifyError::Request(Mismatch::Payload))?;
        let thread = Uuid::parse_str(&binding.thread)
            .ok()
            .filter(|thread| thread.hyphenated().to_string() == binding.thread)
            .ok_or(VerifyError::Request(Mismatch::Payload))?;
        if binding.owner.is_empty() || binding.body_sha256.len() != 64 {
            return Err(VerifyError::Request(Mismatch::Payload));
        }
        if !matches!(binding.method.as_str(), "POST" | "GET" | "DELETE")
            || binding.method != request.method
        {
            return Err(VerifyError::Request(Mismatch::Method));
        }
        if binding.path != request.path {
            return Err(VerifyError::Request(Mismatch::Path));
        }
        let digest = hex(&Sha256::digest(request.body));
        if !bool::from(digest.as_bytes().ct_eq(binding.body_sha256.as_bytes())) {
            return Err(VerifyError::Request(Mismatch::Body));
        }
        Ok(Verified {
            kid: object.kid,
            cti,
            iat,
            exp,
            keep_until: exp.saturating_add(self.skew_secs),
            thread,
            owner: binding.owner,
        })
    }
}

/// The payload: exactly these five members.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    method: String,
    path: String,
    body_sha256: String,
    thread: String,
    owner: String,
}

/// The CWT claims of the protected header, those that are present.
#[derive(Default)]
struct Claims {
    aud: Option<String>,
    exp: Option<u64>,
    iat: Option<u64>,
    cti: Option<Vec<u8>>,
}

/// A parsed object: slices into the bytes it was read from.
struct Parsed<'a> {
    protected: &'a [u8],
    payload: &'a [u8],
    signature: &'a [u8],
    kid: [u8; KID_LEN],
    claims: Claims,
    typ: String,
}

/// The profile of `cratestack_cose::open_claims`, strictly: `#6.18([protected, {}, payload, signature])`, the protected header
/// `{1: -19, 4: kid, ? 15: {claims, ascending}, 16: typ}`.
fn parse(bytes: &[u8]) -> Result<Parsed<'_>, Reject> {
    let mut outer = Reader::new(bytes);
    if outer.expect(cbor::MAJOR_TAG)? != 18 || outer.expect(cbor::MAJOR_ARRAY)? != 4 {
        return Err(Reject);
    }
    let protected = outer.bstr()?;
    outer.exact(cbor::EMPTY_MAP)?;
    let payload = outer.bstr()?;
    let signature = outer.bstr()?;
    if !outer.is_at_end() || signature.len() != 64 {
        return Err(Reject);
    }

    let mut header = Reader::new(protected);
    let entries = header.expect(cbor::MAJOR_MAP)?;
    if entries != 3 && entries != 4 {
        return Err(Reject);
    }
    // 1: -19
    if header.uint()? != 1 || header.nint()? != 18 {
        return Err(Reject);
    }
    if header.uint()? != 4 {
        return Err(Reject);
    }
    let kid: [u8; KID_LEN] = header.bstr()?.try_into().map_err(|_| Reject)?;
    let claims = if entries == 4 {
        if header.uint()? != 15 {
            return Err(Reject);
        }
        claims(&mut header)?
    } else {
        Claims::default()
    };
    if header.uint()? != 16 {
        return Err(Reject);
    }
    let typ = header.tstr()?.to_owned();
    if !header.is_at_end() {
        return Err(Reject);
    }
    Ok(Parsed {
        protected,
        payload,
        signature,
        kid,
        claims,
        typ,
    })
}

fn claims(reader: &mut Reader<'_>) -> Result<Claims, Reject> {
    let entries = reader.expect(cbor::MAJOR_MAP)?;
    if !(1..=4).contains(&entries) {
        return Err(Reject);
    }
    let mut claims = Claims::default();
    let mut previous = 0;
    for _ in 0..entries {
        let label = reader.uint()?;
        if label <= previous {
            return Err(Reject);
        }
        previous = label;
        match label {
            3 => claims.aud = Some(reader.tstr()?.to_owned()),
            4 => claims.exp = Some(reader.uint()?),
            6 => claims.iat = Some(reader.uint()?),
            7 => {
                let cti = reader.bstr()?;
                if cti.is_empty() || cti.len() > 64 {
                    return Err(Reject);
                }
                claims.cti = Some(cti.to_vec());
            }
            _ => return Err(Reject),
        }
    }
    Ok(claims)
}
