//! ADR 0087's **presence check at intake**, in one place for every route that writes a person's
//! words.
//!
//! Two routes write a `user_message` and delegate it to an agent: the AG-UI run
//! (`orch-surface-agui`, a new message or a Stop & send) and the resource API's fork
//! (`POST /api/threads/{id}/fork`, `after` with `text` or `replace`). A prompt from a **signing
//! client** (`auth.signingClients`) must carry `vymalo.promptSignature`, and the one function below
//! decides whether it does — so the two routes cannot drift apart on the answer.
//!
//! The check is presence and shape only: no cryptography happens here (the verifier behind the
//! model gateway checks the signature), the envelope is passed on as it arrived, and the finer
//! reason (`missing`, `malformed`) is logged and never sent. A client that is not listed, and a
//! deployment that lists none, is never checked.

use orch_ports::Principal;

use crate::Problem;

/// ADR 0087, decisions 1, 3 and 7: a prompt from a **signing client** (`auth.signingClients`) that
/// carries no envelope, or one that is not a compact JWS, is refused with `403
/// prompt_signature_invalid`, reason `invalid`, before anything is written or any agent is called.
///
/// `envelope` is the person's prompt signature as the route received it (`None` when the request
/// carries none). A client that is not listed, and a deployment that lists none, is never checked,
/// so `Ok(())` is the ordinary answer.
///
/// It is called only for an input that writes the person's words (a message, a Stop & send, a
/// fork's first message or an edit), and before the rate limit and the allowance: a message that is
/// both unsigned and over the limit is the 403, and nothing is counted.
///
/// # Errors
/// The `403 prompt_signature_invalid` problem, for a signing client whose envelope is missing or
/// malformed. The envelope itself is in no problem, and in no log line.
pub fn check(
    principal: &Principal,
    signing_clients: &[String],
    envelope: Option<&str>,
) -> Result<(), Problem> {
    if !orch_prompt_statement::requires_signature(signing_clients, principal.client_id.as_deref()) {
        return Ok(());
    }
    match orch_prompt_statement::check_presence(envelope) {
        Ok(_) => Ok(()),
        Err(refusal) => {
            tracing::warn!(
                reason = refusal.log_reason(),
                "a signing client's prompt was refused"
            );
            Err(Problem::forbidden(
                "this message couldn't be verified as coming from your device; please try again",
            )
            .with_code(refusal.code())
            .with_reason(refusal.client_reason()))
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    /// A compact JWS of three base64url parts with a detached payload: what a signing app sends.
    const ENVELOPE: &str =
        "eyJhbGciOiJFZERTQSIsImtpZCI6ImRldmljZS0xI3YxIn0..c2lnbmF0dXJlLW9mLXRoZS1wcm9tcHQ";

    fn signing(client: Option<&str>) -> Principal {
        Principal {
            client_id: client.map(str::to_owned),
            ..Principal::of(orch_core::UserId::new("alice@example.com"))
        }
    }

    fn problem(principal: &Principal, envelope: Option<&str>) -> Option<Problem> {
        let list = vec!["desktop".to_owned()];
        check(principal, &list, envelope).err()
    }

    #[test]
    fn a_signing_clients_envelope_is_presence_checked_and_passed_on() {
        let client = signing(Some("desktop"));
        assert!(problem(&client, Some(ENVELOPE)).is_none());
        // the token-with-no-client sentinel fails closed, as in `requires_signature`
        assert!(problem(&signing(Some("")), Some(ENVELOPE)).is_none());
        // a client that is not listed is never checked
        assert!(problem(&signing(Some("web-client")), None).is_none());
        // a credential that names no client at all is never checked
        assert!(problem(&signing(None), None).is_none());
    }

    #[test]
    fn a_signing_clients_missing_or_malformed_envelope_is_the_contracts_403() {
        let client = signing(Some("desktop"));
        for envelope in [None, Some(""), Some("a.b"), Some("a+b.c.d")] {
            let p = problem(&client, envelope).unwrap();
            assert_eq!(p.status, 403);
            assert_eq!(p.code.as_deref(), Some("prompt_signature_invalid"));
            assert_eq!(
                p.ext.as_ref().and_then(|ext| ext.reason.as_deref()),
                Some("invalid")
            );
            // the envelope is in no answer
            let detail = p.detail.as_deref().unwrap_or_default();
            if let Some(envelope) = envelope {
                assert!(!detail.contains(envelope));
            }
        }
    }

    #[test]
    fn a_deployment_that_lists_no_signing_client_asks_nobody() {
        let client = signing(Some("desktop"));
        assert!(check(&client, &[], None).is_ok());
    }
}
