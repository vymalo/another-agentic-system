//! Fractional rank keys: the order of a person's list of threads (ADR 0042, decision 7).
//!
//! A key is a string over `0-9a-z` read as the digits of a base-36 fraction, `0.d1d2d3…`. Keys are
//! compared **bytewise**, which is the order of the fractions as long as no key ends in `0` (a key
//! that did would equal a shorter one), so no key does. A key is at most [`MAX_RANK_LEN`]
//! characters. [`between`] gives a key that sorts strictly between two others, [`spread`] gives
//! `n` evenly spaced keys for the store to re-write a list with when [`between`] has run out of
//! room.
//!
//! The functions are pure: the store reads the neighbours, asks here, and writes the key.

use std::cmp::Ordering;

/// The longest key. A store that would need a longer one re-spreads the owner's keys instead.
pub const MAX_RANK_LEN: usize = 128;

const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
const BASE: u8 = 36;

/// Why a key could not be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RankError {
    /// A key that is empty, longer than [`MAX_RANK_LEN`], ends in `0` or has a character outside
    /// `0-9a-z`.
    #[error("a rank key is empty, too long, ends in 0 or has a character outside 0-9a-z")]
    Malformed,
    /// The lower key does not sort before the upper one (it is equal to it, or after it).
    #[error("the lower key does not sort before the upper key")]
    Order,
    /// The key between would be longer than [`MAX_RANK_LEN`].
    #[error("the key between would be longer than {MAX_RANK_LEN} characters")]
    TooLong,
}

/// Whether `key` is one: non-empty, at most [`MAX_RANK_LEN`] characters of `0-9a-z`, and not ending
/// in `0`.
pub fn is_valid_rank(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_RANK_LEN
        && key.bytes().all(|b| digit(b).is_some())
        && !key.ends_with('0')
}

fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'z' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn digits(key: &str) -> Result<Vec<u8>, RankError> {
    if !is_valid_rank(key) {
        return Err(RankError::Malformed);
    }
    Ok(key.bytes().filter_map(digit).collect())
}

/// Which way the new key is biased, so that the common moves of a list grow a key slowly.
#[derive(Clone, Copy)]
enum Bias {
    /// Nothing below: a prepend. Takes the largest digit that fits, so that a run of prepends
    /// walks down a digit at a time (35 steps a character) and not by halves (5 steps).
    Prepend,
    /// Nothing above: an append. The mirror of `Prepend`.
    Append,
    /// Between two keys: the middle, which halves the room whichever side the next move is on.
    Middle,
}

/// A key strictly between `lower` and `upper` (`None` is the open end of the list: a key before
/// every key, or after every key). Both given keys must be valid and `lower` must sort before
/// `upper`.
///
/// # Errors
/// [`RankError::Malformed`] for a given key that is not one, [`RankError::Order`] when `lower` does
/// not sort before `upper`, [`RankError::TooLong`] when the key would pass [`MAX_RANK_LEN`]: the
/// caller then re-spreads the keys ([`spread`]) and asks again.
pub fn between(lower: Option<&str>, upper: Option<&str>) -> Result<String, RankError> {
    let lo = lower.map(digits).transpose()?;
    let hi = upper.map(digits).transpose()?;
    if let (Some(l), Some(u)) = (lower, upper)
        && l.as_bytes().cmp(u.as_bytes()) != Ordering::Less
    {
        return Err(RankError::Order);
    }
    let bias = match (&lo, &hi) {
        (None, Some(_)) => Bias::Prepend,
        (Some(_), None) => Bias::Append,
        _ => Bias::Middle,
    };
    let key = midpoint(lo.as_deref().unwrap_or(&[]), hi.as_deref(), bias);
    if key.len() > MAX_RANK_LEN {
        return Err(RankError::TooLong);
    }
    Ok(key
        .into_iter()
        .map(|d| char::from(ALPHABET[usize::from(d)]))
        .collect())
}

