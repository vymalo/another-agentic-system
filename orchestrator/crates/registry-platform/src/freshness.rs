//! How long a registry answer may be used (RFC 9111, as the registry contract narrows it): pure,
//! headers in, a lifetime and a validator out.
//!
//! - `max-age`, less `Age`, is how long a copy is fresh, never more than the cap the deployment
//!   sets (`AGENT_REGISTRY_MAX_AGE_SECS`).
//! - No `max-age`, `no-cache`, `no-store`, or a `max-age` or `Age` that cannot be read (or a
//!   `max-age` given twice with two values): the copy is not fresh at all, and every read asks the
//!   registry again.
//! - `stale-while-revalidate`, `stale-if-error` and `s-maxage` are not read: a stale copy is never
//!   served, whatever the registry allows (the contract: an agent that was removed must not stay
//!   selectable because the registry is down).
//! - The validator is the `ETag` when there is one, else `Last-Modified`.

use std::time::Duration;

/// The caching headers of a response, as text (`Cache-Control` joined by commas when it came as
/// several fields).
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheHeaders<'a> {
    /// `Cache-Control`.
    pub cache_control: Option<&'a str>,
    /// `Age`.
    pub age: Option<&'a str>,
    /// `ETag`.
    pub etag: Option<&'a str>,
    /// `Last-Modified`.
    pub last_modified: Option<&'a str>,
}

/// What asks the registry whether a copy is still good.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validator {
    /// Sent as `If-None-Match`.
    ETag(String),
    /// Sent as `If-Modified-Since`.
    LastModified(String),
}

/// What a response allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// How long the copy is fresh from now; zero means every read asks again.
    pub lifetime: Duration,
    /// How to ask whether the copy is still good once it is not fresh.
    pub validator: Option<Validator>,
}

/// The largest `max-age` that is read: RFC 9111 §1.2.2 lets a cache treat a larger one as 2^31.
const LARGEST_DELTA_SECONDS: u64 = 2_147_483_648;

/// What `headers` allow, at most `cap`.
pub fn policy(headers: &CacheHeaders<'_>, cap: Duration) -> Policy {
    Policy {
        lifetime: lifetime(headers).min(cap),
        validator: validator(headers),
    }
}

/// Whether a `304` carries caching headers of its own (then it renews the copy with those, else
/// with what the copy had).
pub fn states_freshness(headers: &CacheHeaders<'_>) -> bool {
    headers.cache_control.is_some()
}

fn lifetime(headers: &CacheHeaders<'_>) -> Duration {
    let Some(cache_control) = headers.cache_control else {
        return Duration::ZERO;
    };
    let mut max_age: Option<u64> = None;
    for directive in directives(cache_control) {
        let (name, value) = match directive.split_once('=') {
            Some((name, value)) => (name.trim(), Some(value.trim().trim_matches('"'))),
            None => (directive.trim(), None),
        };
        match name.to_ascii_lowercase().as_str() {
            "no-store" | "no-cache" => return Duration::ZERO,
            "max-age" => {
                let Some(seconds) = value.and_then(delta_seconds) else {
                    return Duration::ZERO;
                };
                match max_age {
                    // The same directive twice with two values: no way to say which was meant.
                    Some(earlier) if earlier != seconds => return Duration::ZERO,
                    _ => max_age = Some(seconds),
                }
            }
            _ => {}
        }
    }
    let Some(max_age) = max_age else {
        return Duration::ZERO;
    };
    let age = match headers.age {
        None => 0,
        Some(age) => match delta_seconds(age.trim()) {
            Some(age) => age,
            None => return Duration::ZERO,
        },
    };
    Duration::from_secs(max_age.saturating_sub(age))
}

/// The directives of a `Cache-Control` value: split at the commas that are not inside a quoted
/// string.
fn directives(value: &str) -> impl Iterator<Item = &str> {
    let mut rest = Some(value);
    std::iter::from_fn(move || {
        let text = rest?;
        let mut quoted = false;
        let mut end = None;
        for (i, c) in text.char_indices() {
            match c {
                '"' => quoted = !quoted,
                ',' if !quoted => {
                    end = Some(i);
                    break;
                }
                _ => {}
            }
        }
        let (directive, remainder) = match end {
            Some(i) => (&text[..i], Some(&text[i + 1..])),
            None => (text, None),
        };
        rest = remainder;
        Some(directive)
    })
    .filter(|d| !d.trim().is_empty())
}

/// `1*DIGIT` as delta-seconds; a number too large for a `u64` is the largest there is.
fn delta_seconds(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(
        text.parse::<u64>()
            .map_or(LARGEST_DELTA_SECONDS, |n| n.min(LARGEST_DELTA_SECONDS)),
    )
}

