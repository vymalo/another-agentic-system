//! The contract (`docs/api/signed-tool-requests-v1.md`) against the code: the worked example byte for byte, then every check
//! of the receiving side, one at a time.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_signed_request::verify::{Incoming, Mismatch, Object, Shape, Time, Verifier, VerifyError};
use orch_signed_request::{
    CTI_LEN, KID_LEN, KeyError, Method, Request, SignError, Signer, SigningKey, kid_of,
};
use serde_json::Value;
use uuid::Uuid;

fn vectors() -> Value {
    let text = include_str!("vectors/v1.json");
    serde_json::from_str(text).unwrap()
}

fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap().to_owned()
}

fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn seed(v: &Value) -> [u8; 32] {
    unhex(&text(v, "seed")).try_into().unwrap()
}

struct Example {
    signer: Signer,
    public: [u8; 32],
    body: Vec<u8>,
    thread: Uuid,
    owner: String,
    iat: u64,
    cti: [u8; CTI_LEN],
    audience: String,
}

impl Example {
    fn new() -> Self {
        let v = vectors();
        let key = SigningKey::from_seed(&seed(&v)).unwrap();
        let public = key.public_key();
        let audience = text(&v, "audience");
        Example {
            signer: Signer::new(key, &audience, v["ttl_secs"].as_u64().unwrap()).unwrap(),
            public,
            body: text(&v, "body").into_bytes(),
            thread: text(&v, "thread").parse().unwrap(),
            owner: text(&v, "owner"),
            iat: v["iat"].as_u64().unwrap(),
            cti: unhex(&text(&v, "cti")).try_into().unwrap(),
            audience,
        }
    }

    fn request(&self) -> Request<'_> {
        Request {
            method: Method::Post,
            path: "/mcp",
            body: &self.body,
            thread: self.thread,
            owner: &self.owner,
        }
    }

    fn header(&self) -> String {
        self.signer
            .sign_with_cti(&self.request(), self.iat, self.cti)
            .unwrap()
    }

    fn verifier(&self) -> Verifier {
        Verifier::new(&self.audience, 30).pin(self.public).unwrap()
    }

    fn incoming<'a>(&'a self, body: &'a [u8]) -> Incoming<'a> {
        Incoming {
            method: "POST",
            path: "/mcp",
            query: None,
            body,
            content_encoding: None,
        }
    }
}