/// The digits of a key strictly between `a` (padded with zeros; empty is zero) and `b` (`None` is
/// one). `a < b`, neither ends in a zero digit.
fn midpoint(mut a: &[u8], mut b: Option<&[u8]>, bias: Bias) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        if let Some(upper) = b {
            // The digits both keys share are the new key's.
            let mut shared = 0;
            while shared < upper.len() && upper[shared] == a.get(shared).copied().unwrap_or(0) {
                shared += 1;
            }
            if shared > 0 {
                out.extend_from_slice(&upper[..shared]);
                a = a.get(shared..).unwrap_or(&[]);
                b = Some(&upper[shared..]);
            }
        }
        let da = a.first().copied().unwrap_or(0);
        let db = b.and_then(|u| u.first().copied()).unwrap_or(BASE);
        if db - da > 1 {
            out.push(match bias {
                Bias::Prepend => db - 1,
                Bias::Append => da + 1,
                Bias::Middle => (da + db) / 2,
            });
            return out;
        }
        // The first digits are neighbours. When the upper key goes on, its first digit alone is
        // below it and above the lower key; otherwise keep the lower digit and look below it for
        // room, with nothing above any more.
        match b {
            Some(upper) if upper.len() > 1 => {
                out.push(db);
                return out;
            }
            _ => {
                out.push(da);
                a = a.get(1..).unwrap_or(&[]);
                b = None;
            }
        }
    }
}

