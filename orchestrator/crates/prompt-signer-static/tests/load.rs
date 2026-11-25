//! Reading the key and the chain: what is accepted, what is refused (without quoting the text), and what `Debug` shows.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::path::Path;

use orch_ports::{PromptSigner, SignerState};
use orch_prompt_signer_static::{StaticPromptSigner, StaticSignerError};
use support::{CHAIN_PEM, KEY_PEM};

/// The base64 line of the test key's block: what must never be shown.
fn key_line() -> &'static str {
    KEY_PEM.lines().nth(2).unwrap()
}

#[test]
fn the_files_load_as_the_text_does() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors");
    let signer = StaticPromptSigner::from_files(
        &dir.join("orchestrator.test-only.key.pem"),
        &dir.join("chain.test-only.pem"),
    )
    .unwrap();
    assert_eq!(signer.leaf_serial(), "1001");
    assert_eq!(signer.state(), SignerState::Ready);
}

#[test]
fn a_missing_file_is_unreadable_and_names_no_path() {
    let error = StaticPromptSigner::from_files(
        Path::new("/nonexistent/key.pem"),
        Path::new("/nonexistent/chain.pem"),
    )
    .unwrap_err();
    assert_eq!(error, StaticSignerError::Unreadable("key"));
    assert!(!error.to_string().contains("nonexistent"));
}

#[test]
fn what_is_not_a_key_or_a_chain_is_refused() {
    assert_eq!(
        StaticPromptSigner::from_pem("no key here", CHAIN_PEM).unwrap_err(),
        StaticSignerError::NotOneKey
    );
    let two = format!("{KEY_PEM}\n{KEY_PEM}");
    assert_eq!(
        StaticPromptSigner::from_pem(&two, CHAIN_PEM).unwrap_err(),
        StaticSignerError::NotOneKey
    );
    let mangled = KEY_PEM.replace(key_line(), "not*base64");
    let error = StaticPromptSigner::from_pem(&mangled, CHAIN_PEM).unwrap_err();
    assert_eq!(error, StaticSignerError::NotBase64);
    let not_ed25519 = KEY_PEM.replace(key_line(), "AAAA");
    assert_eq!(
        StaticPromptSigner::from_pem(&not_ed25519, CHAIN_PEM).unwrap_err(),
        StaticSignerError::NotEd25519
    );
    assert_eq!(
        StaticPromptSigner::from_pem(KEY_PEM, "TEST ONLY\n").unwrap_err(),
        StaticSignerError::NoCertificate
    );
}

#[test]
fn a_chain_whose_leaf_certifies_another_key_is_refused() {
    // the intermediate first: it certifies the intermediate's key, not the leaf's
    let blocks: Vec<&str> = CHAIN_PEM
        .split("-----BEGIN CERTIFICATE-----")
        .skip(1)
        .collect();
    let swapped = format!(
        "-----BEGIN CERTIFICATE-----{}-----BEGIN CERTIFICATE-----{}",
        blocks[1], blocks[0]
    );
    assert_eq!(
        StaticPromptSigner::from_pem(KEY_PEM, &swapped).unwrap_err(),
        StaticSignerError::KeyDoesNotMatchLeaf
    );
}

#[test]
fn no_error_or_debug_text_shows_the_key() {
    let signer = support::signer();
    let debug = format!("{signer:?} {signer:#?}");
    assert!(debug.contains("1001"), "{debug}");
    assert!(!debug.contains(key_line()), "{debug}");
    let mangled = KEY_PEM.replace(key_line(), &format!("{}!", key_line()));
    let error = StaticPromptSigner::from_pem(&mangled, CHAIN_PEM).unwrap_err();
    assert!(!format!("{error} {error:?}").contains(key_line()));
}

#[tokio::test]
async fn switched_off_and_on_again() {
    let signer = support::signer();
    signer.set_unavailable(true);
    assert_eq!(signer.state(), SignerState::Unavailable);
    signer.set_unavailable(false);
    assert_eq!(signer.state(), SignerState::Ready);
    assert_eq!(signer.signatures(), 0);
}
