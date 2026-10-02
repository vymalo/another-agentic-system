//! A resolver over maps, so no test reads the process environment or the file system.
#![allow(dead_code, missing_docs)]

use std::collections::HashMap;
use std::io;
use std::path::Path;

use orch_config::{ConfigError, Resolve, Validated};

#[derive(Default)]
pub struct Fake {
    pub env: HashMap<String, String>,
    pub files: HashMap<String, String>,
}

impl Fake {
    pub fn env(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.to_owned(), value.to_owned());
        self
    }

    pub fn file(mut self, path: &str, text: &str) -> Self {
        self.files.insert(path.to_owned(), text.to_owned());
        self
    }
}

impl Resolve for Fake {
    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }

    fn read_file(&self, path: &Path) -> io::Result<String> {
        self.files
            .get(path.to_str().unwrap_or_default())
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

/// A secret that is long enough for every rule: 64 bytes.
pub const LONG: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

pub fn load(text: &str, fake: &Fake) -> Result<Validated, Vec<ConfigError>> {
    orch_config::load(text, Path::new("/etc/orchestrator"), fake)
}

/// The lines of the errors, as an operator reads them.
pub fn lines(result: Result<Validated, Vec<ConfigError>>) -> Vec<String> {
    match result {
        Ok(_) => panic!("expected errors, the file is valid"),
        Err(errors) => errors.iter().map(ToString::to_string).collect(),
    }
}
