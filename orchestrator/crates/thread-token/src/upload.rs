//! The id of an upload slot (`uploads/v1`, `docs/api/uploads-v1.md`).
//!
//! `request_upload` gives an agent a presigned `PUT` and an **upload id**. The agent hands the id
//! back in the artifact that names the file, and the dispatcher reads it **with no table**: the
//! orchestrator stays stateless, and any replica that holds the thread-tools key checks an id any
//! replica issued. So the id carries what the dispatcher must know to check the object (the thread,
//! the job, the file's hash, size and type, and when the URL stopped working), in clear, and an
//! HMAC-SHA-256 over all of it: nobody who lacks the key can make an id, and nobody can change what
//! one says.
//!
//! ```text
//! id       "<kid>" "." base64url(payload) "." base64url(MAC)
//! payload  {"t":"<thread>","j":<job>,"h":"<64 hex>","s":<bytes>,"m":"<type>","e":<unix s>}
//! MAC      HMAC-SHA-256(key of <kid>, "uploads/v1\n" "<kid>" "." base64url(payload))
//! ```
//!
//! The first line of what is signed is a domain: a signature of the thread-tools token (which has
//! the same key) is never a valid MAC of an id, and an id is never a valid token (its first segment
//! is a `kid`, not a header). The key's `kid` is the one of the token (the first 16 hex characters of
//! the SHA-256 of the key), so a rotation works the same way: the id says which key signed it, and a
//! replica that holds the current or the previous key reads it.
//!
//! Pure like the rest of the crate: no I/O, no clock. Whether the URL had expired when the object
//! arrived is the caller's to decide, from [`UploadClaims::expires_at`] and the object's own time.

use std::fmt::Write as _;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::Mac as _;
use jiff::Timestamp;
use orch_core::ThreadId;
use serde::{Deserialize, Serialize};

use crate::ThreadToolsKeys;

/// The line a MAC of an id begins with: what separates it from every other signature this key makes.
pub const UPLOAD_ID_DOMAIN: &str = "uploads/v1";

/// The longest id [`verify_upload_id`] reads, in bytes. The longest a well-formed one can be
/// (the longest type, the largest numbers) is about 620.
pub const MAX_UPLOAD_ID_BYTES: usize = 1024;

/// The longest media type an id carries, as the artifact store keeps one.
const MAX_MEDIA_TYPE_BYTES: usize = 255;

/// What an upload id says: the slot the orchestrator issued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadClaims {
    /// The thread the file is for: the one of the key it is put under.
    pub thread: ThreadId,
    /// The job the slot was issued in, from 1.
    pub job: u32,
    /// The SHA-256 of the content: the second half of the object's key.
    pub sha256: [u8; 32],
    /// The size in bytes, at least 1.
    pub size: u64,
    /// The media type the object is put with (the signed `content-type`): plain visible ASCII.
    pub media_type: String,
    /// When the URL stopped working, whole seconds.
    pub expires_at: Timestamp,
}

impl UploadClaims {
    /// Whether the fields agree: a job from 1, at least one byte, a type that is plain visible
    /// ASCII and fits a header, and whole seconds.
    pub fn is_well_formed(&self) -> bool {
        self.job >= 1
            && self.size >= 1
            && !self.media_type.is_empty()
            && self.media_type.len() <= MAX_MEDIA_TYPE_BYTES
            && self
                .media_type
                .chars()
                .all(|c| c.is_ascii_graphic() || c == ' ')
            && self.expires_at.subsec_nanosecond() == 0
    }
}

/// The claims as they are written: short names, in this order (the order is part of the bytes the
/// known-answer vector pins).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    t: ThreadId,
    j: u32,
    h: String,
    s: u64,
    m: String,
    e: i64,
}

