//! The mode a person picks for a message or a thread (ADR 0109, `agent-mode/v1`,
//! [`docs/api/agent-mode-v1.md`](../../../../docs/api/agent-mode-v1.md)).
//!
//! An agent may declare **modes** — a prompt, tools and rules of its own, over the agent's — one of
//! them the default. The card lists `params {modes: [{name, title, description}], default}` and a
//! message carries `metadata[URI] = {"mode": "<name>"}`. A pick is **never** a hint: a mode the
//! live card does not list is refused before anything is written ([`ModeListings::check`]), because
//! a person who chose "plan" must not silently get the agent's default. Everything here is pure:
//! the card is read by an adapter and handed in.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// The URI of `agent-mode/v1` (ADR 0109).
pub const MODE_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/agent-mode/v1";

/// The longest mode name, in bytes.
pub const MAX_MODE_NAME_LEN: usize = 64;

/// Why a string is not a mode name. The message is the one a **400** carries.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct ModeError(&'static str);

/// The name of a mode an agent defines: a non-blank string of at most [`MAX_MODE_NAME_LEN`] bytes
/// with no control characters. The name is deliberately **not** the model pick's alphabet: adam
/// requires only a non-blank name and a folder's `modes/<name>.md` file names vary. Whether the
/// card offers it is the `422`'s job, not this one's.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModeName(String);

impl ModeName {
    /// The name `name`, when it is well formed.
    ///
    /// # Errors
    /// [`ModeError`] for a blank, long or control-bearing name.
    pub fn parse(name: &str) -> Result<ModeName, ModeError> {
        let ok = !name.trim().is_empty()
            && name.len() <= MAX_MODE_NAME_LEN
            && !name.chars().any(char::is_control);
        ok.then(|| ModeName(name.to_owned())).ok_or(ModeError(
            "a mode must be 1 to 64 bytes, not blank and with no control character",
        ))
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ModeName {
    type Error = ModeError;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        ModeName::parse(&name)
    }
}

impl From<ModeName> for String {
    fn from(name: ModeName) -> Self {
        name.0
    }
}

/// One mode an agent offers: the name that travels, and — when the card names them — the title and
/// description a person reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeChoice {
    /// The name, which a message sends.
    pub name: ModeName,
    /// The title a screen shows, when the card gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The description a screen shows, when the card gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// What `agent-mode/v1` offers: the modes and the agent's own default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeOffer {
    /// The modes, in the card's order, without a repeated name.
    pub modes: Vec<ModeChoice>,
    /// The agent's own default, one of the modes.
    pub default: ModeName,
}

impl ModeOffer {
    /// Reads the card's `params`: `modes` is required and non-empty, each an object with a valid,
    /// distinct `name` and optional `title` and `description`; `default` is required and names one
    /// of them. `None` when it is anything else, so a card that lists the extension badly offers
    /// nothing (ADR 0109 decision 2).
    pub fn from_params(params: &Value) -> Option<ModeOffer> {
        let mut modes: Vec<ModeChoice> = Vec::new();
        for item in params.get("modes")?.as_array()? {
            let name = ModeName::parse(item.get("name")?.as_str()?).ok()?;
            if modes.iter().any(|m| m.name == name) {
                return None;
            }
            let title = text_member(item, "title")?;
            let description = text_member(item, "description")?;
            modes.push(ModeChoice {
                name,
                title,
                description,
            });
        }
        if modes.is_empty() {
            return None;
        }
        let default = ModeName::parse(params.get("default")?.as_str()?).ok()?;
        modes
            .iter()
            .any(|m| m.name == default)
            .then_some(ModeOffer { modes, default })
    }

    /// Whether the agent offers `name`.
    pub fn offers(&self, name: &ModeName) -> bool {
        self.modes.iter().any(|m| &m.name == name)
    }

    /// The names, in order.
    pub fn names(&self) -> Vec<String> {
        self.modes.iter().map(|m| m.name.to_string()).collect()
    }
}

/// A `title` or `description` member: absent or `null` is none, a string is it, anything else
/// makes the card malformed.
fn text_member(item: &Value, key: &str) -> Option<Option<String>> {
    match item.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some(Some(text.clone())),
        Some(_) => None,
    }
}

/// Why a mode cannot be sent to an agent: the name, and the modes the card offers (empty when it
/// lists the extension not at all, or badly).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("this agent does not offer the mode '{picked}'")]
pub struct ModeRefusal {
    /// The name that was picked.
    pub picked: String,
    /// The modes the card lists.
    pub offered: Vec<String>,
}

impl ModeRefusal {
    /// The stable code a client acts on, aligned with adam's own name for its `InvalidParams`.
    pub fn code(&self) -> &'static str {
        "mode_not_offered"
    }

    /// What the card offers.
    pub fn offered(&self) -> &[String] {
        &self.offered
    }
}

/// What a card says about `agent-mode/v1`.
pub type ModeListing = crate::Listing<ModeOffer>;

