//! The fixtures: the orchestrator leaf's test-only key and chain of the shared prompt-signature vectors.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_prompt_signer_static::StaticPromptSigner;

pub const KEY_PEM: &str = include_str!("../vectors/orchestrator.test-only.key.pem");
pub const CHAIN_PEM: &str = include_str!("../vectors/chain.test-only.pem");

pub fn signer() -> StaticPromptSigner {
    StaticPromptSigner::from_pem(KEY_PEM, CHAIN_PEM).expect("the test key and chain load")
}

pub async fn make() -> Option<StaticPromptSigner> {
    Some(signer())
}

pub async fn take_down(signer: &StaticPromptSigner) {
    signer.set_unavailable(true);
}
