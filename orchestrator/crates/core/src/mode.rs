//! An agent's mode of working (ADR 0109, `agent-mode/v1`, [`docs/api/agent-mode-v1.md`](../../../../docs/api/agent-mode-v1.md)).
//!
//! A mode is a way of working the **agent** defines (adam's coder: `build`, `plan`): a prompt,
//! tools and rules of its own, over the agent's. It is not a limit (that is the access tier,
//! [`crate::AccessTier`]) and not a model choice ([`crate::ModelPick`]). The orchestrator reads the
//! live card's `params {modes: [{name, title, description}], default}`, checks the name a person
//! picked against it, records the resolved name on the `user_message` and sends it under its URI.
//! Only the name is ever stored; a mode's prompt never leaves the agent.
//!
//! A name is any non-blank string of at most [`MAX_MODE_NAME_BYTES`] bytes with no control
//! character: adam requires only a non-blank name and a folder's `modes/<name>.md` file names vary,
//! so unlike a model alias the alphabet is open. Whether the card offers it is the check's job.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// The URI of `agent-mode/v1`: the way of working the agent defines, per message (ADR 0109).
pub const AGENT_MODE_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/agent-mode/v1";

/// The longest name a mode may have, in bytes.
pub const MAX_MODE_NAME_BYTES: usize = 64;

/// Why a string is not a mode name. The message is the one a **400** carries.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("a mode must be 1 to 64 bytes, not blank, with no control character")]
pub struct ModeNameError;

/// The name of a mode (`build`, `plan`): any non-blank string of at most
/// [`MAX_MODE_NAME_BYTES`] bytes with no control character. It is the word that travels; the
/// mode's title and description are for the person and never stored.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModeName(String);

impl ModeName {
    /// The name `name`, when it is well formed.
    ///
    /// # Errors
    /// [`ModeNameError`] for an empty, blank, long or control-carrying name.
    pub fn parse(name: &str) -> Result<ModeName, ModeNameError> {
        let ok = !name.trim().is_empty()
            && name.len() <= MAX_MODE_NAME_BYTES
            && !name.chars().any(char::is_control);
        ok.then(|| ModeName(name.to_owned())).ok_or(ModeNameError)
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
    type Error = ModeNameError;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        ModeName::parse(&name)
    }
}

impl From<ModeName> for String {
    fn from(name: ModeName) -> Self {
        name.0
    }
}

/// One mode an agent offers: its name, the title a person reads, and what it does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeChoice {
    /// The name a message sends.
    pub name: ModeName,
    /// The name a person reads; the name itself when the card gave none.
    pub title: String,
    /// What the mode does, for a person; empty when the card gave none.
    pub description: String,
}

/// What a live card says under `agent-mode/v1` (`params`): the modes the agent defines and the one
/// it works under when a message names none.
///
/// Only a well-formed `params` is an offer: a card with bad `params` is a card that does not list
/// the extension (ADR 0008, fail closed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeOffer {
    modes: Vec<ModeChoice>,
    default: ModeName,
}

impl ModeOffer {
    /// Reads the `params` of the card's entry: `modes`, a non-empty array of objects with a valid
    /// `name` (each once) and optional `title` and `description`, and `default`, one of the names.
    /// Unknown members are ignored. `None` for anything else.
    pub fn from_params(params: &Value) -> Option<ModeOffer> {
        let listed = params.get("modes")?.as_array()?;
        let mut modes: Vec<ModeChoice> = Vec::with_capacity(listed.len());
        for item in listed {
            let name = ModeName::parse(item.get("name")?.as_str()?).ok()?;
            if modes.iter().any(|m| m.name == name) {
                return None;
            }
            let title = match item.get("title") {
                None | Some(Value::Null) => name.as_str().to_owned(),
                Some(title) => title.as_str()?.to_owned(),
            };
            let description = match item.get("description") {
                None | Some(Value::Null) => String::new(),
                Some(description) => description.as_str()?.to_owned(),
            };
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

    /// The modes the agent defines, in the card's order.
    pub fn modes(&self) -> &[ModeChoice] {
        &self.modes
    }

    /// What the agent works under when a message names no mode.
    pub fn default_mode(&self) -> &ModeName {
        &self.default
    }

    /// Whether the agent defines `mode`.
    pub fn offers(&self, mode: &ModeName) -> bool {
        self.modes.iter().any(|m| &m.name == mode)
    }

    /// The names, in order.
    pub fn names(&self) -> Vec<String> {
        self.modes.iter().map(|m| m.name.to_string()).collect()
    }
}

/// What a live card says about `agent-mode/v1`, read for the call it is read for and never kept
/// (ADR 0008).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModeListing {
    /// The card does not list the extension: the agent is plain A2A and a message to it carries no
    /// mode.
    #[default]
    Absent,
    /// The card lists the extension with `params` that are not `{modes, default}` as the contract
    /// has them. Nothing can be said to such an agent about a mode (fail closed, ADR 0008), so a
    /// message that names one is refused; a message that names none is plain.
    Malformed,
    /// The card lists the extension, well formed.
    Offered(ModeOffer),
}

impl ModeListing {
    /// The offer, when the card has a well-formed one.
    pub fn offer(&self) -> Option<&ModeOffer> {
        match self {
            ModeListing::Offered(offer) => Some(offer),
            ModeListing::Absent | ModeListing::Malformed => None,
        }
    }

