//! The presence check of a signing client's prompt (ADR 0087, decisions 1 and 7), one for every
//! route that writes a person's words: the AG-UI run (`orch-surface-agui`), `startChildSession` and
//! `forkThread`.

use orch_core::PromptClaims;
use orch_ports::Principal;
use orch_prompt_statement::{MAX_ENVELOPE_LEN, Received, UserClaims};

use crate::Problem;

/// ADR 0087, decision 1 and 7: a prompt from a **signing client** (`auth.signingClients`) that
/// carries no envelope, or one that is not a compact JWS, is refused with `403
/// prompt_signature_invalid`, reason `invalid`, before anything is written or any agent is called.
/// No cryptography happens here (the verifier behind the model gateway checks the signature); the
/// finer reason (`missing`, `malformed`) is logged and never sent. A client that is not listed, and
/// a deployment that lists none, is never checked.
///
/// Returns the envelope **to carry**. **A caller must carry the value this returns, not the one it
/// received**, because the bound and the drop happen inside it, and a caller that passes the raw
/// value on silently skips both. An empty string is no envelope for everyone (so it never makes a
/// message count as signed). A value over [`MAX_ENVELOPE_LEN`] is a signing client's `403`; from a
/// client that is not asked for one it is dropped, since a Stop & send keeps what is carried in the
/// job ledger. Nothing else about such a client's value is looked at.
///
/// Call it only for a request that writes the person's words, before anything is written and before
/// the rate limit and the allowance: a message that is both unsigned and over the limit is the 403,
/// and nothing is counted.
///
/// # Errors
/// The `403` problem above.
pub fn check_prompt_signature<'a>(
    principal: &Principal,
    signing_clients: &[String],
    envelope: Option<&'a str>,
) -> Result<Option<&'a str>, Problem> {
    if !orch_prompt_statement::requires_signature(signing_clients, principal.client_id.as_deref()) {
        return Ok(envelope.filter(|value| !value.is_empty() && value.len() <= MAX_ENVELOPE_LEN));
    }
    match orch_prompt_statement::check_presence(envelope) {
        Ok(envelope) => Ok(Some(envelope)),
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

/// The claims kept beside a person's envelope (ADR 0087, decision 2), from the request **as it
/// arrived** ([`Received`]) and the deployment's configured public origin `aud`. The members and
/// their hashes are `orch-prompt-statement`'s [`UserClaims::from_request`], so that intake and the
/// statement cannot hash differently. `env_sha256` is not kept: it is a function of the envelope.
pub fn prompt_claims(aud: &str, received: &Received<'_>, envelope: &str) -> PromptClaims {
    let UserClaims {
        env_sha256: _,
        aud,
        thread,
        agent,
        msg,
        content_sha256,
        props_sha256,
    } = UserClaims::from_request(aud, received, envelope);
    PromptClaims {
        aud,
        thread,
        agent,
        msg,
        content_sha256,
        props_sha256,
    }
}