fn validator(headers: &CacheHeaders<'_>) -> Option<Validator> {
    let given = |v: Option<&str>| {
        v.map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    given(headers.etag)
        .map(Validator::ETag)
        .or_else(|| given(headers.last_modified).map(Validator::LastModified))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: Duration = Duration::from_secs(60);

    fn with(cache_control: &str) -> CacheHeaders<'_> {
        CacheHeaders {
            cache_control: Some(cache_control),
            ..CacheHeaders::default()
        }
    }

    fn secs(headers: &CacheHeaders<'_>) -> u64 {
        policy(headers, CAP).lifetime.as_secs()
    }

    #[test]
    fn max_age_is_the_lifetime() {
        assert_eq!(secs(&with("max-age=30")), 30);
        assert_eq!(secs(&with("private, max-age=30")), 30);
        assert_eq!(secs(&with("MAX-AGE=30 , Private")), 30);
        assert_eq!(secs(&with("max-age=\"30\"")), 30);
        assert_eq!(secs(&with("max-age=0")), 0);
    }

    #[test]
    fn the_cap_bounds_what_the_registry_allows() {
        assert_eq!(secs(&with("max-age=3600")), 60);
        assert_eq!(
            policy(&with("max-age=3600"), Duration::from_secs(5)).lifetime,
            Duration::from_secs(5)
        );
        assert_eq!(secs(&with("max-age=99999999999999999999999")), 60);
    }

    #[test]
    fn age_is_taken_off_and_never_below_zero() {
        let mut h = with("max-age=30");
        h.age = Some("10");
        assert_eq!(secs(&h), 20);
        h.age = Some("30");
        assert_eq!(secs(&h), 0);
        h.age = Some("900");
        assert_eq!(secs(&h), 0);
        h.age = Some("soon");
        assert_eq!(
            secs(&h),
            0,
            "an Age that cannot be read leaves nothing fresh"
        );
    }

    #[test]
    fn without_a_max_age_every_read_asks_again() {
        assert_eq!(secs(&CacheHeaders::default()), 0);
        assert_eq!(secs(&with("private")), 0);
        assert_eq!(secs(&with("")), 0);
        assert_eq!(secs(&with("max-age")), 0);
        assert_eq!(secs(&with("max-age=")), 0);
        assert_eq!(secs(&with("max-age=-1")), 0);
        assert_eq!(secs(&with("max-age=1.5")), 0);
        assert_eq!(secs(&with("max-age=ten")), 0);
        // s-maxage is for shared caches, and the registry's client is none.
        assert_eq!(secs(&with("s-maxage=30")), 0);
    }

    #[test]
    fn no_cache_and_no_store_win_over_a_max_age() {
        assert_eq!(secs(&with("max-age=30, no-store")), 0);
        assert_eq!(secs(&with("no-cache, max-age=30")), 0);
        assert_eq!(secs(&with("max-age=30, no-cache=\"Set-Cookie, x\"")), 0);
        assert_eq!(secs(&with("max-age=30, No-Store")), 0);
    }

    #[test]
    fn stale_if_error_and_stale_while_revalidate_buy_nothing() {
        assert_eq!(secs(&with("max-age=0, stale-if-error=600")), 0);
        assert_eq!(secs(&with("max-age=0, stale-while-revalidate=600")), 0);
        assert_eq!(secs(&with("max-age=10, stale-if-error=600")), 10);
    }

    #[test]
    fn a_max_age_given_twice_is_trusted_only_when_it_agrees() {
        assert_eq!(secs(&with("max-age=30, max-age=30")), 30);
        assert_eq!(secs(&with("max-age=30, max-age=20")), 0);
    }

    #[test]
    fn a_comma_inside_a_quoted_string_does_not_split_a_directive() {
        assert_eq!(secs(&with("private=\"a, max-age=5\", max-age=30")), 30);
    }

    #[test]
    fn the_validator_is_the_etag_else_last_modified() {
        let etag = "\"r-2026-10-01T09:00:00Z\"";
        let modified = "Wed, 01 Oct 2026 09:00:00 GMT";
        let both = CacheHeaders {
            etag: Some(etag),
            last_modified: Some(modified),
            ..CacheHeaders::default()
        };
        assert_eq!(
            policy(&both, CAP).validator,
            Some(Validator::ETag(etag.into()))
        );
        let only_date = CacheHeaders {
            last_modified: Some(modified),
            ..CacheHeaders::default()
        };
        assert_eq!(
            policy(&only_date, CAP).validator,
            Some(Validator::LastModified(modified.into()))
        );
        let weak = CacheHeaders {
            etag: Some("W/\"x\""),
            ..CacheHeaders::default()
        };
        assert_eq!(
            policy(&weak, CAP).validator,
            Some(Validator::ETag("W/\"x\"".into()))
        );
        assert_eq!(policy(&CacheHeaders::default(), CAP).validator, None);
        let blank = CacheHeaders {
            etag: Some("  "),
            ..CacheHeaders::default()
        };
        assert_eq!(policy(&blank, CAP).validator, None);
    }

    #[test]
    fn a_not_modified_states_freshness_only_when_it_says_so() {
        assert!(states_freshness(&with("max-age=30")));
        assert!(!states_freshness(&CacheHeaders::default()));
    }
}
