//! The testkit catches what it claims to: six signers, each wrapping the static one and breaking **one** rule of the
//! port, and each fails the case of that rule. A case that passed a broken signer would be a check that checks nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Mutex;

use orch_ports::testkit::prompt_signer as kit;
use orch_ports::{AttestedPayload, PromptSigner, SignedStatement, SignerError, SignerState};
use orch_prompt_signer_static::StaticPromptSigner;

/// The ways to break a signer, one rule each.
#[derive(Clone, Copy)]
enum Break {
    /// The payload is left out (a detached JWS).
    DetachesThePayload,
    /// The header names another `typ` (and is no longer the statement's).
    WrongTyp,
    /// One bit of the signature is flipped.
    ForgesTheSignature,
    /// The first statement is answered for every payload.
    ReplaysTheFirstStatement,
    /// It signs and says it is unavailable.
    LiesAboutItsState,
    /// It ignores being made unavailable.
    SignsWhenUnavailable,
    /// Nothing: the control.
    Nothing,
}

struct Broken {
    inner: StaticPromptSigner,
    how: Break,
    first: Mutex<Option<SignedStatement>>,
}

fn b64u(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn unb64u(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .unwrap()
}

impl PromptSigner for Broken {
    async fn sign(&self, payload: &AttestedPayload) -> Result<SignedStatement, SignerError> {
        let good = self.inner.sign(payload).await?;
        let mut parts: Vec<String> = good.as_str().split('.').map(str::to_owned).collect();
        match self.how {
            Break::DetachesThePayload => parts[1].clear(),
            Break::WrongTyp => {
                let header = String::from_utf8(unb64u(&parts[0])).unwrap();
                parts[0] = b64u(
                    header
                        .replace("prompt-statement/v1", "prompt/v1")
                        .as_bytes(),
                );
            }
            Break::ForgesTheSignature => {
                let mut signature = unb64u(&parts[2]);
                signature[0] ^= 1;
                parts[2] = b64u(&signature);
            }
            Break::ReplaysTheFirstStatement => {
                let mut first = self.first.lock().unwrap();
                return Ok(first.get_or_insert(good).clone());
            }
            Break::LiesAboutItsState | Break::SignsWhenUnavailable | Break::Nothing => {}
        }
        Ok(SignedStatement::new(parts.join(".")))
    }

    fn state(&self) -> SignerState {
        match self.how {
            // says unavailable while it signs; and, made unavailable, says so while it still signs
            Break::LiesAboutItsState | Break::SignsWhenUnavailable => SignerState::Unavailable,
            _ => self.inner.state(),
        }
    }
}

fn broken(how: Break) -> Broken {
    Broken {
        inner: support::signer(),
        how,
        first: Mutex::new(None),
    }
}

/// The `unavailable` hook that the `SignsWhenUnavailable` signer ignores: the switch is never passed on.
async fn ignored(_signer: &Broken) {}

/// The hook as a working adapter supplies it.
async fn take_down(signer: &Broken) {
    signer.inner.set_unavailable(true);
}

#[tokio::test]
#[should_panic(expected = "assertion `left == right` failed")]
async fn a_detached_payload_fails_the_attached_payload_case() {
    kit::the_payload_is_attached_byte_for_byte(broken(Break::DetachesThePayload)).await;
}

#[tokio::test]
#[should_panic(expected = "typ")]
async fn another_typ_fails_the_header_case() {
    kit::the_protected_header_is_the_statements(broken(Break::WrongTyp)).await;
}

#[tokio::test]
#[should_panic(expected = "the signature verifies under the leaf's key")]
async fn a_forged_signature_fails_the_signature_case() {
    kit::the_signature_verifies_under_the_leaf_key(broken(Break::ForgesTheSignature)).await;
}

#[tokio::test]
#[should_panic(expected = "two payloads, two statements")]
async fn a_replayed_statement_fails_the_own_statement_case() {
    kit::each_payload_gets_its_own_statement(broken(Break::ReplaysTheFirstStatement)).await;
}

#[tokio::test]
#[should_panic(expected = "a fresh signer can sign")]
async fn a_signer_that_says_unavailable_and_signs_fails_the_state_case() {
    kit::a_signer_that_signs_is_not_unavailable(broken(Break::LiesAboutItsState)).await;
}

#[tokio::test]
#[should_panic(expected = "an unavailable signer signs nothing")]
async fn a_signer_that_signs_while_unavailable_fails_the_refusal_case() {
    kit::an_unavailable_signer_refuses_and_says_so(broken(Break::SignsWhenUnavailable), ignored)
        .await;
}

/// And the wrapper with nothing broken passes every case, so each failure above is the break's.
#[tokio::test]
async fn the_wrapper_with_nothing_broken_passes_every_case() {
    kit::the_payload_is_attached_byte_for_byte(broken(Break::Nothing)).await;
    kit::the_protected_header_is_the_statements(broken(Break::Nothing)).await;
    kit::the_signature_verifies_under_the_leaf_key(broken(Break::Nothing)).await;
    kit::each_payload_gets_its_own_statement(broken(Break::Nothing)).await;
    kit::a_signer_that_signs_is_not_unavailable(broken(Break::Nothing)).await;
    kit::an_unavailable_signer_refuses_and_says_so(broken(Break::Nothing), take_down).await;
}
