//! The check of an issued leaf, before it is trusted with a TLS connection. OpenBao is believed
//! about nothing it cannot be shown: a leaf that is for another key, another purpose, another id
//! or another time is refused, however it got to be signed.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use jiff::{SignedDuration, Timestamp};
use x509_cert::Certificate;
use x509_cert::der::Decode as _;
use x509_cert::der::asn1::ObjectIdentifier;
use x509_cert::ext::pkix::name::GeneralName;
use x509_cert::ext::pkix::{
    BasicConstraints, ExtendedKeyUsage, KeyUsage, KeyUsages, SubjectAltName,
};
use x509_cert::time::Time;

/// The chain sent to the platform is the leaf and at most seven more.
const MAX_CHAIN: usize = 8;
/// id-kp-clientAuth.
const CLIENT_AUTH: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.2");
/// id-kp-serverAuth.
const SERVER_AUTH: ObjectIdentifier = ObjectIdentifier::new_unwrap("1.3.6.1.5.5.7.3.1");
/// anyExtendedKeyUsage: a leaf that says it may be used for anything is not for clients only.
const ANY_EXTENDED_KEY_USAGE: ObjectIdentifier = ObjectIdentifier::new_unwrap("2.5.29.37.0");

/// Why a leaf is not used. A phrase: nothing of the certificate is in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum LeafError {
    #[error("it, or a certificate sent with it, cannot be read")]
    Unreadable,
    #[error("it is for another key than the one the request was made with")]
    OtherKey,
    #[error("it is not for client authentication only")]
    Purpose,
    #[error("it is a certificate authority, or may sign certificates")]
    Authority,
    #[error("it does not carry exactly the configured SPIFFE id and nothing else as its names")]
    Identity,
    #[error("it is not valid now")]
    NotValidNow,
    #[error("it lives no longer than the renewal margin")]
    TooShort,
    #[error("its chain is not a leaf, its intermediates and no root, in a usable number")]
    Chain,
}

/// What the leaf is checked against.
pub(crate) struct Expected<'a> {
    /// The public point of the key the CSR was made with.
    pub public_point: &'a [u8],
    /// The one URI name the leaf must carry.
    pub spiffe_id: &'a str,
    /// Now, on the adapter's clock.
    pub now: Timestamp,
    /// The renewal margin: a leaf that does not outlive it would be renewed at every call.
    pub margin: SignedDuration,
}

/// A leaf that passed.
pub(crate) struct Verified {
    /// The DER of the leaf, then of its intermediates (no root).
    pub chain: Vec<Vec<u8>>,
    /// The leaf's serial number in hexadecimal, bytes joined by `:`.
    pub serial: String,
    /// The end of the leaf's validity.
    pub not_after: Timestamp,
}

/// The DER of every `CERTIFICATE` block of a PEM text.
pub(crate) fn pem_certificates(text: &str) -> Result<Vec<Vec<u8>>, LeafError> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(BEGIN) {
        let after = &rest[start + BEGIN.len()..];
        let end = after.find(END).ok_or(LeafError::Unreadable)?;
        let body: String = after[..end]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        out.push(B64.decode(body).map_err(|_| LeafError::Unreadable)?);
        if out.len() > MAX_CHAIN {
            return Err(LeafError::Chain);
        }
        rest = &after[end + END.len()..];
    }
    Ok(out)
}

fn timestamp(time: Time) -> Result<Timestamp, LeafError> {
    let since_epoch =
        SignedDuration::try_from(time.to_unix_duration()).map_err(|_| LeafError::Unreadable)?;
    Timestamp::from_duration(since_epoch).map_err(|_| LeafError::Unreadable)
}

fn serial_text(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 3);
    for (i, byte) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(':');
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Checks the leaf and builds the chain to present: the leaf, then the certificates OpenBao sent
/// with it that are not roots (a root is the platform's to hold, never ours to send), each once,
/// at most eight in all.
pub(crate) fn verify(
    leaf_pem: &str,
    others_pem: &[&str],
    expected: &Expected<'_>,
) -> Result<Verified, LeafError> {
    let mut leaves = pem_certificates(leaf_pem)?;
    let (Some(leaf_der), true) = (leaves.pop(), leaves.is_empty()) else {
        return Err(LeafError::Unreadable);
    };
    let cert = Certificate::from_der(&leaf_der).map_err(|_| LeafError::Unreadable)?;
    let tbs = &cert.tbs_certificate;

    // 1. Our key: a leaf for another key is of no use to us and signed for somebody else.
    if tbs.subject_public_key_info.subject_public_key.raw_bytes() != expected.public_point {
        return Err(LeafError::OtherKey);
    }

    // 2. For clients, and for nothing else: clientAuth present, serverAuth absent.
    match tbs.get::<ExtendedKeyUsage>() {
        Ok(Some((_, ExtendedKeyUsage(purposes))))
            if purposes.contains(&CLIENT_AUTH)
                && !purposes.contains(&SERVER_AUTH)
                && !purposes.contains(&ANY_EXTENDED_KEY_USAGE) => {}
        Ok(_) => return Err(LeafError::Purpose),
        Err(_) => return Err(LeafError::Unreadable),
    }

    // 2b. A leaf, not an authority: no `cA`, and no right to sign certificates.
    match tbs.get::<BasicConstraints>() {
        Ok(Some((_, constraints))) if constraints.ca => return Err(LeafError::Authority),
        Ok(_) => {}
        Err(_) => return Err(LeafError::Unreadable),
    }
    match tbs.get::<KeyUsage>() {
        Ok(Some((_, usage))) if usage.0.contains(KeyUsages::KeyCertSign) => {
            return Err(LeafError::Authority);
        }
        Ok(_) => {}
        Err(_) => return Err(LeafError::Unreadable),
    }

    // 3. Exactly one name, and it is the configured id.
    let names = match tbs.get::<SubjectAltName>() {
        Ok(Some((_, SubjectAltName(names)))) => names,
        Ok(None) => return Err(LeafError::Identity),
        Err(_) => return Err(LeafError::Unreadable),
    };
    match names.as_slice() {
        [GeneralName::UniformResourceIdentifier(uri)] if uri.as_str() == expected.spiffe_id => {}
        _ => return Err(LeafError::Identity),
    }

    // 4. Valid now, and for longer than the margin.
    let not_before = timestamp(tbs.validity.not_before)?;
    let not_after = timestamp(tbs.validity.not_after)?;
    if expected.now < not_before || expected.now >= not_after {
        return Err(LeafError::NotValidNow);
    }
    let renew_at = not_after
        .checked_sub(expected.margin)
        .map_err(|_| LeafError::TooShort)?;
    if expected.now >= renew_at {
        return Err(LeafError::TooShort);
    }
    if tbs.issuer == tbs.subject {
        // A self-signed leaf is no leaf of a PKI.
        return Err(LeafError::Chain);
    }

    // 5. The chain: the leaf, then what is not a root.
    let serial = serial_text(tbs.serial_number.as_bytes());
    let mut chain = vec![leaf_der];
    for pem in others_pem {
        for der in pem_certificates(pem)? {
            let other = Certificate::from_der(&der).map_err(|_| LeafError::Unreadable)?;
            let is_root = other.tbs_certificate.issuer == other.tbs_certificate.subject;
            if is_root || chain.contains(&der) {
                continue;
            }
            chain.push(der);
        }
    }
    if chain.len() > MAX_CHAIN {
        return Err(LeafError::Chain);
    }
    Ok(Verified {
        chain,
        serial,
        not_after,
    })
}
