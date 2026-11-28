//! The person's prompt signature (ADR 0087) as the core carries it: [`SignedPrompt`], the one type
//! of the whole path from intake to the agent's request, made of the envelope
//! ([`PromptSignature`]) and what only the request it came in could say ([`PromptClaims`]).

use std::fmt;

use serde::{Deserialize, Serialize};

/// The person's prompt signature (ADR 0087, decision 2): the compact JWS their app sent, kept
/// **byte for byte** and never decoded here. It is a credential-shaped string, so `Debug` shows its
/// **length** and never its value: every type that carries it ([`Input`](crate::Input),
/// [`Command`](crate::Command), [`Job`](crate::Job), an outbox row) derives `Debug`, and none of
/// them may put it in a log line. There is no `Display`.
///
/// It serialises as the plain string. Serialising is not redaction: whoever hands a job to a person
/// takes the [`SignedPrompt`] out first.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PromptSignature(String);

impl PromptSignature {
    /// The bytes as they were received.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The bytes as they were received, owned.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<String> for PromptSignature {
    fn from(received: String) -> Self {
        Self(received)
    }
}

impl fmt::Debug for PromptSignature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&Length(&self.0), f)
    }
}

/// A string as `Debug` shows it here: how long it is, never what it says.
struct Length<'a>(&'a str);

impl fmt::Debug for Length<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} bytes>", self.0.len())
    }
}

/// A person's signed prompt (ADR 0087): the envelope their app sent and the claims of the request
/// it came in. **One value** goes from intake through the input, the command, the outbox row, the
/// job ledger of a Stop & send and the agent's request, so that a member added later is one change.
///
/// It is not conversation content: no event, frame, step or export holds it. `Debug` shows lengths
/// and never a value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedPrompt {
    /// The compact JWS, byte for byte.
    pub envelope: PromptSignature,
    /// What a verifier needs beside the envelope to rebuild what was signed. **Optional as a
    /// whole.** A run (a message or a resumed answer, a fork made by one included) and a
    /// helper-session start fill it. A REST fork or edit has none, because the contract does not
    /// yet say what it signs, so it is signed and **not verifiable** (ADR 0087). A deployment that
    /// configures no public origin keeps none either.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims: Option<PromptClaims>,
}

impl SignedPrompt {
    /// An envelope with no claims: all that a route which cannot say what was signed may hold.
    pub fn envelope_only(received: impl Into<PromptSignature>) -> Self {
        Self {
            envelope: received.into(),
            claims: None,
        }
    }
}

/// The members of the person's signed payload that only the request holds (ADR 0087, decision 2),
/// **computed at intake**, where the request as it arrived is still in hand: none of the hashes can
/// be rebuilt from the text that is kept. The statement's `user.env_sha256` is not here: it is a
/// function of the envelope's bytes. Named as the statement names them.
///
/// The orchestrator verifies nothing with them. `Debug` shows the length of each and never a value:
/// a hash of a short prompt can be guessed.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptClaims {
    /// The origin the person signed for: the deployment's configured public origin, never taken
    /// from the request.
    pub aud: String,
    /// The thread id as the request wrote it.
    pub thread: String,
    /// The agent as sent: the `agentId` path segment, percent-decoded once, before an alias is
    /// resolved.
    pub agent: String,
    /// The AG-UI message id; for a resumed answer, the interrupt id.
    pub msg: String,
    /// `base64url(SHA-256(JCS(content)))` of the message's `content` as received; for a resumed
    /// answer, of the whole `payload`.
    pub content_sha256: String,
    /// `base64url(SHA-256(JCS(forwardedProps)))` as received, or of `{}` when there is none.
    pub props_sha256: String,
}

impl fmt::Debug for PromptClaims {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            aud,
            thread,
            agent,
            msg,
            content_sha256,
            props_sha256,
        } = self;
        f.debug_struct("PromptClaims")
            .field("aud", &Length(aud))
            .field("thread", &Length(thread))
            .field("agent", &Length(agent))
            .field("msg", &Length(msg))
            .field("content_sha256", &Length(content_sha256))
            .field("props_sha256", &Length(props_sha256))
            .finish()
    }
}