/// Decodes a header, lets `edit` change the bytes, and encodes it again.
fn edited(header: &str, edit: impl FnOnce(&mut Vec<u8>)) -> String {
    let mut bytes = URL_SAFE_NO_PAD.decode(header).unwrap();
    edit(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[test]
fn the_worked_example_is_written_byte_for_byte_as_the_real_cratestack_cose_wrote_it() {
    let v = vectors();
    let example = Example::new();
    // the key: its public half and its kid, derived by the thumbprint of RFC 9679
    assert_eq!(hex(&example.public), text(&v, "public_key"));
    assert_eq!(hex(&kid_of(&example.public)), text(&v, "kid"));
    assert_eq!(hex(&example.signer.kid()), text(&v, "kid"));
    // the request: the digest, and the payload as the real library signed it
    let header = example.header();
    assert_eq!(header, text(&v, "header"));
    let cose = URL_SAFE_NO_PAD.decode(&header).unwrap();
    assert_eq!(hex(&cose), text(&v, "cose_hex"));
    assert_eq!(cose.len(), 379);
    assert_eq!(header.len(), 506);
    let payload = text(&v, "payload");
    assert!(
        windows(&cose, payload.as_bytes()),
        "the payload is in the object, whole and in the order of the contract"
    );
}

fn windows(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_contract_page_and_the_vectors_say_the_same_thing() {
    let v = vectors();
    let page = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/api/signed-tool-requests-v1.md"
    ))
    .unwrap();
    for key in [
        "seed",
        "public_key",
        "kid",
        "audience",
        "cti",
        "body",
        "body_sha256",
        "payload",
        "thread",
        "owner",
        "header",
    ] {
        let value = text(&v, key);
        assert!(
            page.contains(&value),
            "the page does not hold {key}: {value}"
        );
    }
    assert!(page.contains(&v["iat"].to_string()));
    assert!(page.contains(&(v["iat"].as_u64().unwrap() + 30).to_string()));
    // the signature and the constants the page promises
    let cose = text(&v, "cose_hex");
    assert!(page.contains(&cose[cose.len() - 128..]));
    for constant in [
        orch_signed_request::TYP,
        orch_signed_request::HEADER,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    ] {
        assert!(page.contains(constant), "the page does not name {constant}");
    }
}

#[test]
fn the_receiving_side_accepts_the_example_and_says_who_it_is_for() {
    let example = Example::new();
    let header = example.header();
    let seen = example
        .verifier()
        .verify(
            &[&header],
            &example.incoming(&example.body),
            example.iat + 5,
        )
        .unwrap();
    assert_eq!(seen.owner, "amina@example.com");
    assert_eq!(seen.thread, example.thread);
    assert_eq!(seen.cti, example.cti);
    assert_eq!(
        (seen.iat, seen.exp, seen.keep_until),
        (example.iat, example.iat + 30, example.iat + 60)
    );
    assert_eq!(hex(&seen.kid), "34b84aeaf4b08543");
    // the skew on both sides of the lifetime
    for now in [example.iat - 30, example.iat + 30 + 30] {
        example
            .verifier()
            .verify(&[&header], &example.incoming(&example.body), now)
            .unwrap();
    }
}

#[test]
fn each_check_fails_in_its_own_place() {
    let example = Example::new();
    let header = example.header();
    let ok_time = example.iat + 5;
    let verify = |verifier: &Verifier, headers: &[&str], incoming: Incoming<'_>, now: u64| {
        verifier.verify(headers, &incoming, now).unwrap_err()
    };
    let body = example.body.clone();
    let good = example.incoming(&body);

    // 1: the header and the shape around it
    assert_eq!(
        verify(&example.verifier(), &[], good, ok_time),
        VerifyError::Header(Shape::Missing)
    );
    assert_eq!(
        verify(&example.verifier(), &[&header, &header], good, ok_time),
        VerifyError::Header(Shape::Repeated)
    );
    assert_eq!(
        verify(
            &example.verifier(),
            &["A".repeat(4097).as_str()],
            good,
            ok_time
        ),
        VerifyError::Header(Shape::TooLong)
    );
    for bad in [
        format!("{header}="),
        header.replace('-', "+"),
        format!(" {header}"),
        "!!!!".to_owned(),
    ] {
        let found = verify(&example.verifier(), &[bad.as_str()], good, ok_time);
        assert!(
            found == VerifyError::Header(Shape::NotBase64Url)
                || found == VerifyError::Object(Object::Malformed),
            "{bad}: {found}"
        );
    }
    assert_eq!(
        verify(&example.verifier(), &[&format!("{header}=")], good, ok_time),
        VerifyError::Header(Shape::NotBase64Url)
    );
    assert_eq!(
        verify(
            &example.verifier(),
            &[&header],
            Incoming {
                query: Some("a=1"),
                ..good
            },
            ok_time
        ),
        VerifyError::Header(Shape::Query)
    );
    assert_eq!(
        verify(
            &example.verifier(),
            &[&header],
            Incoming {
                content_encoding: Some("gzip"),
                ..good
            },
            ok_time
        ),
        VerifyError::Header(Shape::ContentEncoding)
    );

    // 2: the object
    let other = SigningKey::from_seed(&[9; 32]).unwrap().public_key();
    let stranger = Verifier::new(&example.audience, 30).pin(other).unwrap();
    assert_eq!(
        verify(&stranger, &[&header], good, ok_time),
        VerifyError::Object(Object::Unverified)
    );
    let no_key = Verifier::new(&example.audience, 30);
    assert_eq!(
        verify(&no_key, &[&header], good, ok_time),
        VerifyError::Object(Object::Unverified)
    );
    // a flipped bit in the signature, in the payload, in the protected header
    let len = URL_SAFE_NO_PAD.decode(&header).unwrap().len();
    for at in [len - 1, len - 70, 20, 40, 100] {
        let flipped = edited(&header, |b| b[at] ^= 1);
        let found = verify(&example.verifier(), &[&flipped], good, ok_time);
        assert!(
            matches!(found, VerifyError::Object(_)),
            "byte {at}: {found}"
        );
    }
    assert_eq!(
        verify(
            &example.verifier(),
            &[&edited(&header, |b| b.push(0))],
            good,
            ok_time
        ),
        VerifyError::Object(Object::Malformed),
        "trailing bytes"
    );
    assert_eq!(
        verify(
            &example.verifier(),
            &[&edited(&header, |b| b[0] = 0xd1)],
            good,
            ok_time
        ),
        VerifyError::Object(Object::Malformed),
        "another tag"
    );
    assert_eq!(
        verify(
            &example.verifier(),
            &[&edited(&header, |b| b.truncate(100))],
            good,
            ok_time
        ),
        VerifyError::Object(Object::Malformed),
        "cut short"
    );
    // a non-minimal head: the protected header's length written in two bytes where one is enough is another encoding of the
    // same value, and the profile refuses it
    let lengthened = edited(&header, |b| {
        assert_eq!(&b[..4], &[0xd2, 0x84, 0x58, 0x76]);
        b.splice(2..4, [0x59, 0x00, 0x76]);
    });
    assert_eq!(
        verify(&example.verifier(), &[&lengthened], good, ok_time),
        VerifyError::Object(Object::Malformed)
    );
    // a non-empty unprotected header (it is not signed, so only the profile can refuse it)
    let unprotected_at = 4 + 0x76;
    let with_unprotected = edited(&header, |b| {
        assert_eq!(b[unprotected_at], 0xa0);
        b.splice(unprotected_at..=unprotected_at, [0xa1, 0x01, 0x01]);
    });
    assert_eq!(
        verify(&example.verifier(), &[&with_unprotected], good, ok_time),
        VerifyError::Object(Object::Malformed)
    );
    // a transport envelope-like object with another typ: re-signed by the right key, so only the typ is wrong
    // (see the unit tests of the crate for the typ; here the algorithm label)
    let other_alg = edited(&header, |b| b[6] = 0x38); // 1: -19 written as -25 and a following byte: not the profile
    assert!(matches!(
        verify(&example.verifier(), &[&other_alg], good, ok_time),
        VerifyError::Object(Object::Malformed)
    ));

    // 3: the audience
    let elsewhere = Verifier::new("https://other.example/mcp", 30)
        .pin(example.public)
        .unwrap();
    assert_eq!(
        verify(&elsewhere, &[&header], good, ok_time),
        VerifyError::Audience
    );

    // 4: the time
    assert_eq!(
        verify(&example.verifier(), &[&header], good, example.iat - 31),
        VerifyError::Time(Time::NotYet)
    );
    assert_eq!(
        verify(&example.verifier(), &[&header], good, example.iat + 30 + 31),
        VerifyError::Time(Time::Expired)
    );

    // 5: the request
    assert_eq!(
        verify(
            &example.verifier(),
            &[&header],
            Incoming {
                method: "PUT",
                ..good
            },
            ok_time
        ),
        VerifyError::Request(Mismatch::Method)
    );
    assert_eq!(
        verify(
            &example.verifier(),
            &[&header],
            Incoming {
                method: "GET",
                ..good
            },
            ok_time
        ),
        VerifyError::Request(Mismatch::Method)
    );
    for path in ["/mcp2", "/mcp/", "/MCP", "/", "/other"] {
        assert_eq!(
            verify(
                &example.verifier(),
                &[&header],
                Incoming { path, ..good },
                ok_time
            ),
            VerifyError::Request(Mismatch::Path),
            "{path}"
        );
    }
    let mut spaced = body.clone();
    spaced.push(b' ');
    let mut reordered = body.clone();
    reordered.swap(2, 3);
    for other_body in [spaced.as_slice(), reordered.as_slice(), b"".as_slice()] {
        assert_eq!(
            verify(
                &example.verifier(),
                &[&header],
                example.incoming(other_body),
                ok_time
            ),
            VerifyError::Request(Mismatch::Body)
        );
    }
}

