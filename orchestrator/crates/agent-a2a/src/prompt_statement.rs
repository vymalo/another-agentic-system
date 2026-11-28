//! `prompt-statement/v1` (ADR 0087, the cloud path; adam-rs ADR 0077): the orchestrator builds and
//! signs a statement of its own for a message, and sends the person's envelope beside it.
//!
//! The statement is built **at send time**, from the parts this adapter sends, because only the
//! adapter holds them: `content_sha256` is the hash of the A2A parts exactly as they go on the wire,
//! `a2a.contextId` and `a2a.messageId` are the message's own, and `agent` is the agent as resolved.
//! The members only the sender knows (`sub`, `azp`, `thread`, `src`) come from the
//! [`PromptRequest`] the dispatcher fills; `aud` is the deployment's model-gateway origin.
//!
//! The entry goes in the message's metadata under [`PROMPT_STATEMENT_EXTENSION`] **only when the
//! live card lists the exact URI** (ADR 0008: read for this very call, never remembered, fail
//! closed). The URI is activated on the request and in the message's own `extensions`.
//!
//! **`user` is `null` today.** The statement's `user` member holds the members of the person's
//! signed payload, which only the AG-UI request has; they are not carried to the send path yet, so
//! the statement does not claim them. The person's envelope still rides beside it (as `user` in the
//! metadata, which is adam's name for the envelope), byte for byte, for a verifier to check on its
//! own. A statement that named the A2A parts as the person's payload would bind the envelope to the
//! wrong bytes, which is worse than naming none. `docs/api/prompt-statement-v1.md` (ANR-903) and
//! the `server.publicOrigin` of ADR 0087's P2 are where the claims come from.

use std::future::Future;
use std::pin::Pin;

use a2a::{AgentCard, Message};
use orch_core::PROMPT_STATEMENT_EXTENSION;
use orch_ports::{
    AgentError, AttestedPayload, PromptRequest, PromptSigner, SignedStatement, SignerError,
};
use orch_prompt_statement::{A2aIds, JTI_LEN, Statement, hash_of, statement_exp};
use serde_json::{Map, Value};

/// The future a signer's `sign` gives, boxed so that a signer can be a trait object.
pub(crate) type SigningFuture<'a> =
    Pin<Box<dyn Future<Output = Result<SignedStatement, SignerError>> + Send + 'a>>;

/// The signer as this adapter holds it. `PromptSigner::sign` returns `impl Future`, so the trait
/// cannot be a trait object; this one returns a boxed future, and the blanket implementation gives
/// every [`PromptSigner`] one. The composition therefore keeps its signer behind a `dyn` handle
/// (ADR 0009) without a change to the port.
pub(crate) trait Sign: Send + Sync + 'static {
    /// The compact JWS of `payload`, as [`PromptSigner::sign`] gives it.
    fn sign<'a>(&'a self, payload: &'a AttestedPayload) -> SigningFuture<'a>;
}

impl<T: PromptSigner> Sign for T {
    fn sign<'a>(&'a self, payload: &'a AttestedPayload) -> SigningFuture<'a> {
        Box::pin(PromptSigner::sign(self, payload))
    }
}

/// Whether the card, read for this very call, lists `prompt-statement/v1` by its exact URI.
pub(crate) fn offered(card: &AgentCard) -> bool {
    card.capabilities
        .extensions
        .iter()
        .flatten()
        .any(|e| e.uri == PROMPT_STATEMENT_EXTENSION)
}

/// Builds and signs the statement of `message` for `prompt`, and answers the metadata entry:
/// `{statement}` alone, or `{statement, user}` when the person's envelope rides with it.
///
/// # Errors
/// [`AgentError::NotSent`] when the statement cannot be built or signed: a message that should
/// carry one is never sent without it. The detail holds no key, token, URL or envelope.
pub(crate) async fn entry(
    signer: &dyn Sign,
    aud: &str,
    prompt: &PromptRequest,
    agent: &str,
    message: &Message,
    envelope: Option<&str>,
) -> Result<Value, AgentError> {
    let parts = serde_json::to_value(&message.parts)
        .map_err(|e| AgentError::protocol("the message's parts cannot be read").with_source(e))?;
    let iat = u64::try_from(jiff::Timestamp::now().as_second()).unwrap_or(0);
    let mut jti = [0u8; JTI_LEN];
    getrandom::fill(&mut jti).map_err(|_| AgentError::NotSent {
        detail: "no randomness for a prompt statement".to_owned(),
    })?;
    let statement = Statement {
        aud: aud.to_owned(),
        iat,
        // The leaf's `notAfter` is the signer's to know and the verifier's to check; the statement
        // never lives longer than a day (`statement_exp`'s own bound).
        exp: statement_exp(iat, u64::MAX),
        jti,
        src: prompt.src,
        sub: prompt.sub.clone(),
        azp: prompt.azp.clone(),
        thread: prompt.thread.clone(),
        agent: agent.to_owned(),
        a2a: A2aIds {
            // a first message of a conversation names no context: the agent assigns one, and the
            // statement says what the message carried
            context_id: message.context_id.clone().unwrap_or_default(),
            message_id: message.message_id.clone(),
        },
        content_sha256: hash_of(&parts),
        // See the module's note: the person's claims are not carried to the send path yet.
        user: None,
    };
    let payload = statement.attest(None).map_err(|e| AgentError::NotSent {
        detail: format!("the prompt statement is not valid ({})", e.log_reason()),
    })?;
    let signed = signer
        .sign(&payload)
        .await
        .map_err(|e| AgentError::NotSent {
            detail: format!("the prompt statement could not be signed ({e})"),
        })?;
    let mut entry = Map::new();
    entry.insert("statement".to_owned(), Value::String(signed.into_string()));
    if let Some(envelope) = envelope {
        entry.insert("user".to_owned(), Value::String(envelope.to_owned()));
    }
    Ok(Value::Object(entry))
}
