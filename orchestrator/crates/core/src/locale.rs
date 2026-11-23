//! The interface locale a client sends with a message (`locale/v1`, ADR 0085,
//! `docs/api/locale-v1.md`): the language of the person's screen, as a BCP 47 tag, carried as a
//! **hint** and never enforced.
//!
//! The orchestrator decides nothing with it: it checks the tag's shape ([`LocaleHint::parse`],
//! **400** for a malformed one), keeps it on the outbox row the commit writes (the job ledger,
//! never the event log), and the adapter activates the extension on the message to an agent whose
//! live card lists it. An agent without the extension gets the plain A2A message it always got
//! (ADR 0008), and a message that names no locale carries no member at all.
//!
//! The set of tags is **not** a closed list of this build. The languages an interface offers are
//! the deployment's configuration (ADR 0085), so the core validates the *shape* of a tag and
//! refuses anything that is not one: a tag is data, not a protocol variant (ADR 0004).

use std::fmt;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// The URI of `locale/v1`: the interface locale a client sends with a message, as a hint
/// (ADR 0085, `docs/api/locale-v1.md`).
pub const LOCALE_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/locale/v1";

/// The longest tag this build accepts, in bytes. BCP 47 (RFC 5646 section 4.4.1) asks an
/// implementation to accept at least 35 characters, and the tags an interface produces are far
/// shorter.
pub const MAX_LOCALE_LEN: usize = 35;

/// Why a string is not a locale tag. The message is the one a **400** carries.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("not a BCP 47 language tag: {0}")]
pub struct LocaleError(String);

impl LocaleError {
    fn of(reason: &str) -> LocaleError {
        LocaleError(reason.to_owned())
    }
}

/// The interface locale of one message: a BCP 47 language tag (`en-GB`, `fr-FR`, `gsw-CH`,
/// `fur-IT`, `de-DE`, `zh-Hant-TW`, `es-419`), validated when it is read and carried as the client
/// wrote it.
///
/// **Accepted:** the `langtag` production of RFC 5646 (a language of 2 to 8 letters, up to three
/// three-letter extlangs, an optional script, an optional region, variants, extensions and a
/// private-use part), and a private-use-only tag (`x-twain`). Subtags are matched
/// case-insensitively: `EN-GB` is the tag `en-GB`, and [`as_str`](Self::as_str) gives it back as
/// it was written, because the client's spelling is what the agent is sent.
///
/// **Refused:** anything else, rather than passed through — an empty tag, a tag longer than
/// [`MAX_LOCALE_LEN`], a non-ASCII letter, a separator other than `-` (`en_CM`, `en CM`), an empty
/// subtag (`en--CM`, `-en`, `en-`), a subtag longer than its production allows
/// (`en-GB-abcdefghi`), a repeated variant or extension singleton, a private-use part with no
/// subtag (`en-GB-x`), a first subtag that is not a language, and the irregular grandfathered tags
/// (`i-klingon`, `en-GB-oed`), which no interface produces. A tag that is refused is a **400**:
/// the orchestrator never guesses what a client meant.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LocaleHint(String);

impl LocaleHint {
    /// The tag `tag`, when it is a well-formed BCP 47 tag (see the type's documentation for what
    /// is accepted and what is refused).
    ///
    /// # Errors
    /// [`LocaleError`] for anything that is not one.
    pub fn parse(tag: &str) -> Result<LocaleHint, LocaleError> {
        well_formed(tag)
            .then(|| LocaleHint(tag.to_owned()))
            .ok_or_else(|| LocaleError::of(tag))
    }

    /// The tag, as the client wrote it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LocaleHint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Subtags are case-insensitive (RFC 5646 section 2.1.1): `en-GB` and `EN-GB` are the same tag.
impl PartialEq for LocaleHint {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }
}

impl Eq for LocaleHint {}

impl Hash for LocaleHint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_ascii_lowercase().hash(state);
    }
}

impl TryFrom<String> for LocaleHint {
    type Error = LocaleError;

    fn try_from(tag: String) -> Result<Self, Self::Error> {
        LocaleHint::parse(&tag)
    }
}

impl From<LocaleHint> for String {
    fn from(hint: LocaleHint) -> Self {
        hint.0
    }
}

