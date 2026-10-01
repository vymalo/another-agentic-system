//! Properties of the token: it reads back what was written, and nothing else reads.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use jiff::Timestamp;
use orch_core::{AgentId, Caller};
use orch_thread_token::{Claims, ThreadToolsKeys, TokenError, mint, verify};
use proptest::prelude::*;
use secrecy::{ExposeSecret, SecretString};

const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const OTHER: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";
const NOW: i64 = 1_790_856_000;

fn keys() -> ThreadToolsKeys {
    ThreadToolsKeys::new(SecretString::from(KEY.to_owned()), None).unwrap()
}

fn caller_and_depth() -> impl Strategy<Value = (Caller, u8)> {
    prop_oneof![
        Just((Caller::Main, 0u8)),
        (1u32..=u32::MAX, 1u8..=255).prop_map(|(n, d)| (Caller::Ask(n), d)),
    ]
}

fn claims() -> impl Strategy<Value = Claims> {
    (
        any::<u128>(),
        1u32..=u32::MAX,
        "[a-z0-9][a-z0-9-]{0,62}",
        caller_and_depth(),
        "[ -~]{1,64}",
        0i64..4_000_000_000,
        0i64..86_400,
    )
        .prop_map(
            |(thread, job, agent, (caller, depth), message_id, iat, ttl)| Claims {
                thread: orch_core::ThreadId(uuid_of(thread)),
                job,
                agent: AgentId::new(agent),
                caller,
                depth,
                message_id,
                issued_at: Timestamp::from_second(iat).unwrap(),
                expires_at: Timestamp::from_second(iat + ttl).unwrap(),
            },
        )
}

fn uuid_of(bits: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(bits)
}

proptest! {
    #[test]
    fn what_is_minted_reads_back(claims in claims()) {
        let token = mint(&keys(), &claims).unwrap();
        let at = claims.issued_at;
        prop_assert_eq!(verify(&keys(), token.expose_secret(), at).unwrap(), claims);
    }

    /// Any one character of a token changed to any other character of the alphabet (or removed,
    /// or a character added) is refused: the signature covers the text, and the encoding is
    /// strict, so there is no second spelling.
    #[test]
    fn no_single_change_of_a_token_is_accepted(
        claims in claims(),
        position in any::<prop::sample::Index>(),
        replacement in prop::sample::select(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.=+/ ".chars().collect::<Vec<_>>()
        ),
    ) {
        let token = mint(&keys(), &claims).unwrap();
        let token = token.expose_secret();
        let i = position.index(token.len());
        let original = token.as_bytes()[i] as char;
        prop_assume!(original != replacement);
        let mut changed: Vec<char> = token.chars().collect();
        changed[i] = replacement;
        let changed: String = changed.into_iter().collect();
        prop_assert!(verify(&keys(), &changed, claims.issued_at).is_err(), "{changed}");

        let mut shorter = token.to_owned();
        shorter.remove(i);
        prop_assert!(verify(&keys(), &shorter, claims.issued_at).is_err());
        let mut longer = token.to_owned();
        longer.insert(i, replacement);
        prop_assert!(verify(&keys(), &longer, claims.issued_at).is_err());
    }

    /// A token is for the keys that minted it: another key does not read it, whatever the claims.
    #[test]
    fn another_key_never_reads_it(claims in claims()) {
        let token = mint(&keys(), &claims).unwrap();
        let other = ThreadToolsKeys::new(SecretString::from(OTHER.to_owned()), None).unwrap();
        prop_assert_eq!(
            verify(&other, token.expose_secret(), claims.issued_at),
            Err(TokenError::UnknownKey)
        );
    }

    /// The lifetime is exactly the claims': valid from 30 s before the issue to 30 s after the
    /// expiry, whatever the times are.
    #[test]
    fn the_lifetime_is_the_claims_plus_the_skew(claims in claims(), offset in -120i64..120) {
        let token = mint(&keys(), &claims).unwrap();
        let now = Timestamp::from_second(claims.issued_at.as_second() + offset).unwrap();
        let got = verify(&keys(), token.expose_secret(), now);
        let past_expiry = now.as_second() >= claims.expires_at.as_second() + 30;
        let before_issue = claims.issued_at.as_second() > now.as_second() + 30;
        match (past_expiry, before_issue) {
            (true, _) => prop_assert_eq!(got, Err(TokenError::Expired)),
            (false, true) => prop_assert_eq!(got, Err(TokenError::NotYetValid)),
            (false, false) => prop_assert!(got.is_ok()),
        }
    }

    /// Whatever bytes arrive in the header, the payload or the signature position, `verify` does
    /// not panic and does not accept.
    #[test]
    fn garbage_is_refused_without_a_panic(
        a in "[A-Za-z0-9_=.+/ -]{0,60}",
        b in "[A-Za-z0-9_=.+/ -]{0,60}",
        c in "[A-Za-z0-9_=.+/ -]{0,60}",
    ) {
        let token = [a, b, c].join(".");
        prop_assert!(verify(&keys(), &token, Timestamp::from_second(NOW).unwrap()).is_err());
    }
}
