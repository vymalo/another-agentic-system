//! The id of an upload slot (`uploads/v1`, `docs/api/uploads-v1.md`): a known-answer vector, what
//! reads back, and everything that does not.
//!
//! The vector was computed with an independent implementation (Python's `hmac`, `hashlib` and
//! `base64`, 2026-10-14), not with this crate. It is a test vector, not a credential: the key is the
//! made-up key 1 of `vectors.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use jiff::Timestamp;
use orch_core::ThreadId;
use orch_thread_token::{
    MAX_UPLOAD_ID_BYTES, ThreadToolsKeys, UploadClaims, UploadIdError, mint, mint_upload_id,
    verify, verify_upload_id,
};
use secrecy::SecretString;

const KEY_1: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const KID_1: &str = "6c86c6aac5fb24bc";
const KEY_2: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

const THREAD: &str = "01927a4e-3b00-7000-8000-000000000001";
/// SHA-256 of `hello`.
const HASH: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
/// 2026-10-01T12:05:00Z.
const EXPIRES: i64 = 1_790_856_300;

const PAYLOAD: &str = "eyJ0IjoiMDE5MjdhNGUtM2IwMC03MDAwLTgwMDAtMDAwMDAwMDAwMDAxIiwiaiI6MywiaCI6IjJjZjI0ZGJhNWZiMGEzMGUyNmU4M2IyYWM1YjllMjllMWIxNjFlNWMxZmE3NDI1ZTczMDQzMzYyOTM4Yjk4MjQiLCJzIjo1LCJtIjoidGV4dC9wbGFpbiIsImUiOjE3OTA4NTYzMDB9";
// nosemgrep: a known-answer test vector, signed with the made-up test key above, not a credential
const VECTOR: &str = "6c86c6aac5fb24bc.eyJ0IjoiMDE5MjdhNGUtM2IwMC03MDAwLTgwMDAtMDAwMDAwMDAwMDAxIiwiaiI6MywiaCI6IjJjZjI0ZGJhNWZiMGEzMGUyNmU4M2IyYWM1YjllMjllMWIxNjFlNWMxZmE3NDI1ZTczMDQzMzYyOTM4Yjk4MjQiLCJzIjo1LCJtIjoidGV4dC9wbGFpbiIsImUiOjE3OTA4NTYzMDB9.SHGqqprRlR9qz8sWWoOaGnE0siTkX7hVQ-ofWML7zvs";

fn secret(s: &str) -> SecretString {
    SecretString::from(s.to_owned())
}

fn keys() -> ThreadToolsKeys {
    ThreadToolsKeys::new(secret(KEY_1), None).unwrap()
}

fn hash() -> [u8; 32] {
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(HASH.as_bytes().chunks_exact(2)) {
        *slot = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
    }
    out
}

fn claims() -> UploadClaims {
    UploadClaims {
        thread: THREAD.parse().unwrap(),
        job: 3,
        sha256: hash(),
        size: 5,
        media_type: "text/plain".to_owned(),
        expires_at: Timestamp::from_second(EXPIRES).unwrap(),
    }
}

#[test]
fn the_known_answer_is_exactly_these_bytes() {
    let id = mint_upload_id(&keys(), &claims()).unwrap();
    assert_eq!(id, VECTOR);
    assert!(id.starts_with(&format!("{KID_1}.{PAYLOAD}.")));
    assert_eq!(verify_upload_id(&keys(), VECTOR).unwrap(), claims());
}

#[test]
fn what_is_minted_reads_back_with_the_current_key_and_the_previous_one() {
    let id = mint_upload_id(&keys(), &claims()).unwrap();
    assert_eq!(verify_upload_id(&keys(), &id).unwrap(), claims());

    // a rotation: the new current key signs, the old one still verifies
    let rotated = ThreadToolsKeys::new(secret(KEY_2), Some(secret(KEY_1))).unwrap();
    assert_eq!(verify_upload_id(&rotated, &id).unwrap(), claims());
    let new = mint_upload_id(&rotated, &claims()).unwrap();
    assert_ne!(new, id);
    assert_eq!(
        verify_upload_id(&keys(), &new),
        Err(UploadIdError::UnknownKey),
        "a replica that holds neither key of the new id refuses it"
    );
}

#[test]
fn another_key_never_reads_an_id() {
    let other = ThreadToolsKeys::new(secret(KEY_2), None).unwrap();
    assert_eq!(
        verify_upload_id(&other, VECTOR),
        Err(UploadIdError::UnknownKey)
    );
    // the same `kid` text, a different key: the signature is what refuses
    let forged = {
        let mut parts: Vec<&str> = VECTOR.split('.').collect();
        parts[0] = "8588cdfcd6d2b0d5";
        parts.join(".")
    };
    assert_eq!(
        verify_upload_id(&other, &forged),
        Err(UploadIdError::Signature)
    );
}

