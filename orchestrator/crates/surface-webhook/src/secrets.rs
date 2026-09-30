//! The shared secrets of a webhook route: one, or two while a secret is rotated.

use std::fmt;

use secrecy::{ExposeSecret, SecretString};

/// The most secrets a route holds: the current one and the previous one.
pub const MAX_SECRETS: usize = 2;

/// Why a list of secrets is unusable. The messages carry no secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SecretsError {
    /// The list holds no secret (it is empty, or only commas and blanks).
    #[error("names no secret")]
    Empty,
    /// More than [`MAX_SECRETS`].
    #[error(
        "holds {count} secrets, at most {MAX_SECRETS} are allowed (the current one and the \
         previous one, so a secret can be rotated without a gap)"
    )]
    TooMany {
        /// How many the list holds.
        count: usize,
    },
}

/// One or two shared secrets. A signature is good when it matches any of them, which is what
/// lets a secret be rotated without a gap: add the new one in front, move the sender over, drop
/// the old one.
///
/// The values are [`SecretString`]s: never shown by `Debug`, and zeroed when dropped.
#[derive(Clone)]
pub struct Secrets(Vec<SecretString>);

impl Secrets {
    /// Reads the comma-separated form of `WEBHOOK_GENERIC_SECRETS` and `WEBHOOK_GITHUB_SECRETS`.
    /// Whitespace around a secret and empty entries (`a,,b`, a trailing comma) are ignored; a
    /// secret therefore cannot contain a comma or start or end with a blank.
    ///
    /// # Errors
    /// [`SecretsError::Empty`] when nothing is left, [`SecretsError::TooMany`] above
    /// [`MAX_SECRETS`].
    pub fn parse(raw: &str) -> Result<Self, SecretsError> {
        Self::new(
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| SecretString::from(s.to_owned())),
        )
    }

    /// Secrets from values that are already separated.
    ///
    /// # Errors
    /// As [`parse`](Self::parse); an empty value counts for nothing.
    pub fn new(secrets: impl IntoIterator<Item = SecretString>) -> Result<Self, SecretsError> {
        let secrets: Vec<SecretString> = secrets
            .into_iter()
            .filter(|s| !s.expose_secret().is_empty())
            .collect();
        match secrets.len() {
            0 => Err(SecretsError::Empty),
            n if n > MAX_SECRETS => Err(SecretsError::TooMany { count: n }),
            _ => Ok(Secrets(secrets)),
        }
    }

    /// How many secrets there are: 1 or 2.
    #[allow(clippy::len_without_is_empty)] // never empty by construction
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The secrets' bytes, for the signature check and nothing else.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &[u8]> {
        self.0.iter().map(|s| s.expose_secret().as_bytes())
    }
}

impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secrets(<{} redacted>)", self.0.len())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn one_or_two_secrets_are_read_trimmed() {
        assert_eq!(Secrets::parse("a").unwrap().len(), 1);
        assert_eq!(Secrets::parse(" a , b ").unwrap().len(), 2);
        assert_eq!(Secrets::parse("a,,b,").unwrap().len(), 2);
    }

    #[test]
    fn nothing_or_three_is_refused() {
        for empty in ["", " ", ",", " , ,"] {
            assert_eq!(Secrets::parse(empty).unwrap_err(), SecretsError::Empty);
        }
        assert_eq!(
            Secrets::parse("a,b,c").unwrap_err(),
            SecretsError::TooMany { count: 3 }
        );
    }

    #[test]
    fn debug_and_errors_never_show_a_secret() {
        let secrets = Secrets::parse("hunter2,correct-horse").unwrap();
        let shown = format!("{secrets:?}");
        assert!(
            !shown.contains("hunter2") && !shown.contains("correct"),
            "{shown}"
        );
        let err = Secrets::parse("hunter2,correct-horse,third").unwrap_err();
        assert!(!err.to_string().contains("hunter2"), "{err}");
    }
}
