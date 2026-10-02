//! Secrets by reference: what the file holds, and what a reference resolves to.
//!
//! The file holds a [`SecretRef`] and never a value. A resolved [`Secret`] keeps its reference
//! beside its value (a `SecretString`), and every `Debug` of it, and of everything that holds
//! one, prints the reference and never the value.

use std::fmt;
use std::io;
use std::path::Path;

use secrecy::{ExposeSecret as _, SecretString};

use crate::types::SecretRef;

/// The most a `{ file }` secret may hold, 64 KiB.
pub const MAX_SECRET_FILE_BYTES: usize = 64 * 1024;

/// Where references are resolved: the environment and the file system, which the library does
/// not read itself. The binary passes the process environment and the real file system; a test
/// passes a map.
pub trait Resolve {
    /// The value of the environment variable `name`, if it is set.
    fn env(&self, name: &str) -> Option<String>;

    /// The text of the file at `path`. A reader should stop a little after
    /// [`MAX_SECRET_FILE_BYTES`]: the library refuses a longer text.
    ///
    /// # Errors
    /// Any I/O error; the library never shows its text.
    fn read_file(&self, path: &Path) -> io::Result<String>;
}

/// A resolved secret: the reference it came from, and its value.
pub struct Secret {
    reference: SecretRef,
    value: SecretString,
}

impl Secret {
    pub(crate) fn new(reference: SecretRef, value: String) -> Self {
        Secret {
            reference,
            value: SecretString::from(value),
        }
    }

    /// The reference this secret was read through.
    pub fn reference(&self) -> &SecretRef {
        &self.reference
    }

    /// The value. Handle it as a secret: never log it, never put it in an error.
    pub fn expose(&self) -> &str {
        self.value.expose_secret()
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret({:?})", self.reference)
    }
}

impl Clone for Secret {
    fn clone(&self) -> Self {
        Secret::new(self.reference.clone(), self.expose().to_owned())
    }
}