/// `n` keys in increasing order, evenly spaced with room between them: what a list is re-written
/// with when [`between`] has run out of room. All are valid and far shorter than [`MAX_RANK_LEN`].
pub fn spread(n: usize) -> Vec<String> {
    if n == 0 {
        return Vec::new();
    }
    // The width that leaves at least 36 keys' room between neighbours.
    let wanted = (n as u128 + 1).saturating_mul(u128::from(BASE));
    let (mut width, mut scale) = (1_usize, u128::from(BASE));
    while scale < wanted {
        width += 1;
        scale = scale.saturating_mul(u128::from(BASE));
    }
    let step = scale / (n as u128 + 1);
    (1..=n as u128)
        .map(|i| {
            let mut value = step * i;
            let mut digits = vec![0_u8; width];
            for slot in digits.iter_mut().rev() {
                *slot = u8::try_from(value % u128::from(BASE)).unwrap_or(0);
                value /= u128::from(BASE);
            }
            while digits.last() == Some(&0) {
                digits.pop();
            }
            digits
                .into_iter()
                .map(|d| char::from(ALPHABET[usize::from(d)]))
                .collect()
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn key(lower: Option<&str>, upper: Option<&str>) -> String {
        between(lower, upper).unwrap()
    }

    #[test]
    fn the_first_key_is_in_the_middle() {
        assert_eq!(key(None, None), "i");
    }

    #[test]
    fn a_key_is_between_its_neighbours() {
        let k = key(Some("a"), Some("c"));
        assert!("a" < k.as_str() && k.as_str() < "c", "{k}");
        let k = key(Some("a"), Some("b"));
        assert!("a" < k.as_str() && k.as_str() < "b", "{k}");
        let k = key(Some("1"), Some("10001"));
        assert!("1" < k.as_str() && k.as_str() < "10001", "{k}");
        let k = key(None, Some("1"));
        assert!(k.as_str() < "1", "{k}");
        let k = key(Some("zz"), None);
        assert!(k.as_str() > "zz", "{k}");
    }

    #[test]
    fn a_prepend_walks_down_a_digit_at_a_time() {
        let mut k = key(None, Some("i"));
        assert_eq!(k, "h");
        for expect in ["g", "f", "e"] {
            k = key(None, Some(&k));
            assert_eq!(k, expect);
        }
    }

    #[test]
    fn an_append_walks_up_a_digit_at_a_time() {
        assert_eq!(key(Some("i"), None), "j");
        assert_eq!(key(Some("y"), None), "z");
        // past the last digit it goes on to the next place
        assert_eq!(key(Some("z"), None), "z1");
    }

    #[test]
    fn a_key_never_ends_in_zero() {
        // before "1" there is only "0...": the key has to go a place down
        let k = key(None, Some("1"));
        assert!(is_valid_rank(&k), "{k}");
        let k = key(None, Some("01"));
        assert!(is_valid_rank(&k), "{k}");
        assert_eq!(k.as_bytes().last().copied(), Some(b'z'));
    }

    #[test]
    fn the_ends_of_the_backfill_have_room() {
        // what migration 0016 writes: twelve hex digits and an h
        let first = "000000000001h";
        let second = "000000000002h";
        assert!(is_valid_rank(first));
        let before = key(None, Some(first));
        assert!(before.as_str() < first);
        let middle = key(Some(first), Some(second));
        assert!(first < middle.as_str() && middle.as_str() < second);
        let after = key(Some(second), None);
        assert!(after.as_str() > second);
    }

    #[test]
    fn refuses_what_is_not_a_key() {
        for bad in ["", "A", "a0", "a-b", "é"] {
            assert_eq!(between(Some(bad), None), Err(RankError::Malformed), "{bad}");
            assert_eq!(between(None, Some(bad)), Err(RankError::Malformed), "{bad}");
        }
        let long = "1".repeat(MAX_RANK_LEN + 1);
        assert_eq!(between(Some(&long), None), Err(RankError::Malformed));
    }

    #[test]
    fn refuses_keys_out_of_order() {
        assert_eq!(between(Some("b"), Some("a")), Err(RankError::Order));
        assert_eq!(between(Some("b"), Some("b")), Err(RankError::Order));
        // a prefix sorts first
        assert_eq!(between(Some("11"), Some("1")), Err(RankError::Order));
        assert!(between(Some("1"), Some("11")).is_ok());
    }

    #[test]
    fn a_key_at_the_cap_has_no_room_below() {
        let full = format!("{}1", "0".repeat(MAX_RANK_LEN - 1));
        assert!(is_valid_rank(&full));
        // a key before it needs one place more
        assert_eq!(between(None, Some(&full)), Err(RankError::TooLong));
        // between it and its neighbour above there is room
        assert!(between(Some(&full), Some("1")).is_ok());
    }

    #[test]
    fn spread_gives_increasing_valid_keys_with_room_between() {
        for n in [0, 1, 2, 35, 36, 37, 1000, 50_000] {
            let keys = spread(n);
            assert_eq!(keys.len(), n);
            for k in &keys {
                assert!(is_valid_rank(k), "{k}");
            }
            for pair in keys.windows(2) {
                assert!(pair[0] < pair[1], "{pair:?}");
                // and there is a key between, short of the cap
                assert!(between(Some(&pair[0]), Some(&pair[1])).is_ok());
            }
            if let (Some(first), Some(last)) = (keys.first(), keys.last()) {
                assert!(between(None, Some(first)).is_ok());
                assert!(between(Some(last), None).is_ok());
            }
        }
    }

    /// A store that re-spreads when [`between`] says `TooLong`, as the stores do: ten thousand
    /// prepends never produce a key past the cap, never lose the order, and re-spread only now and
    /// then.
    #[test]
    fn ten_thousand_prepends_stay_bounded() {
        let mut keys: Vec<String> = Vec::new();
        let (mut longest, mut spreads) = (0, 0);
        for _ in 0..10_000 {
            let first = keys.first().cloned();
            let next = match between(None, first.as_deref()) {
                Ok(k) => k,
                Err(RankError::TooLong) => {
                    spreads += 1;
                    let fresh = spread(keys.len());
                    keys = fresh;
                    between(None, keys.first().map(String::as_str)).unwrap()
                }
                Err(e) => panic!("{e}"),
            };
            longest = longest.max(next.len());
            keys.insert(0, next);
        }
        assert!(longest <= MAX_RANK_LEN, "{longest}");
        assert!(keys.windows(2).all(|w| w[0] < w[1]), "order lost");
        // A re-spread is a rewrite of the whole list, so it must stay rare: after one, a few
        // thousand prepends fit before the next.
        assert!(spreads <= 6, "{spreads} re-spreads in 10000 prepends");
    }

    #[test]
    fn ten_thousand_appends_stay_bounded() {
        let mut last: Option<String> = None;
        let mut longest = 0;
        let mut spreads = 0;
        let mut all: Vec<String> = Vec::new();
        for _ in 0..10_000 {
            let next = match between(last.as_deref(), None) {
                Ok(k) => k,
                Err(RankError::TooLong) => {
                    spreads += 1;
                    all = spread(all.len());
                    between(all.last().map(String::as_str), None).unwrap()
                }
                Err(e) => panic!("{e}"),
            };
            longest = longest.max(next.len());
            last = Some(next.clone());
            all.push(next);
        }
        assert!(longest <= MAX_RANK_LEN);
        assert!(all.windows(2).all(|w| w[0] < w[1]));
        assert!(spreads <= 6, "{spreads}");
    }
}