/// Whether `tag` is a well-formed BCP 47 tag: the `langtag` production of RFC 5646 section 2.1, or
/// a private-use-only tag.
fn well_formed(tag: &str) -> bool {
    if tag.is_empty() || tag.len() > MAX_LOCALE_LEN {
        return false;
    }
    let parts: Vec<&str> = tag.split('-').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_alphanumeric()))
    {
        return false;
    }
    let alpha = |p: &str| p.bytes().all(|b| b.is_ascii_alphabetic());
    // a private-use-only tag names no language and is still well formed: `x-twain`
    if parts[0].eq_ignore_ascii_case("x") {
        return private_use(&parts, 1);
    }
    // language: 2*3ALPHA with up to three 3ALPHA extlangs, 4ALPHA, or 5*8ALPHA
    let mut i = 1;
    match parts[0].len() {
        2..=3 if alpha(parts[0]) => {
            let mut extlangs = 0;
            while extlangs < 3 && i < parts.len() && parts[i].len() == 3 && alpha(parts[i]) {
                i += 1;
                extlangs += 1;
            }
        }
        4..=8 if alpha(parts[0]) => {}
        _ => return false,
    }
    // script: 4ALPHA
    if i < parts.len() && parts[i].len() == 4 && alpha(parts[i]) {
        i += 1;
    }
    // region: 2ALPHA or 3DIGIT
    if i < parts.len() {
        let p = parts[i];
        let region =
            (p.len() == 2 && alpha(p)) || (p.len() == 3 && p.bytes().all(|b| b.is_ascii_digit()));
        if region {
            i += 1;
        }
    }
    // variants: 5*8alphanum or DIGIT 3alphanum, each at most once
    let mut variants: Vec<String> = Vec::new();
    while i < parts.len() {
        let p = parts[i];
        let variant =
            (5..=8).contains(&p.len()) || (p.len() == 4 && p.as_bytes()[0].is_ascii_digit());
        if !variant {
            break;
        }
        let folded = p.to_ascii_lowercase();
        if variants.contains(&folded) {
            return false;
        }
        variants.push(folded);
        i += 1;
    }
    // extensions: a singleton that is not `x`, then one or more 2*8alphanum subtags
    let mut singletons: Vec<String> = Vec::new();
    while i < parts.len() && parts[i].len() == 1 && !parts[i].eq_ignore_ascii_case("x") {
        let singleton = parts[i].to_ascii_lowercase();
        if singletons.contains(&singleton) {
            return false;
        }
        singletons.push(singleton);
        i += 1;
        let first = i;
        while i < parts.len() && (2..=8).contains(&parts[i].len()) {
            i += 1;
        }
        if i == first {
            return false;
        }
    }
    // private use: `x` then one or more 1*8alphanum subtags
    if i < parts.len() && parts[i].eq_ignore_ascii_case("x") {
        return private_use(&parts, i + 1);
    }
    i == parts.len()
}

/// A private-use part starting at `from`: at least one subtag of 1 to 8 alphanumerics, and nothing
/// else.
fn private_use(parts: &[&str], from: usize) -> bool {
    from < parts.len() && parts[from..].iter().all(|p| (1..=8).contains(&p.len()))
}

/// What a live card says under `locale/v1` (`params`): the tag the agent writes in when a message
/// names none, when it says one.
///
/// Only a well-formed `params` is an offer: a card with a `locale` member that is not a tag is a
/// card that does not list the extension (ADR 0008, fail closed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocaleOffer {
    default: Option<LocaleHint>,
}

impl LocaleOffer {
    /// Reads the `params` of the card's entry: no `locale` member (the agent takes a hint and says
    /// nothing about its own language), or one that is a well-formed tag. Unknown members are
    /// ignored. `None` for anything else, a `locale` member that is not a tag included.
    pub fn from_params(params: &Value) -> Option<LocaleOffer> {
        let Some(value) = params.get("locale") else {
            return Some(LocaleOffer { default: None });
        };
        value
            .as_str()
            .and_then(|tag| LocaleHint::parse(tag).ok())
            .map(|default| LocaleOffer {
                default: Some(default),
            })
    }

    /// The tag the agent writes in when a message names none, when it said one.
    pub fn default_locale(&self) -> Option<&LocaleHint> {
        self.default.as_ref()
    }
}

/// What a live card says about `locale/v1`, read for the call it is read for and never kept
/// (ADR 0008).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LocaleListing {
    /// The card does not list the extension: the agent is plain A2A and a message to it carries no
    /// locale.
    #[default]
    Absent,
    /// The card lists the extension with a `locale` member that is not a BCP 47 tag. Nothing can be
    /// said to such an agent about a language (fail closed, ADR 0008), so a message to it is sent
    /// plain: a hint that cannot be delivered is dropped, never guessed at.
    Malformed,
    /// The card lists the extension, well formed.
    Offered(LocaleOffer),
}

impl LocaleListing {
    /// The offer, when the card has a well-formed one.
    pub fn offer(&self) -> Option<&LocaleOffer> {
        match self {
            LocaleListing::Offered(offer) => Some(offer),
            LocaleListing::Absent | LocaleListing::Malformed => None,
        }
    }

