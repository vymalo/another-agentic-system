//! Only the identity layer answers 401 (ADR 0033; web/README.md "Signing in again").
//!
//! The web sends a request again after a 401, a run's POST included, because a 401 means no handler
//! ran: `require_identity` (`src/auth.rs`) wraps every route and refuses before a handler is reached,
//! and an agent's own 401 reaches the person as a 502 (`src/problem.rs`). A handler that answered 401
//! after doing work would make that resend a second run. This test reads the sources of the crates
//! that hold the person's routes and fails when any file but those two names the status.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::fs;
use std::path::{Path, PathBuf};

/// The sources of the crates whose routes the web calls: the resource API and the AG-UI surface.
const CRATES: [&str; 2] = ["src", "../surface-agui/src"];
/// The only files that may name it: the layer that answers it, and the problem type it is built with.
const ALLOWED: [&str; 2] = ["auth.rs", "problem.rs"];
/// How a 401 is written in this code base.
const FORBIDDEN: [&str; 4] = [
    "UNAUTHORIZED",
    "Problem::unauthorized",
    "from_u16(401",
    "status(401",
];

fn sources(dir: &Path, into: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, into);
        } else if path.extension().is_some_and(|e| e == "rs") {
            into.push(path);
        }
    }
}

#[test]
fn only_the_identity_layer_answers_401() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in CRATES {
        sources(&root.join(dir), &mut files);
    }
    assert!(files.len() > 5, "the sources were not found: {files:?}");
    let mut offenders = Vec::new();
    let mut identity_names_it = false;
    for file in &files {
        let name = file.file_name().unwrap().to_string_lossy();
        let text = fs::read_to_string(file).unwrap();
        let names_it = FORBIDDEN.iter().any(|token| text.contains(token));
        if ALLOWED.contains(&name.as_ref()) {
            identity_names_it |= name == "auth.rs" && names_it;
        } else if names_it {
            offenders.push(file.display().to_string());
        }
    }
    // the test bites: the layer it protects is where it looks
    assert!(
        identity_names_it,
        "auth.rs no longer answers 401 the way this test reads it"
    );
    assert!(
        offenders.is_empty(),
        "a handler must never answer 401 (the web sends a request again after one): {offenders:?}"
    );
}
