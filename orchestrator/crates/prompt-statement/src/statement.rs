//! The statement's members, the integrity rule, and the payload that may be signed.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};

use crate::jcs::{canonical, hash_of, props_hash, sha256_b64u};

/// The statement's `typ`, in its payload and in its protected header. *Proposed* by the contract's cloud section until
/// the signature contract fixes it.
pub const STATEMENT_TYP: &str = "prompt-statement/v1";

/// The `typ` of the person's envelope (ADR 0087, decision 2): what a signing app signs, and what this crate never
/// verifies.
pub const PERSON_TYP: &str = "prompt/v1";

/// The longest a statement lives: 24 hours, adam's park limit, so that a certificate renewed with 24 hours left covers
/// every statement it signed.
pub const MAX_LIFETIME_SECS: u64 = 24 * 3600;

/// The length of a `jti`: 16 random bytes.
pub const JTI_LEN: usize = 16;

/// The statement's `exp`: `min(iat + 24 h, the leaf's notAfter)`. Times are whole seconds since the Unix epoch.
pub fn statement_exp(iat: u64, leaf_not_after: u64) -> u64 {
    iat.saturating_add(MAX_LIFETIME_SECS).min(leaf_not_after)
}

/// What started the run the statement is for: the contract's `src`. Closed on purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// A person's message on the run route.
    Chat,
    /// A person's answer that resumes a run (ADR 0087, decision 3).
    Resume,
    /// A schedule's firing: no person's envelope.
    Schedule,
    /// A webhook: no person's envelope.
    Webhook,
    /// The orchestrator's MCP surface.
    Mcp,
}

impl Source {
    /// The name the payload carries.
    pub const fn as_str(self) -> &'static str {
        match self {
            Source::Chat => "chat",
            Source::Resume => "resume",
            Source::Schedule => "schedule",
            Source::Webhook => "webhook",
            Source::Mcp => "mcp",
        }
    }
}

/// The A2A ids of the message the statement goes with: the payload's `a2a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A2aIds {
    /// `contextId`.
    pub context_id: String,
    /// `messageId`.
    pub message_id: String,
}

/// The person's message as the AG-UI run route received it: what the integrity rule compares `user` with.
#[derive(Debug, Clone, Copy)]
pub struct Received<'a> {
    /// The thread id.
    pub thread: &'a str,
    /// The `agentId` path segment as sent, percent-decoded once (not the agent an alias resolves to).
    pub agent: &'a str,
    /// The AG-UI message id.
    pub msg: &'a str,
    /// The message's `content` member exactly as received: a string or an array, every part included.
    pub content: &'a Value,
    /// The run's `forwardedProps`; `None` or JSON `null` when it has none.
    pub forwarded_props: Option<&'a Value>,
}

/// The payload's `user`: the hash of the person's envelope and the members of the person's payload that only the chat
/// request holds, so that the verifier behind the gateway can rebuild what the person signed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserClaims {
    /// `base64url(SHA-256)` of the envelope's exact bytes.
    pub env_sha256: String,
    /// The app-facing origin the person signed for.
    pub aud: String,
    /// The thread id.
    pub thread: String,
    /// The agent as sent.
    pub agent: String,
    /// The AG-UI message id.
    pub msg: String,
    /// `base64url(SHA-256(JCS(content)))`.
    pub content_sha256: String,
    /// `base64url(SHA-256(JCS(forwardedProps or {})))`.
    pub props_sha256: String,
}

impl UserClaims {
    /// The `user` of a request: every member taken from it, `aud` from the deployment's configuration and `env_sha256`
    /// from the envelope's exact bytes (the value of `vymalo.promptSignature`). It reads the envelope as bytes and
    /// nothing more.
    pub fn from_request(aud: &str, received: &Received<'_>, envelope: &str) -> Self {
        UserClaims {
            env_sha256: sha256_b64u(envelope.as_bytes()),
            aud: aud.to_owned(),
            thread: received.thread.to_owned(),
            agent: received.agent.to_owned(),
            msg: received.msg.to_owned(),
            content_sha256: hash_of(received.content),
            props_sha256: props_hash(received.forwarded_props),
        }
    }

    /// The integrity rule: `thread`, `agent`, `msg`, `content_sha256` and `props_sha256` are what the request holds,
    /// the two hashes recomputed from it.
    ///
    /// # Errors
    /// [`StatementError::Mismatch`] naming the first member that differs.
    pub fn check_against(&self, received: &Received<'_>) -> Result<(), StatementError> {
        let checks = [
            (Field::Thread, self.thread == received.thread),
            (Field::Agent, self.agent == received.agent),
            (Field::Msg, self.msg == received.msg),
            (
                Field::ContentSha256,
                self.content_sha256 == hash_of(received.content),
            ),
            (
                Field::PropsSha256,
                self.props_sha256 == props_hash(received.forwarded_props),
            ),
        ];
        match checks.into_iter().find(|(_, same)| !same) {
            Some((field, _)) => Err(StatementError::Mismatch(field)),
            None => Ok(()),
        }
    }

    fn to_value(&self) -> Value {
        json!({
            "env_sha256": self.env_sha256,
            "aud": self.aud,
            "thread": self.thread,
            "agent": self.agent,
            "msg": self.msg,
            "content_sha256": self.content_sha256,
            "props_sha256": self.props_sha256,
        })
    }
}

