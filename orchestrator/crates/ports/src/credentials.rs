//! The reference a definition keeps to the credential of a person's MCP server (ADR 0046).
//!
//! A definition ([`UserToolServer`](crate::UserToolServer)) holds no secret. It holds a
//! [`CredentialRef`]: an opaque id the credential broker gave when it stored the credential, which
//! tells a reader that there is one and lets the broker find it. Knowing a reference opens nothing:
//! a credential is read only by the broker, for an owner and a server, and leased for seconds.

use std::fmt;

use crate::user_tools::DefinitionError;

/// The longest [`CredentialRef`], in bytes.
pub const MAX_CREDENTIAL_REF_BYTES: usize = 128;

/// An opaque id of a stored credential, as the broker made it: 1 to 128 bytes of visible ASCII.
///
/// It is not a secret (it names a credential, it does not open one) and it is not the key of the
/// credential: the broker keys what it stores by owner and server. Nothing outside the broker reads
/// anything into its text.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CredentialRef(String);

impl CredentialRef {
    /// Wraps the id a broker gave.
    ///
    /// # Errors
    /// [`DefinitionError::BadCredentialRef`] when `text` is empty, longer than
    /// [`MAX_CREDENTIAL_REF_BYTES`] or not visible ASCII (what a stored row that was altered would
    /// look like).
    pub fn new(text: &str) -> Result<Self, DefinitionError> {
        if text.is_empty()
            || text.len() > MAX_CREDENTIAL_REF_BYTES
            || !text.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(DefinitionError::BadCredentialRef);
        }
        Ok(CredentialRef(text.to_owned()))
    }

    /// The id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CredentialRef({})", self.0)
    }
}

impl fmt::Display for CredentialRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