/// Checks `mode` against the card: it must be listed and offered. Fails **closed**: a card that
/// lists the extension not at all, or badly, refuses any mode.
///
/// # Errors
/// [`ModeRefusal`] naming the mode and what the card offers.
pub fn check(listing: &ModeListing, mode: Option<&ModeName>) -> Result<(), ModeRefusal> {
    let Some(mode) = mode else {
        return Ok(());
    };
    let offer = listing.offer();
    if offer.is_some_and(|o| o.offers(mode)) {
        return Ok(());
    }
    Err(ModeRefusal {
        picked: mode.to_string(),
        offered: offer.map(ModeOffer::names).unwrap_or_default(),
    })
}

/// What an agent's card lists for a mode, as a screen reads it (`GET /api/agents`): the modes and
/// which one is the agent's default. Nothing when the card lists none. It is a listing, never a
/// check: a request that names something else is still refused by [`check`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMode {
    /// The modes the card offers, in its order.
    pub modes: Vec<ModeChoice>,
    /// The agent's own default.
    pub default: String,
}

impl AgentMode {
    /// The listing a screen reads, or `None` when the card lists the extension not at all or badly.
    #[must_use]
    pub fn of(listing: &ModeListing) -> Option<AgentMode> {
        listing.offer().map(|offer| AgentMode {
            modes: offer.modes.clone(),
            default: offer.default.to_string(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn offer() -> ModeOffer {
        ModeOffer::from_params(&json!({
            "modes": [
                {"name": "build", "title": "Build", "description": "Make the change."},
                {"name": "plan", "title": "Plan", "description": "Read and plan."}
            ],
            "default": "build"
        }))
        .unwrap()
    }

    #[test]
    fn a_mode_name_is_non_blank_and_bounded_and_has_no_control_character() {
        for good in ["build", "plan", "a b", "plan-2", "é"] {
            assert!(ModeName::parse(good).is_ok(), "{good}");
        }
        for bad in ["", "   ", "\t", "a\nb", "a\u{0}b", &"a".repeat(65)] {
            assert!(ModeName::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_card_is_read_with_its_modes_and_default() {
        let offer = offer();
        assert_eq!(offer.names(), ["build", "plan"]);
        assert_eq!(offer.default.as_str(), "build");
        assert_eq!(offer.modes[1].title.as_deref(), Some("Plan"));
    }

    #[test]
    fn params_that_cannot_be_read_offer_nothing() {
        for bad in [
            json!({}),
            json!({"modes": [], "default": "build"}),
            json!({"modes": [{"name": "build"}]}),
            json!({"modes": [{"name": ""}], "default": "build"}),
            json!({"modes": [{"name": "a"}, {"name": "a"}], "default": "a"}),
            json!({"modes": [{"name": "a"}], "default": "b"}),
            json!({"modes": [{"name": "a", "title": 3}], "default": "a"}),
            json!({"modes": "a", "default": "a"}),
        ] {
            assert!(ModeOffer::from_params(&bad).is_none(), "{bad}");
        }
        // a title and a description are optional
        assert!(
            ModeOffer::from_params(&json!({"modes": [{"name": "a"}], "default": "a"})).is_some()
        );
    }

    #[test]
    fn a_mode_the_card_lists_is_accepted_and_one_it_does_not_is_refused_with_the_offer() {
        let listing: ModeListing = crate::Listing::Offered(offer());
        let plan = ModeName::parse("plan").unwrap();
        assert_eq!(check(&listing, Some(&plan)), Ok(()));
        assert_eq!(check(&listing, None), Ok(()));
        let refusal = check(&listing, Some(&ModeName::parse("review").unwrap())).unwrap_err();
        assert_eq!(refusal.code(), "mode_not_offered");
        assert_eq!(refusal.offered(), ["build", "plan"]);
        assert_eq!(
            refusal.to_string(),
            "this agent does not offer the mode 'review'"
        );
    }

    #[test]
    fn a_card_that_lists_nothing_refuses_a_mode_with_an_empty_offer() {
        for listing in [ModeListing::Absent, ModeListing::Malformed] {
            let refusal = check(&listing, Some(&ModeName::parse("plan").unwrap())).unwrap_err();
            assert!(refusal.offered().is_empty());
            assert_eq!(check(&listing, None), Ok(()));
        }
    }

    #[test]
    fn a_mode_serialises_as_its_name_and_the_listing_as_camel_case() {
        let name = ModeName::parse("plan").unwrap();
        assert_eq!(serde_json::to_value(&name).unwrap(), json!("plan"));
        assert!(serde_json::from_value::<ModeName>(json!("  ")).is_err());
        let listing = AgentMode::of(&ModeListing::Offered(offer())).unwrap();
        assert_eq!(
            serde_json::to_value(&listing).unwrap(),
            json!({
                "modes": [
                    {"name": "build", "title": "Build", "description": "Make the change."},
                    {"name": "plan", "title": "Plan", "description": "Read and plan."}
                ],
                "default": "build"
            })
        );
        assert!(AgentMode::of(&ModeListing::Absent).is_none());
    }
}