/// Why an id could not be written or was refused. The tool answers every refusal of an id with the
/// same words ("the file could not be kept"); the variants are for the operator's log and the
/// tests, and none shows a byte of the id or of a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UploadIdError {
    /// The claims do not agree ([`UploadClaims::is_well_formed`]).
    #[error("the claims of the upload id are not consistent")]
    Claims,
    /// More than [`MAX_UPLOAD_ID_BYTES`].
    #[error("the upload id is too long")]
    TooLong,
    /// Not three segments of strict base64url (the first a `kid`).
    #[error("the upload id is malformed")]
    Malformed,
    /// A `kid` that is neither the current nor the previous key.
    #[error("the upload id was signed with an unknown key")]
    UnknownKey,
    /// The MAC does not match.
    #[error("the upload id signature does not match")]
    Signature,
}

fn hex(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    let digits = text.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let value = |d: u8| match d {
        b'0'..=b'9' => Some(d - b'0'),
        b'a'..=b'f' => Some(d - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(digits.chunks_exact(2)) {
        *slot = value(pair[0])? << 4 | value(pair[1])?;
    }
    Some(out)
}

/// What is signed: the domain, then the first two segments as written.
fn signing_input(kid: &str, payload: &str) -> String {
    format!("{UPLOAD_ID_DOMAIN}\n{kid}.{payload}")
}

/// Writes the id for `claims`, signed with the current key. It never fails for claims that are well
/// formed.
///
/// # Errors
///
/// [`UploadIdError::Claims`] when the claims do not agree.
pub fn mint_upload_id(
    keys: &ThreadToolsKeys,
    claims: &UploadClaims,
) -> Result<String, UploadIdError> {
    if !claims.is_well_formed() {
        return Err(UploadIdError::Claims);
    }
    let wire = Wire {
        t: claims.thread,
        j: claims.job,
        h: hex(&claims.sha256),
        s: claims.size,
        m: claims.media_type.clone(),
        e: claims.expires_at.as_second(),
    };
    let payload = serde_json::to_vec(&wire)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| UploadIdError::Claims)?;
    let kid = keys.current.kid.as_str();
    let mut mac = keys.current.mac().map_err(|_| UploadIdError::Claims)?;
    mac.update(signing_input(kid, &payload).as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("{kid}.{payload}.{signature}"))
}

/// Checks an id and returns what it says: at most [`MAX_UPLOAD_ID_BYTES`], three segments, a `kid`
/// that names the current or the previous key, the MAC (compared in constant time), then the
/// claims, every one present and well formed, no other.
///
/// It does not look at the time: whether the URL had expired when the object arrived is for the
/// caller, from [`UploadClaims::expires_at`].
///
/// # Errors
///
/// The [`UploadIdError`] of the first check that fails.
pub fn verify_upload_id(keys: &ThreadToolsKeys, id: &str) -> Result<UploadClaims, UploadIdError> {
    if id.len() > MAX_UPLOAD_ID_BYTES {
        return Err(UploadIdError::TooLong);
    }
    let mut segments = id.split('.');
    let (Some(kid), Some(payload), Some(signature), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(UploadIdError::Malformed);
    };
    let payload_bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| UploadIdError::Malformed)?;
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| UploadIdError::Malformed)?;
    let key = keys.by_kid(kid).ok_or(UploadIdError::UnknownKey)?;
    let mut mac = key.mac().map_err(|_| UploadIdError::UnknownKey)?;
    mac.update(signing_input(kid, payload).as_bytes());
    mac.verify_slice(&signature_bytes)
        .map_err(|_| UploadIdError::Signature)?;

    let wire: Wire =
        serde_json::from_slice(&payload_bytes).map_err(|_| UploadIdError::Malformed)?;
    let claims = UploadClaims {
        thread: wire.t,
        job: wire.j,
        sha256: unhex(&wire.h).ok_or(UploadIdError::Malformed)?,
        size: wire.s,
        media_type: wire.m,
        expires_at: Timestamp::from_second(wire.e).map_err(|_| UploadIdError::Malformed)?,
    };
    if !claims.is_well_formed() {
        return Err(UploadIdError::Malformed);
    }
    Ok(claims)
}