#[test]
fn no_one_character_change_is_accepted() {
    let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.";
    for (i, original) in VECTOR.char_indices() {
        for c in alphabet.chars().filter(|c| *c != original) {
            let mut changed = VECTOR.to_owned();
            changed.replace_range(i..=i, &c.to_string());
            assert!(
                verify_upload_id(&keys(), &changed).is_err(),
                "position {i} changed to {c:?} was accepted"
            );
        }
    }
}

#[test]
fn truncated_extended_and_malformed_ids_are_refused() {
    for bad in [
        "",
        ".",
        "..",
        "...",
        "a.b",
        "a.b.c.d",
        &VECTOR[..VECTOR.len() - 1],
        &format!("{VECTOR}A"),
        &format!("{VECTOR}="),
        &VECTOR.replace('-', "+"),
        &format!(" {VECTOR}"),
        &format!("{VECTOR}\n"),
        &"x".repeat(MAX_UPLOAD_ID_BYTES + 1),
    ] {
        assert!(verify_upload_id(&keys(), bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        verify_upload_id(&keys(), &"x".repeat(MAX_UPLOAD_ID_BYTES + 1)),
        Err(UploadIdError::TooLong)
    );
}

#[test]
fn an_id_is_not_a_thread_tools_token_and_a_token_is_not_an_id() {
    // the id has the shape of three dot-separated segments, like a token: neither opens the other
    let now = Timestamp::from_second(EXPIRES - 100).unwrap();
    assert!(verify(&keys(), VECTOR, now).is_err());

    let token = mint(
        &keys(),
        &orch_thread_token::Claims {
            thread: THREAD.parse().unwrap(),
            job: 3,
            agent: orch_core::AgentId::new("coder"),
            caller: orch_core::Caller::Main,
            depth: 0,
            message_id: "m-1".to_owned(),
            issued_at: now,
            expires_at: Timestamp::from_second(EXPIRES + 7200).unwrap(),
        },
    )
    .unwrap();
    use secrecy::ExposeSecret as _;
    assert!(verify_upload_id(&keys(), token.expose_secret()).is_err());
}

#[test]
fn the_mac_is_domain_separated_from_a_token_signature() {
    // An id signed over the payload alone (what a token's signature would cover, without the
    // domain) is not accepted: the domain line is part of what is signed.
    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use hmac::{Hmac, Mac as _};
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(KEY_1.as_bytes()).unwrap();
    mac.update(format!("{KID_1}.{PAYLOAD}").as_bytes());
    let undomained = format!(
        "{KID_1}.{PAYLOAD}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    );
    assert_eq!(
        verify_upload_id(&keys(), &undomained),
        Err(UploadIdError::Signature)
    );
}

#[test]
fn claims_that_disagree_are_not_minted_and_a_signed_id_of_them_is_not_read() {
    let mut bad = [
        UploadClaims { job: 0, ..claims() },
        UploadClaims {
            size: 0,
            ..claims()
        },
        UploadClaims {
            media_type: String::new(),
            ..claims()
        },
        UploadClaims {
            media_type: "text/plain\r\nx: y".to_owned(),
            ..claims()
        },
        UploadClaims {
            media_type: "t".repeat(256),
            ..claims()
        },
        UploadClaims {
            expires_at: Timestamp::new(EXPIRES, 1).unwrap(),
            ..claims()
        },
    ]
    .into_iter();
    assert!(bad.all(|c| mint_upload_id(&keys(), &c) == Err(UploadIdError::Claims)));
}

#[test]
fn the_claims_are_the_threads_the_jobs_and_the_files_own() {
    let other_thread = UploadClaims {
        thread: ThreadId(uuid::Uuid::from_u128(7)),
        ..claims()
    };
    let a = mint_upload_id(&keys(), &claims()).unwrap();
    let b = mint_upload_id(&keys(), &other_thread).unwrap();
    assert_ne!(a, b);
    assert_eq!(
        verify_upload_id(&keys(), &b).unwrap().thread,
        other_thread.thread
    );
    // an id is at most the bound, even with the longest type
    let longest = UploadClaims {
        media_type: format!("application/{}", "x".repeat(255 - "application/".len())),
        size: u64::MAX,
        job: u32::MAX,
        ..claims()
    };
    let id = mint_upload_id(&keys(), &longest).unwrap();
    assert!(id.len() <= MAX_UPLOAD_ID_BYTES, "{}", id.len());
    assert_eq!(verify_upload_id(&keys(), &id).unwrap(), longest);
}

#[test]
fn an_id_shows_no_key() {
    let shown = format!("{:?}", claims());
    assert!(!shown.contains(KEY_1));
    assert!(
        !format!("{:?}", UploadIdError::Signature).contains(KEY_1),
        "an error never shows a key"
    );
}
