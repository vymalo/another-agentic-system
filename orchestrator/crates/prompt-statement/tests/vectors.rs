//! The shared prompt-signature vectors (`tests/vectors/prompt-signatures.json`, copied from the vectors of the signature
//! contract's cloud section) against this crate: the hashes of V3a and V3b, and **the statement built for V2's input is
//! V2's payload, byte for byte**, with the protected header and the signing input V2 was signed over.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use orch_prompt_statement::{
    A2aIds, JTI_LEN, Received, STATEMENT_TYP, Source, Statement, UserClaims, canonical, hash_of,
    jws, props_hash, statement_exp,
};
use serde_json::Value;

fn vectors() -> Value {
    serde_json::from_str(include_str!("vectors/prompt-signatures.json")).unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap()
}

/// The AG-UI request of V3a: the person's message as the run route received it.
fn v3a_received(v: &Value) -> Received<'_> {
    let req = &v["v3a_request"];
    Received {
        thread: s(&req["thread"]),
        agent: s(&req["agent"]),
        msg: s(&req["msg"]),
        content: &v["v3a"]["content"],
        forwarded_props: Some(&v["v3a"]["forwardedProps"]),
    }
}

/// The statement for V2's input: the members the orchestrator knows, and `user` bound to V3a's request.
fn v2_statement(v: &Value) -> Statement {
    let p = &v["v2"]["payload"];
    let received = v3a_received(v);
    // the A2A parts as the orchestrator sends them to the agent: one text part per text
    let parts = serde_json::json!([{ "text": s(&v["v3a"]["content"]) }]);
    let iat = p["iat"].as_u64().unwrap();
    Statement {
        aud: s(&p["aud"]).to_owned(),
        iat,
        exp: statement_exp(iat, v["v2_leaf_validity"]["not_after"].as_u64().unwrap()),
        jti: [1; JTI_LEN],
        src: Source::Chat,
        sub: s(&p["sub"]).to_owned(),
        azp: s(&p["azp"]).to_owned(),
        thread: s(&p["thread"]).to_owned(),
        agent: s(&p["agent"]).to_owned(),
        a2a: A2aIds {
            context_id: s(&p["a2a"]["contextId"]).to_owned(),
            message_id: s(&p["a2a"]["messageId"]).to_owned(),
        },
        content_sha256: hash_of(&parts),
        user: Some(UserClaims::from_request(
            s(&v["v3a_request"]["aud"]),
            &received,
            s(&v["v3a"]["envelope"]),
        )),
    }
}

#[test]
fn the_content_of_v3a_and_v3b_hashes_as_the_vectors_say() {
    let v = vectors();
    for id in ["v3a", "v3b"] {
        let content = &v[id]["content"];
        assert_eq!(
            String::from_utf8(canonical(content)).unwrap(),
            s(&v[id]["content_jcs"]),
            "{id}"
        );
        assert_eq!(hash_of(content), s(&v[id]["content_sha256"]), "{id}");
    }
}

#[test]
fn absent_forwarded_props_hash_as_an_empty_object() {
    let v = vectors();
    assert_eq!(s(&v["v3a"]["props_jcs"]), "{}");
    let expected = s(&v["v3a"]["props_sha256"]);
    assert_eq!(props_hash(None), expected);
    assert_eq!(
        props_hash(Some(&Value::Null)),
        expected,
        "JSON null is none"
    );
    assert_eq!(props_hash(Some(&serde_json::json!({}))), expected);
}

#[test]
fn the_user_member_of_v3a_is_the_vectors() {
    let v = vectors();
    let user = UserClaims::from_request(
        s(&v["v3a_request"]["aud"]),
        &v3a_received(&v),
        s(&v["v3a"]["envelope"]),
    );
    let expected = &v["v3a"]["statement_user"];
    assert_eq!(user.aud, s(&expected["aud"]));
    assert_eq!(user.thread, s(&expected["thread"]));
    assert_eq!(user.agent, s(&expected["agent"]));
    assert_eq!(user.msg, s(&expected["msg"]));
    assert_eq!(user.content_sha256, s(&expected["content_sha256"]));
    assert_eq!(user.props_sha256, s(&expected["props_sha256"]));
    assert_eq!(user.env_sha256, s(&expected["env_sha256"]));
    assert_eq!(user.env_sha256, s(&v["v3a"]["env_sha256"]));
}

#[test]
fn the_statement_for_v2s_input_is_v2s_payload_byte_for_byte() {
    let v = vectors();
    let attested = v2_statement(&v).attest(Some(&v3a_received(&v))).unwrap();
    assert_eq!(
        std::str::from_utf8(attested.as_bytes()).unwrap(),
        s(&v["v2"]["payload_jcs"])
    );
    // and V2's payload as a value is what canonical JSON of it gives: the vector's own JCS agrees with this crate's
    assert_eq!(
        String::from_utf8(canonical(&v["v2"]["payload"])).unwrap(),
        s(&v["v2"]["payload_jcs"])
    );
}

#[test]
fn v2s_exp_is_a_day_after_its_iat_because_the_leaf_lives_longer() {
    let v = vectors();
    let p = &v["v2"]["payload"];
    assert_eq!(
        p["exp"].as_u64().unwrap() - p["iat"].as_u64().unwrap(),
        24 * 3600
    );
    assert_eq!(v2_statement(&v).exp, p["exp"].as_u64().unwrap());
    // a leaf that ends sooner ends the statement with it
    assert_eq!(statement_exp(1_000, 2_000), 2_000);
    assert_eq!(statement_exp(1_000, 1_000_000), 1_000 + 24 * 3600);
}

#[test]
fn the_protected_header_and_signing_input_are_the_ones_v2_was_signed_over() {
    let v = vectors();
    let x5c: Vec<Vec<u8>> = v["v2"]["protected_header"]["x5c"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| STANDARD.decode(s(c)).unwrap())
        .collect();
    let header = jws::protected_header(&x5c);
    assert_eq!(header, canonical(&v["v2"]["protected_header"]));
    let header_value: Value = serde_json::from_slice(&header).unwrap();
    assert_eq!(header_value["typ"], STATEMENT_TYP);

    let attested = v2_statement(&v).attest(Some(&v3a_received(&v))).unwrap();
    let input = jws::signing_input(&header, &attested);
    let statement = s(&v["v2"]["statement"]);
    let (signed_part, signature) = statement.rsplit_once('.').unwrap();
    assert_eq!(input, signed_part);
    assert_eq!(
        jws::compact(&input, &URL_SAFE_NO_PAD.decode(signature).unwrap()),
        statement
    );
}

#[test]
fn v3cs_content_and_forwarded_props_hash_as_the_vectors_say() {
    let v = vectors();
    let c = &v["v3c"];
    assert_eq!(hash_of(&c["content"]), s(&c["content_sha256"]));
    assert_eq!(
        String::from_utf8(canonical(&c["forwardedProps"])).unwrap(),
        s(&c["props_jcs"])
    );
    assert_eq!(
        props_hash(Some(&c["forwardedProps"])),
        s(&c["props_sha256"])
    );
}
