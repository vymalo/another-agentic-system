//! The compact JWS around a statement (RFC 7515, attached payload): what a signer adapter builds around its signature,
//! so that every adapter writes the same bytes. It signs nothing itself.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use serde_json::json;

use crate::jcs::canonical;
use crate::statement::{AttestedPayload, STATEMENT_TYP};

/// The protected header, RFC 8785: `{"alg":"EdDSA","typ":"prompt-statement/v1","x5c":[...]}`, where `x5c` is the DER
/// of each certificate in standard base64 with padding (RFC 7515 §4.1.6), the leaf first and the root left out.
pub fn protected_header<C: AsRef<[u8]>>(x5c: &[C]) -> Vec<u8> {
    let chain: Vec<String> = x5c.iter().map(|der| STANDARD.encode(der)).collect();
    canonical(&json!({ "alg": "EdDSA", "typ": STATEMENT_TYP, "x5c": chain }))
}

/// `base64url(header) . base64url(payload)`: the bytes the Ed25519 signature is over.
pub fn signing_input(protected_header: &[u8], payload: &AttestedPayload) -> String {
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(protected_header),
        URL_SAFE_NO_PAD.encode(payload.as_bytes())
    )
}

/// The compact JWS: the signing input, a dot, and `base64url(signature)`.
pub fn compact(signing_input: &str, signature: &[u8]) -> String {
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
}
