//! **V2 of the shared prompt-signature vectors, byte for byte**: the statement built for V2's input
//! (`orch-prompt-statement`'s `tests/vectors/prompt-signatures.json`) and signed by this adapter with V1's orchestrator
//! leaf key and chain is V2's compact JWS. Ed25519 is deterministic, so the bytes are exact.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_ports::{PromptSigner, SignerState};
use orch_prompt_statement::{
    A2aIds, JTI_LEN, Received, Source, Statement, UserClaims, hash_of, statement_exp,
};
use serde_json::{Value, json};

fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../prompt-statement/tests/vectors/prompt-signatures.json"
    ))
    .unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

#[tokio::test]
async fn the_statement_for_v2s_input_signed_with_v1s_leaf_is_v2() {
    let v = vectors();
    let p = &v["v2"]["payload"];
    let req = &v["v3a_request"];
    let received = Received {
        thread: s(&req["thread"]),
        agent: s(&req["agent"]),
        msg: s(&req["msg"]),
        content: &v["v3a"]["content"],
        forwarded_props: Some(&v["v3a"]["forwardedProps"]),
    };
    let iat = p["iat"].as_u64().unwrap();
    let statement = Statement {
        aud: s(&p["aud"]).into(),
        iat,
        exp: statement_exp(iat, v["v2_leaf_validity"]["not_after"].as_u64().unwrap()),
        jti: [1; JTI_LEN],
        src: Source::Chat,
        sub: s(&p["sub"]).into(),
        azp: s(&p["azp"]).into(),
        thread: s(&p["thread"]).into(),
        agent: s(&p["agent"]).into(),
        a2a: A2aIds {
            context_id: s(&p["a2a"]["contextId"]).into(),
            message_id: s(&p["a2a"]["messageId"]).into(),
        },
        content_sha256: hash_of(&json!([{ "text": s(&v["v3a"]["content"]) }])),
        user: Some(UserClaims::from_request(
            s(&req["aud"]),
            &received,
            s(&v["v3a"]["envelope"]),
        )),
    };
    let payload = statement.attest(Some(&received)).unwrap();

    let signer = support::signer();
    assert_eq!(signer.state(), SignerState::Ready);
    let signed = signer.sign(&payload).await.unwrap();
    assert_eq!(signed.as_str(), s(&v["v2"]["statement"]));
    assert_eq!(signer.leaf_serial(), "1001");
}
