//! A changed content or option is refused before anything reaches the signer: the integrity rule of
//! `orch-prompt-statement` gives no payload, and the signer counts no signature.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_ports::PromptSigner;
use orch_prompt_signer_static::StaticPromptSigner;
use orch_prompt_statement::{
    A2aIds, Field, JTI_LEN, Received, Source, Statement, StatementError, UserClaims, hash_of,
};
use serde_json::{Value, json};

fn statement(user: UserClaims) -> Statement {
    Statement {
        aud: "https://models.test.invalid".into(),
        iat: 1_795_000_000,
        exp: 1_795_003_600,
        jti: [9; JTI_LEN],
        src: Source::Chat,
        sub: "person-1".into(),
        azp: "app".into(),
        thread: "thread-1".into(),
        agent: "coder".into(),
        a2a: A2aIds {
            context_id: "ctx".into(),
            message_id: "m".into(),
        },
        content_sha256: hash_of(&json!([{ "text": "Run the tests." }])),
        user: Some(user),
    }
}

fn received<'a>(content: &'a Value, props: &'a Value) -> Received<'a> {
    Received {
        thread: "thread-1",
        agent: "adam",
        msg: "msg-1",
        content,
        forwarded_props: Some(props),
    }
}

/// What the run route will do: attest, and sign only what was attested.
async fn attest_and_sign(
    signer: &StaticPromptSigner,
    statement: &Statement,
    received: &Received<'_>,
) -> Result<String, StatementError> {
    let payload = statement.attest(Some(received))?;
    Ok(signer.sign(&payload).await.unwrap().into_string())
}

fn person_signed_for() -> (Value, Value, UserClaims) {
    let content = json!("Run the tests.");
    let props = json!({ "vymalo": { "accessTier": "read" }, "maxTurns": 3 });
    let user = UserClaims::from_request(
        "https://api.test.invalid",
        &received(&content, &props),
        "eyJhbGciOiJFZERTQSJ9..c2ln",
    );
    (content, props, user)
}

#[tokio::test]
async fn the_request_the_person_signed_for_is_signed_once() {
    let signer = support::signer();
    let (content, props, user) = person_signed_for();
    attest_and_sign(&signer, &statement(user), &received(&content, &props))
        .await
        .unwrap();
    assert_eq!(signer.signatures(), 1);
}

#[tokio::test]
async fn a_changed_content_is_refused_and_nothing_is_signed() {
    let signer = support::signer();
    let (_, props, user) = person_signed_for();
    let changed = json!("Delete the branch.");
    let error = attest_and_sign(&signer, &statement(user), &received(&changed, &props))
        .await
        .unwrap_err();
    assert_eq!(error, StatementError::Mismatch(Field::ContentSha256));
    assert_eq!(error.code(), Some("prompt_signature_invalid"));
    assert_eq!(signer.signatures(), 0);
}

#[tokio::test]
async fn a_changed_props_hash_is_refused_and_nothing_is_signed() {
    let signer = support::signer();
    let (content, props, mut user) = person_signed_for();
    user.props_sha256 = hash_of(&json!({ "vymalo": { "accessTier": "full" } }));
    let error = attest_and_sign(&signer, &statement(user), &received(&content, &props))
        .await
        .unwrap_err();
    assert_eq!(error, StatementError::Mismatch(Field::PropsSha256));
    assert_eq!(error.client_reason(), Some("invalid"));
    assert_eq!(signer.signatures(), 0);
}

#[tokio::test]
async fn a_changed_content_hash_is_refused_and_nothing_is_signed() {
    let signer = support::signer();
    let (content, props, mut user) = person_signed_for();
    user.content_sha256 = hash_of(&json!("Delete the branch."));
    let error = attest_and_sign(&signer, &statement(user), &received(&content, &props))
        .await
        .unwrap_err();
    assert_eq!(error, StatementError::Mismatch(Field::ContentSha256));
    assert_eq!(signer.signatures(), 0);
}