    /// Whether a message to this agent is sent with the extension activated: only when the card
    /// lists it well formed. A card that does not list it, or lists it badly, gets the plain A2A
    /// message.
    pub fn activates(&self) -> bool {
        matches!(self, LocaleListing::Offered(_))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_tags_a_deployment_offers_and_the_ones_a_client_brings_are_accepted() {
        for tag in [
            // the deployment's own, from day 0 (ADR 0085)
            "en-GB",
            "fr-FR",
            "gsw-CH",
            "fur-IT",
            // the plain language, and the tags of the wider world
            "en",
            "fr",
            "de-DE",
            "en-US",
            "es-419",
            "zh-Hant-TW",
            "sr-Latn-RS",
            "de-CH-1901",
            "sl-rozaj-biske",
            "en-a-bbb-x-a-ccc",
            "x-twain",
            "en-x-twain",
        ] {
            let hint = LocaleHint::parse(tag).unwrap_or_else(|e| panic!("{tag}: {e}"));
            assert_eq!(hint.as_str(), tag);
        }
    }

    #[test]
    fn a_tag_is_read_case_insensitively_and_carried_as_it_was_written() {
        let written = LocaleHint::parse("EN-GB").unwrap();
        assert_eq!(written.as_str(), "EN-GB");
        assert_eq!(written, LocaleHint::parse("en-GB").unwrap());
        assert_eq!(written.to_string(), "EN-GB");
        // the same tag hashes the same, whatever the case of its subtags
        let hashed = |h: &LocaleHint| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            h.hash(&mut hasher);
            hasher.finish()
        };
        assert_eq!(
            hashed(&written),
            hashed(&LocaleHint::parse("en-GB").unwrap())
        );
    }

    #[test]
    fn a_tag_that_is_not_one_is_refused_rather_than_passed_through() {
        for tag in [
            "",
            "e",
            "en_CM",
            "en CM",
            "en--CM",
            "-en",
            "en-",
            "en-GB-",
            "1234",
            "en-12",
            "en-GB-abcdefghi",
            "fr-é",
            "en-GB-x",
            "en-a-bbb-a-ccc",
            "en-GB-CM-CM",
            "i-klingon",
            "en-GB-oed",
            "en-GB;q=1",
            " en-GB",
            "en-GB ",
        ] {
            assert!(LocaleHint::parse(tag).is_err(), "{tag:?}");
        }
        // the bound is the one the type names, in bytes
        let long = format!("en-GB-{}", "a".repeat(MAX_LOCALE_LEN));
        assert!(LocaleHint::parse(&long).is_err());
        assert!(LocaleHint::parse(&"a".repeat(MAX_LOCALE_LEN)).is_err());
    }

    #[test]
    fn a_refusal_says_what_was_refused() {
        let error = LocaleHint::parse("en_CM").unwrap_err();
        assert_eq!(error.to_string(), "not a BCP 47 language tag: en_CM");
    }

    #[test]
    fn a_tag_is_its_json_string() {
        let hint = LocaleHint::parse("en-GB").unwrap();
        assert_eq!(serde_json::to_value(&hint).unwrap(), json!("en-GB"));
        assert_eq!(
            serde_json::from_value::<LocaleHint>(json!("fr-FR")).unwrap(),
            LocaleHint::parse("fr-FR").unwrap()
        );
        // and a stored one that is not a tag is refused when it is read back
        assert!(serde_json::from_value::<LocaleHint>(json!("en_CM")).is_err());
    }

    #[test]
    fn an_offer_is_read_from_well_formed_params() {
        let offered = LocaleOffer::from_params(&json!({"locale": "en-GB", "x": 1})).unwrap();
        assert_eq!(
            offered.default_locale().map(LocaleHint::as_str),
            Some("en-GB")
        );
        // a card that says nothing of its own language takes a hint all the same
        for quiet in [json!({}), json!({"locales": ["en-GB"]})] {
            let offered = LocaleOffer::from_params(&quiet).unwrap();
            assert_eq!(offered.default_locale(), None, "{quiet}");
        }
    }

    #[test]
    fn params_that_are_not_well_formed_offer_nothing() {
        for bad in [
            json!({"locale": "en_CM"}),
            json!({"locale": ""}),
            json!({"locale": 3}),
            json!({"locale": null}),
            json!({"locale": ["en-GB"]}),
        ] {
            assert_eq!(LocaleOffer::from_params(&bad), None, "{bad}");
        }
        assert_eq!(LocaleListing::default(), LocaleListing::Absent);
        assert!(!LocaleListing::Absent.activates());
        assert!(!LocaleListing::Malformed.activates());
        assert!(LocaleListing::Offered(LocaleOffer::from_params(&json!({})).unwrap()).activates());
        assert!(
            LocaleListing::Offered(LocaleOffer::from_params(&json!({})).unwrap())
                .offer()
                .is_some()
        );
        assert!(LocaleListing::Malformed.offer().is_none());
    }
}