#[test]
fn the_thread_and_the_owner_are_bound_by_the_signature() {
    // they are in the payload, which the signature covers: change one and the object is not the one that was signed
    let example = Example::new();
    let header = example.header();
    for (from, to) in [
        (
            "0194a6c2-6b1e-7c3a-9f40-5d2e8b7a1c33",
            "0194a6c2-6b1e-7c3a-9f40-5d2e8b7a1c34",
        ),
        ("amina@example.com", "bella@example.com"),
    ] {
        let swapped = edited(&header, |b| {
            let at = b
                .windows(from.len())
                .position(|w| w == from.as_bytes())
                .unwrap();
            b[at..at + to.len()].copy_from_slice(to.as_bytes());
        });
        assert_eq!(
            example
                .verifier()
                .verify(
                    &[&swapped],
                    &example.incoming(&example.body),
                    example.iat + 1
                )
                .unwrap_err(),
            VerifyError::Object(Object::Unverified),
            "{from} -> {to}"
        );
    }
    // and a different thread or owner signed by the real key arrives as what it says
    let other = Uuid::from_u128(0x0194_a6c2_6b1e_7c3a_9f40_5d2e_8b7a_1c99);
    let header = example
        .signer
        .sign_with_cti(
            &Request {
                thread: other,
                owner: "bella@example.com",
                ..example.request()
            },
            example.iat,
            [7; CTI_LEN],
        )
        .unwrap();
    let seen = example
        .verifier()
        .verify(&[&header], &example.incoming(&example.body), example.iat)
        .unwrap();
    assert_eq!(
        (seen.thread, seen.owner.as_str()),
        (other, "bella@example.com")
    );
}

