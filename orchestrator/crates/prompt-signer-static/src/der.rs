//! The two things this adapter reads from its leaf certificate: the Ed25519 key it certifies and its serial number. A
//! reader of the few DER shapes on that path, not an X.509 parser: the chain is the verifier's to judge.

/// The SubjectPublicKeyInfo of an Ed25519 key up to the key (RFC 8410 §4): the 32 bytes that follow are the key.
const ED25519_SPKI_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
];

/// The Ed25519 key the certificate holds, when it holds exactly one.
pub(crate) fn ed25519_key(cert: &[u8]) -> Option<[u8; 32]> {
    let mut found = cert
        .windows(ED25519_SPKI_PREFIX.len())
        .enumerate()
        .filter(|(_, w)| *w == ED25519_SPKI_PREFIX)
        .map(|(at, _)| at + ED25519_SPKI_PREFIX.len());
    let at = found.next()?;
    if found.next().is_some() {
        return None;
    }
    cert.get(at..at + 32)?.try_into().ok()
}

/// The certificate's serial number (`tbsCertificate.serialNumber`), lower-case hexadecimal without leading zeros.
pub(crate) fn serial_hex(cert: &[u8]) -> Option<String> {
    let (tag, certificate, _) = tlv(cert)?;
    if tag != 0x30 {
        return None;
    }
    let (tag, tbs, _) = tlv(certificate)?;
    if tag != 0x30 {
        return None;
    }
    let (tag, value, rest) = tlv(tbs)?;
    let (tag, value) = if tag == 0xa0 {
        // [0] EXPLICIT version, then the serial
        let (tag, value, _) = tlv(rest)?;
        (tag, value)
    } else {
        (tag, value)
    };
    if tag != 0x02 || value.is_empty() {
        return None;
    }
    let digits: String = value.iter().map(|b| format!("{b:02x}")).collect();
    let trimmed = digits.trim_start_matches('0');
    Some(if trimmed.is_empty() { "0" } else { trimmed }.to_owned())
}

/// One DER element: its tag, its contents and what follows it. Definite lengths only, of at most four bytes.
fn tlv(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n]
            .iter()
            .fold(0_usize, |acc, b| (acc << 8) | usize::from(*b));
        (len, &rest[n..])
    };
    if rest.len() < len {
        return None;
    }
    Some((tag, &rest[..len], &rest[len..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_serial_after_the_version() {
        // SEQ { SEQ { [0] { INT 2 }, INT 0x00 0x10 0x01 } }
        let cert = [
            0x30, 0x0c, 0x30, 0x0a, 0xa0, 0x03, 0x02, 0x01, 0x02, 0x02, 0x03, 0x00, 0x10, 0x01,
        ];
        assert_eq!(serial_hex(&cert).as_deref(), Some("1001"));
    }

    #[test]
    fn a_cut_element_is_none() {
        assert_eq!(serial_hex(&[0x30, 0x0c, 0x30]), None);
        assert_eq!(serial_hex(&[]), None);
        assert_eq!(ed25519_key(&ED25519_SPKI_PREFIX), None);
    }
}
