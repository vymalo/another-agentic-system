//! The `PromptSigner` testkit against the static signer.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use support::{make, take_down};

orch_ports::prompt_signer_conformance!(make, unavailable = take_down);