#[test]
fn an_object_that_lives_too_long_or_ends_before_it_begins_is_refused() {
    let example = Example::new();
    let sign = |iat: u64, exp: u64| {
        // the object a signer would refuse to make: written by the crate's own path with the lifetime changed
        example
            .signer
            .sign_for_tests(&example.request(), iat, exp, [3; CTI_LEN])
            .unwrap()
    };
    let long = sign(example.iat, example.iat + 61);
    assert_eq!(
        example
            .verifier()
            .verify(&[&long], &example.incoming(&example.body), example.iat)
            .unwrap_err(),
        VerifyError::Time(Time::Lifetime)
    );
    let backwards = sign(example.iat, example.iat - 1);
    assert_eq!(
        example
            .verifier()
            .verify(&[&backwards], &example.incoming(&example.body), example.iat)
            .unwrap_err(),
        VerifyError::Time(Time::Lifetime)
    );
    let at_the_limit = sign(example.iat, example.iat + 60);
    example
        .verifier()
        .verify(
            &[&at_the_limit],
            &example.incoming(&example.body),
            example.iat,
        )
        .unwrap();
}

#[test]
fn a_get_and_a_delete_bind_the_digest_of_nothing() {
    let example = Example::new();
    for (method, name) in [(Method::Get, "GET"), (Method::Delete, "DELETE")] {
        let header = example
            .signer
            .sign(
                &Request {
                    method,
                    body: b"",
                    ..example.request()
                },
                example.iat,
            )
            .unwrap();
        let incoming = Incoming {
            method: name,
            ..example.incoming(b"")
        };
        example
            .verifier()
            .verify(&[&header], &incoming, example.iat)
            .unwrap();
        assert_eq!(
            example
                .verifier()
                .verify(
                    &[&header],
                    &Incoming {
                        method: "POST",
                        ..incoming
                    },
                    example.iat
                )
                .unwrap_err(),
            VerifyError::Request(Mismatch::Method)
        );
    }
}

#[test]
fn every_header_has_a_cti_of_its_own() {
    let example = Example::new();
    let ctis: Vec<_> = (0..64)
        .map(|_| {
            let header = example
                .signer
                .sign(&example.request(), example.iat)
                .unwrap();
            example
                .verifier()
                .verify(&[&header], &example.incoming(&example.body), example.iat)
                .unwrap()
                .cti
        })
        .collect();
    for (i, a) in ctis.iter().enumerate() {
        assert_ne!(*a, [0; CTI_LEN]);
        assert!(ctis[i + 1..].iter().all(|b| b != a), "a cti came twice");
    }
}

#[test]
fn the_signer_keeps_to_the_limits_of_the_contract() {
    let example = Example::new();
    let key = || SigningKey::from_seed(&[1; 32]).unwrap();
    // construction
    for ttl in [0, 61, u64::MAX] {
        assert_eq!(
            Signer::new(key(), "aud", ttl).unwrap_err(),
            SignError::Lifetime
        );
    }
    Signer::new(key(), "aud", 1).unwrap();
    Signer::new(key(), "aud", 60).unwrap();
    for audience in [String::new(), "a".repeat(257), "a\nb".to_owned()] {
        assert_eq!(
            Signer::new(key(), audience, 30).unwrap_err(),
            SignError::Audience
        );
    }
    Signer::new(key(), "a".repeat(256), 30).unwrap();
    // a request
    let sign = |request: &Request<'_>| example.signer.sign(request, example.iat);
    for path in ["mcp", "", "/caf\u{e9}", "/a\nb"] {
        assert_eq!(
            sign(&Request {
                path,
                ..example.request()
            })
            .unwrap_err(),
            SignError::Path,
            "{path:?}"
        );
    }
    let long = format!("/{}", "a".repeat(512));
    assert_eq!(
        sign(&Request {
            path: &long,
            ..example.request()
        })
        .unwrap_err(),
        SignError::Path
    );
    let longest = format!("/{}", "a".repeat(511));
    sign(&Request {
        path: &longest,
        ..example.request()
    })
    .unwrap();
    for owner in ["", &"a".repeat(321), "a\nb"] {
        assert_eq!(
            sign(&Request {
                owner,
                ..example.request()
            })
            .unwrap_err(),
            SignError::Owner,
            "{owner:?}"
        );
    }
    let owner = "a".repeat(320);
    let header = sign(&Request {
        owner: &owner,
        path: &longest,
        ..example.request()
    })
    .unwrap();
    assert!(
        header.len() <= orch_signed_request::MAX_HEADER_BYTES,
        "{}",
        header.len()
    );
    // the biggest audience, owner and path still fit a header
    let big = Signer::new(
        SigningKey::from_seed(&[1; 32]).unwrap(),
        "a".repeat(256),
        60,
    )
    .unwrap();
    let header = big
        .sign(
            &Request {
                owner: &owner,
                path: &longest,
                ..example.request()
            },
            example.iat,
        )
        .unwrap();
    assert!(
        header.len() <= orch_signed_request::MAX_HEADER_BYTES,
        "{}",
        header.len()
    );
}

