//! The presence check of a signing client's prompt (ADR 0087, decision 1; the owner's decision of 2026-11-25).
//!
//! The orchestrator does not verify the person's signature on the cloud path. It checks that the envelope is there and
//! shaped like a compact JWS, and passes the received string on unchanged. Presence and shape only: no decoding of the
//! payload, no cryptography.

/// What the presence check refused: ADR 0087's `403 prompt_signature_invalid`, reason `invalid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PresenceError {
    /// A signing client sent no `vymalo.promptSignature`.
    #[error("a signing client's prompt carries no signature")]
    Missing,
    /// The value is not three non-empty, base64url, dot-separated parts.
    #[error("the signature is not a compact JWS of three parts")]
    Malformed,
}

impl PresenceError {
    /// The problem's `code` for the client.
    pub const fn code(&self) -> &'static str {
        "prompt_signature_invalid"
    }

    /// The `reason` the client sees: `invalid`.
    pub const fn client_reason(&self) -> &'static str {
        "invalid"
    }

    /// The finer reason, for the orchestrator's log and never the response.
    pub const fn log_reason(&self) -> &'static str {
        match self {
            PresenceError::Missing => "missing",
            PresenceError::Malformed => "malformed",
        }
    }
}

/// The longest envelope accepted: a header, a signature and a detached payload leave no room for more.
pub const MAX_ENVELOPE_LEN: usize = 16 * 1024;

/// Checks that a signing client's envelope is present and shaped like a compact JWS (RFC 7515): three non-empty parts
/// of base64url characters separated by two dots, except that the middle part (the detached payload, appendix F) may
/// be empty. Returns the received string itself, never a decoded and re-encoded copy.
///
/// # Errors
/// [`PresenceError::Missing`] for `None` or an empty string, [`PresenceError::Malformed`] for any other shape.
pub fn check_presence(envelope: Option<&str>) -> Result<&str, PresenceError> {
    let envelope = match envelope {
        None | Some("") => return Err(PresenceError::Missing),
        Some(value) => value,
    };
    if envelope.len() > MAX_ENVELOPE_LEN {
        return Err(PresenceError::Malformed);
    }
    let mut parts = envelope.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(PresenceError::Malformed);
    };
    let base64url = |s: &str| {
        s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    if header.is_empty()
        || signature.is_empty()
        || !base64url(header)
        || !base64url(payload)
        || !base64url(signature)
    {
        return Err(PresenceError::Malformed);
    }
    Ok(envelope)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_detached_compact_jws_passes_and_is_returned_as_received() {
        let envelope = "eyJhbGciOiJFUzI1NiJ9..c2ln-_x";
        assert_eq!(check_presence(Some(envelope)), Ok(envelope));
        assert!(std::ptr::eq(
            check_presence(Some(envelope)).unwrap(),
            envelope
        ));
    }

    #[test]
    fn an_attached_payload_is_still_three_parts() {
        assert!(check_presence(Some("aa.bb.cc")).is_ok());
    }

    #[test]
    fn nothing_is_missing() {
        assert_eq!(check_presence(None), Err(PresenceError::Missing));
        assert_eq!(check_presence(Some("")), Err(PresenceError::Missing));
    }

    #[test]
    fn any_other_shape_is_malformed() {
        for bad in [
            "a.b", "a.b.c.d", ".b.c", "a.b.", "a..", "a b.c.d", "a.b.c=", "a.b.c\n", "ä.b.c", "...",
        ] {
            assert_eq!(
                check_presence(Some(bad)),
                Err(PresenceError::Malformed),
                "{bad:?}"
            );
        }
        let long = format!("a..{}", "c".repeat(MAX_ENVELOPE_LEN));
        assert_eq!(check_presence(Some(&long)), Err(PresenceError::Malformed));
    }

    #[test]
    fn the_refusal_is_the_contracts() {
        for e in [PresenceError::Missing, PresenceError::Malformed] {
            assert_eq!(e.code(), "prompt_signature_invalid");
            assert_eq!(e.client_reason(), "invalid");
        }
        assert_eq!(PresenceError::Missing.log_reason(), "missing");
        assert_eq!(PresenceError::Malformed.log_reason(), "malformed");
    }
}