/// The member of `user` the integrity rule found different from the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// `thread`.
    Thread,
    /// `agent`.
    Agent,
    /// `msg`.
    Msg,
    /// `content_sha256`.
    ContentSha256,
    /// `props_sha256`.
    PropsSha256,
}

impl Field {
    /// The member's name in the payload.
    pub const fn as_str(self) -> &'static str {
        match self {
            Field::Thread => "thread",
            Field::Agent => "agent",
            Field::Msg => "msg",
            Field::ContentSha256 => "content_sha256",
            Field::PropsSha256 => "props_sha256",
        }
    }
}

/// Why a statement was not attested. Nothing is signed then. No variant holds a value of the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StatementError {
    /// What would go into `user` is not what the request held: ADR 0087's `403 prompt_signature_invalid`, reason
    /// `invalid`.
    #[error("the person's prompt does not match the request it came with ({})", .0.as_str())]
    Mismatch(Field),
    /// `user` was given with no request to compare it with: refused the same way.
    #[error("the person's prompt was given without the request it came with")]
    Unbound,
    /// `exp` is not after `iat`, or more than [`MAX_LIFETIME_SECS`] after it: the orchestrator's own mistake, not the
    /// person's.
    #[error("a statement lives more than zero seconds and at most a day")]
    Lifetime,
}

impl StatementError {
    /// Whether the person's request is refused with ADR 0087's `403 prompt_signature_invalid`.
    pub const fn is_prompt_signature_invalid(&self) -> bool {
        matches!(self, StatementError::Mismatch(_) | StatementError::Unbound)
    }

    /// The problem's `code` for the client, when the request is refused: `prompt_signature_invalid`.
    pub const fn code(&self) -> Option<&'static str> {
        if self.is_prompt_signature_invalid() {
            Some("prompt_signature_invalid")
        } else {
            None
        }
    }

    /// The `reason` the client sees, when the request is refused: `invalid` (ADR 0087, decision 7).
    pub const fn client_reason(&self) -> Option<&'static str> {
        if self.is_prompt_signature_invalid() {
            Some("invalid")
        } else {
            None
        }
    }

    /// The finer reason, for the orchestrator's log and never the response.
    pub const fn log_reason(&self) -> &'static str {
        match self {
            StatementError::Mismatch(_) | StatementError::Unbound => "mismatch",
            StatementError::Lifetime => "lifetime",
        }
    }
}

/// The members of a prompt statement (the contract's cloud section, decision 2). Times are whole seconds since the Unix
/// epoch; the caller reads the clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// The model gateway's canonical origin.
    pub aud: String,
    /// When the orchestrator signs.
    pub iat: u64,
    /// [`statement_exp`] of `iat` and the leaf's `notAfter`.
    pub exp: u64,
    /// 16 random bytes; written base64url.
    pub jti: [u8; JTI_LEN],
    /// What started the run.
    pub src: Source,
    /// The person.
    pub sub: String,
    /// The token's client (`azp`, or `client_id` when there is none).
    pub azp: String,
    /// The thread id.
    pub thread: String,
    /// The agent as resolved.
    pub agent: String,
    /// The A2A ids of the message.
    pub a2a: A2aIds,
    /// `base64url(SHA-256(JCS(the A2A parts as sent)))`.
    pub content_sha256: String,
    /// The person's side, or `None` when there is no envelope (a web client, a schedule, a webhook).
    pub user: Option<UserClaims>,
}

impl Statement {
    /// Checks the statement and gives the payload that may be signed: the RFC 8785 JSON of its members with `typ`.
    ///
    /// With a `user`, the integrity rule runs against `received` ([`UserClaims::check_against`]), which must be
    /// there. Without one, `received` is not looked at. **It never verifies the envelope's signature.**
    ///
    /// # Errors
    /// [`StatementError`]; nothing may be signed then.
    pub fn attest(
        &self,
        received: Option<&Received<'_>>,
    ) -> Result<AttestedPayload, StatementError> {
        if self.exp <= self.iat || self.exp - self.iat > MAX_LIFETIME_SECS {
            return Err(StatementError::Lifetime);
        }
        if let Some(user) = &self.user {
            user.check_against(received.ok_or(StatementError::Unbound)?)?;
        }
        let value = json!({
            "typ": STATEMENT_TYP,
            "aud": self.aud,
            "iat": self.iat,
            "exp": self.exp,
            "jti": URL_SAFE_NO_PAD.encode(self.jti),
            "src": self.src.as_str(),
            "sub": self.sub,
            "azp": self.azp,
            "thread": self.thread,
            "agent": self.agent,
            "a2a": { "contextId": self.a2a.context_id, "messageId": self.a2a.message_id },
            "content_sha256": self.content_sha256,
            "user": self.user.as_ref().map_or(Value::Null, UserClaims::to_value),
        });
        Ok(AttestedPayload(canonical(&value)))
    }
}

/// A statement's payload that passed [`Statement::attest`]: the only thing a prompt signer signs. It cannot be made any
/// other way, so a statement the integrity rule refused cannot reach a signer.
///
/// `Debug` shows the length: the payload names the person.
#[derive(Clone, PartialEq, Eq)]
pub struct AttestedPayload(Vec<u8>);

impl AttestedPayload {
    /// The canonical JSON bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for AttestedPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AttestedPayload")
            .field("len", &self.0.len())
            .finish()
    }
}