    /// Checks that the card offers `mode`. Fails **closed**: a card that lists the extension not
    /// at all, or badly, refuses every mode, with an empty offer.
    ///
    /// # Errors
    /// [`ModeRefusal`] naming the mode and the names the card offers (empty when it lists none).
    pub fn check(&self, mode: &ModeName) -> Result<(), ModeRefusal> {
        let offer = self.offer();
        if offer.is_some_and(|offer| offer.offers(mode)) {
            Ok(())
        } else {
            Err(ModeRefusal {
                picked: mode.clone(),
                offered: offer.map(ModeOffer::names).unwrap_or_default(),
            })
        }
    }
}

/// A mode the person picked that the live card of the agent does not offer (ADR 0109): refused
/// before anything is written, never silently replaced by the agent's default.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("this agent does not offer the mode '{picked}'")]
pub struct ModeRefusal {
    /// The mode that was picked.
    pub picked: ModeName,
    /// The names the card offers (empty when it lists none).
    pub offered: Vec<String>,
}

impl ModeRefusal {
    /// The stable code a client acts on.
    pub fn code(&self) -> &'static str {
        "mode_not_offered"
    }

    /// What the card offers.
    pub fn offered(&self) -> &[String] {
        &self.offered
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_mode_name_is_any_non_blank_string_with_no_control_character() {
        for good in ["build", "plan", "a b", "üñî", &"a".repeat(64)] {
            assert!(ModeName::parse(good).is_ok(), "{good:?}");
        }
        for bad in ["", "   ", "\t", "a\nb", "a\u{7f}b", &"a".repeat(65)] {
            assert!(ModeName::parse(bad).is_err(), "{bad:?}");
        }
        assert_eq!(
            serde_json::to_value(ModeName::parse("build").unwrap()).unwrap(),
            json!("build")
        );
        assert!(serde_json::from_value::<ModeName>(json!("a\nb")).is_err());
    }

    #[test]
    fn an_offer_is_read_from_well_formed_params() {
        let offer = ModeOffer::from_params(&json!({
            "modes": [
                {"name": "build", "title": "Build", "description": "Make the change."},
                {"name": "plan"}
            ],
            "default": "build",
            "x": 1
        }))
        .unwrap();
        assert_eq!(offer.names(), ["build", "plan"]);
        assert_eq!(offer.default_mode().as_str(), "build");
        assert!(offer.offers(&ModeName::parse("plan").unwrap()));
        assert_eq!(offer.modes()[1].title, "plan");
        assert_eq!(offer.modes()[1].description, "");
        assert_eq!(
            serde_json::to_value(&offer).unwrap(),
            json!({
                "modes": [
                    {"name": "build", "title": "Build", "description": "Make the change."},
                    {"name": "plan", "title": "plan", "description": ""}
                ],
                "default": "build"
            })
        );
    }

    #[test]
    fn params_that_are_not_well_formed_offer_nothing() {
        for bad in [
            json!(null),
            json!({}),
            json!({"default": "build"}),
            json!({"modes": [{"name": "build"}]}),
            json!({"modes": [], "default": "build"}),
            json!({"modes": "build", "default": "build"}),
            json!({"modes": [{"name": "build"}, {"name": "build"}], "default": "build"}),
            json!({"modes": [{"name": ""}], "default": ""}),
            json!({"modes": [{"name": 3}], "default": "build"}),
            json!({"modes": [{"name": "build"}], "default": "plan"}),
            json!({"modes": [{"name": "build", "title": 3}], "default": "build"}),
        ] {
            assert_eq!(ModeOffer::from_params(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_listing_refuses_a_mode_it_does_not_offer_with_the_names_it_does() {
        let offered = ModeListing::Offered(
            ModeOffer::from_params(&json!({
                "modes": [{"name": "build"}, {"name": "plan"}],
                "default": "build"
            }))
            .unwrap(),
        );
        assert_eq!(offered.check(&ModeName::parse("plan").unwrap()), Ok(()));
        let refused = offered
            .check(&ModeName::parse("yolo").unwrap())
            .unwrap_err();
        assert_eq!(refused.code(), "mode_not_offered");
        assert_eq!(refused.offered(), ["build", "plan"]);
        assert!(refused.to_string().contains("'yolo'"), "{refused}");
        for listing in [ModeListing::Absent, ModeListing::Malformed] {
            let refused = listing
                .check(&ModeName::parse("build").unwrap())
                .unwrap_err();
            assert!(refused.offered().is_empty());
        }
        assert_eq!(ModeListing::default(), ModeListing::Absent);
        assert!(offered.offer().is_some() && ModeListing::Malformed.offer().is_none());
    }
}