/// A PKCS#8 version-1 `PrivateKeyInfo` for an Ed25519 seed, as `openssl genpkey -algorithm ed25519` writes it, in PEM.
fn pem_of(seed: &[u8; 32]) -> String {
    let mut der = vec![
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    der.extend_from_slice(seed);
    let body = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = body
        .as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect();
    format!(
        "-----BEGIN PRIVATE {}-----\n{}\n-----END PRIVATE {}-----\n",
        "KEY",
        lines.join("\n"),
        "KEY"
    )
}

#[test]
fn a_pkcs8_pem_is_read_and_anything_else_is_refused_without_quoting_it() {
    let v = vectors();
    let pem = pem_of(&seed(&v));
    let key = SigningKey::from_pkcs8_pem(&pem).unwrap();
    assert_eq!(hex(&key.public_key()), text(&v, "public_key"));
    assert_eq!(hex(&key.kid()), text(&v, "kid"));
    assert_eq!(key.kid().len(), KID_LEN);
    // the CRLF of a file written on Windows, and the white space around the block
    SigningKey::from_pkcs8_pem(&format!("\n  {}  \n", pem.replace('\n', "\r\n"))).unwrap();
    // Debug shows the kid and nothing of the key
    let shown = format!("{key:?}");
    assert!(shown.contains("34b84aeaf4b08543"), "{shown}");
    assert!(
        !shown.contains(&text(&v, "seed")) && !shown.contains("MC4C"),
        "{shown}"
    );
    // not a key
    for (bad, why) in [
        ("", KeyError::NotPem),
        ("hello", KeyError::NotPem),
        (&format!("text before\n{pem}"), KeyError::NotPem),
        (&format!("{pem}{pem}"), KeyError::NotBase64),
        (
            "-----BEGIN PRIVATE KEY-----\n!!!\n-----END PRIVATE KEY-----",
            KeyError::NotBase64,
        ),
        (
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----",
            KeyError::NotEd25519,
        ),
    ] {
        let found = SigningKey::from_pkcs8_pem(bad).unwrap_err();
        assert_eq!(found, why, "{bad:?}");
        assert!(!found.to_string().contains("AAAA"));
    }
    // a PEM of a key of another kind (an RSA header with the same framing) is not an Ed25519 key
    let mut der = vec![0x30, 0x03, 0x02, 0x01, 0x00];
    der.extend_from_slice(&[0; 40]);
    let other = format!(
        "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
        base64::engine::general_purpose::STANDARD.encode(der)
    );
    assert_eq!(
        SigningKey::from_pkcs8_pem(&other).unwrap_err(),
        KeyError::NotEd25519
    );
}

#[test]
fn a_verifier_pins_at_most_two_keys() {
    let one = SigningKey::from_seed(&[1; 32]).unwrap().public_key();
    let two = SigningKey::from_seed(&[2; 32]).unwrap().public_key();
    let three = SigningKey::from_seed(&[3; 32]).unwrap().public_key();
    let verifier = Verifier::new("aud", 30).pin(one).unwrap().pin(two).unwrap();
    assert_eq!(verifier.pin(three).unwrap_err(), VerifyError::TooManyKeys);
    // either key verifies while both are pinned: a rotation
    for seed in [[1_u8; 32], [2; 32]] {
        let signer = Signer::new(SigningKey::from_seed(&seed).unwrap(), "aud", 30).unwrap();
        let both = Verifier::new("aud", 30).pin(one).unwrap().pin(two).unwrap();
        let thread = Uuid::from_u128(1);
        let request = Request {
            method: Method::Post,
            path: "/p",
            body: b"{}",
            thread,
            owner: "o@e.com",
        };
        let header = signer.sign(&request, 1_000).unwrap();
        both.verify(
            &[&header],
            &Incoming {
                method: "POST",
                path: "/p",
                query: None,
                body: b"{}",
                content_encoding: None,
            },
            1_000,
        )
        .unwrap();
    }
}
