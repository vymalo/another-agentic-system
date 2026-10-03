//! Property tests of the rank keys that order a person's list of threads (ADR 0042).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::{MAX_RANK_LEN, RankError, between, is_valid_rank, spread};
use proptest::prelude::*;

fn key() -> impl Strategy<Value = String> {
    // up to twelve characters, never ending in `0`
    "[0-9a-z]{0,11}[1-9a-z]"
}

proptest! {
    /// Whatever two keys are given in order, the key between them is valid and strictly between,
    /// or there is no room under the cap.
    #[test]
    fn a_key_between_two_keys_is_between_them(a in key(), b in key()) {
        prop_assume!(a != b);
        let (lower, upper) = if a < b { (a, b) } else { (b, a) };
        match between(Some(&lower), Some(&upper)) {
            Ok(k) => {
                prop_assert!(is_valid_rank(&k), "{k}");
                prop_assert!(lower < k && k < upper, "{lower} < {k} < {upper}");
            }
            Err(e) => prop_assert_eq!(e, RankError::TooLong),
        }
    }

    /// Before the first and after the last there is always a key.
    #[test]
    fn a_key_beyond_either_end_is_beyond_it(a in key()) {
        match between(None, Some(&a)) {
            Ok(k) => {
                prop_assert!(is_valid_rank(&k));
                prop_assert!(k < a, "{k} < {a}");
            }
            Err(e) => prop_assert_eq!(e, RankError::TooLong),
        }
        match between(Some(&a), None) {
            Ok(k) => {
                prop_assert!(is_valid_rank(&k));
                prop_assert!(k > a, "{k} > {a}");
            }
            Err(e) => prop_assert_eq!(e, RankError::TooLong),
        }
    }

    /// Any run of inserts at any places of a list that is re-spread when a key does not fit keeps
    /// the order the places asked for, keeps every key valid and under the cap, and re-spreads
    /// seldom.
    #[test]
    fn inserts_anywhere_keep_the_order_asked_for(
        places in proptest::collection::vec(0..1000_usize, 1..400)
    ) {
        // `order` is the list as the person sees it; `keys` are what it is stored with
        let mut order: Vec<usize> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        for (n, place) in places.into_iter().enumerate() {
            let at = place % (order.len() + 1);
            let made = |keys: &[String]| {
                between(
                    at.checked_sub(1).and_then(|i| keys.get(i)).map(String::as_str),
                    keys.get(at).map(String::as_str),
                )
            };
            let key = match made(&keys) {
                Ok(k) => k,
                Err(RankError::TooLong) => {
                    keys = spread(keys.len());
                    made(&keys).unwrap()
                }
                Err(e) => panic!("{e}"),
            };
            prop_assert!(is_valid_rank(&key) && key.len() <= MAX_RANK_LEN, "{key}");
            order.insert(at, n);
            keys.insert(at, key);
            prop_assert!(keys.windows(2).all(|w| w[0] < w[1]), "order lost: {keys:?}");
        }
    }
}
