//! The shared secrets of a webhook route: one, or two while a secret is rotated.

use std::fmt;

use secrecy::{ExposeSecret, SecretString};

/// The most secrets a route holds: the current one and the previous one.
pub const MAX_SECRETS: usize = 2;

/// The fewest bytes of a secret: an HMAC key shorter than its 32-byte tag is guessable
/// (`openssl rand -hex 32` makes 64 characters).
pub const MIN_SECRET_BYTES: usize = 32;

/// Why a list of secrets is unusable. The messages carry no secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SecretsError {
    /// The list holds no secret (it is empty, or only commas and blanks).
    #[error("names no secret")]
    Empty,
    /// A secret shorter than [`MIN_SECRET_BYTES`].
    #[error(
        "holds a secret of {len} bytes, the minimum is {MIN_SECRET_BYTES} (openssl rand -hex 32)"
    )]
    TooShort {
        /// How long the short one is.
        len: usize,
    },
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
    /// [`MAX_SECRETS`], [`SecretsError::TooShort`] for a secret under [`MIN_SECRET_BYTES`].
    pub fn parse(raw: &str) -> Result<Self, SecretsError> {
        Self::new(
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| SecretString::from(s.to_owned())),
        )
    }

    /// Secrets from values that are already separated. Each is trimmed, so what the sender must
    /// sign with is the trimmed text; a value that is empty or only blanks counts for nothing.
    ///
    /// # Errors
    /// As [`parse`](Self::parse).
    pub fn new(secrets: impl IntoIterator<Item = SecretString>) -> Result<Self, SecretsError> {
        let secrets: Vec<SecretString> = secrets
            .into_iter()
            .map(|s| SecretString::from(s.expose_secret().trim().to_owned()))
            .filter(|s| !s.expose_secret().is_empty())
            .collect();
        if secrets.is_empty() {
            return Err(SecretsError::Empty);
        }
        if secrets.len() > MAX_SECRETS {
            return Err(SecretsError::TooMany {
                count: secrets.len(),
            });
        }
        if let Some(short) = secrets
            .iter()
            .map(|s| s.expose_secret().len())
            .find(|len| *len < MIN_SECRET_BYTES)
        {
            return Err(SecretsError::TooShort { len: short });
        }
        Ok(Secrets(secrets))
    }

    /// One secret as it is, without the checks of [`new`](Self::new): for the known-answer
    /// vectors of other people's documentation, whose keys are short.
    #[cfg(test)]
    pub(crate) fn unchecked(raw: &str) -> Self {
        Secrets(vec![SecretString::from(raw.to_owned())])
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

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const C: &str = "cccccccccccccccccccccccccccccccc";

    #[test]
    fn one_or_two_secrets_are_read_trimmed() {
        assert_eq!(Secrets::parse(A).unwrap().len(), 1);
        assert_eq!(Secrets::parse(&format!(" {A} , {B} ")).unwrap().len(), 2);
        assert_eq!(Secrets::parse(&format!("{A},,{B},")).unwrap().len(), 2);
    }

    #[test]
    fn nothing_or_three_is_refused() {
        for empty in ["", " ", ",", " , ,"] {
            assert_eq!(Secrets::parse(empty).unwrap_err(), SecretsError::Empty);
        }
        assert_eq!(
            Secrets::parse(&format!("{A},{B},{C}")).unwrap_err(),
            SecretsError::TooMany { count: 3 }
        );
    }

    #[test]
    fn a_short_secret_is_refused_and_a_blank_one_is_nothing() {
        assert_eq!(
            Secrets::parse("hunter2").unwrap_err(),
            SecretsError::TooShort { len: 7 }
        );
        // The length is that of the trimmed value, and a good one beside a short one still fails.
        let padded = format!("{}{}", "x".repeat(31), " ".repeat(10));
        assert_eq!(
            Secrets::new([SecretString::from(padded)]).unwrap_err(),
            SecretsError::TooShort { len: 31 }
        );
        assert_eq!(
            Secrets::parse(&format!("{A},short")).unwrap_err(),
            SecretsError::TooShort { len: 5 }
        );
        assert_eq!(
            Secrets::new([SecretString::from("   ".to_owned())]).unwrap_err(),
            SecretsError::Empty
        );
        assert!(Secrets::parse(&"x".repeat(MIN_SECRET_BYTES)).is_ok());
        assert!(Secrets::new([SecretString::from(format!("  {A}\n"))]).is_ok());
    }

    #[test]
    fn debug_and_errors_never_show_a_secret() {
        let one = "hunter2-hunter2-hunter2-hunter2-hunter2";
        let two = "correct-horse-battery-staple-correct-horse";
        let secrets = Secrets::parse(&format!("{one},{two}")).unwrap();
        let shown = format!("{secrets:?}");
        assert!(
            !shown.contains("hunter2") && !shown.contains("correct"),
            "{shown}"
        );
        let err = Secrets::parse(&format!("{one},{two},third")).unwrap_err();
        assert!(!err.to_string().contains("hunter2"), "{err}");
        let err = Secrets::parse("hunter2").unwrap_err();
        assert!(!err.to_string().contains("hunter2"), "{err}");
    }
}
