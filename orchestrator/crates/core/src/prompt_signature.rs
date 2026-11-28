//! The person's prompt signature (ADR 0087) as the core carries it.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The person's prompt signature (ADR 0087, decision 2): the compact JWS their app sent, kept
/// **byte for byte** and never decoded here. It is a credential-shaped string, so `Debug` shows its
/// **length** and never its value: every type that carries it ([`Input`](crate::Input),
/// [`Command`](crate::Command), [`Job`](crate::Job), an outbox row) derives `Debug`, and none of
/// them may put it in a log line. There is no `Display`.
///
/// It serialises as the plain string, for the places that keep it (the outbox row, and
/// [`Job::after_stop_prompt_signature`](crate::Job::after_stop_prompt_signature)). Serialising is
/// not redaction: whoever hands a job to a person takes it out first.
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
        write!(f, "<{} bytes>", self.0.len())
    }
}
