//! The commands an agent serves (`commands/v1`, ADR 0079, `docs/api/commands-v1.md`): the prompt
//! templates a person invokes with `/<name>`, listed on the live card.
//!
//! A card lists the extension with `params: {commands: [{name, description, argumentHint?}]}`, read
//! here as a [`CommandsOffer`]. Only well-formed `params` are an offer: a card that lists the URI
//! badly offers nothing (ADR 0008, fail closed), and the capabilities document then carries no key.
//!
//! The core reads the offer and nothing else: the orchestrator never expands a command and never
//! sends one. The web draws its `/` menu from the offer and inserts the text a person picks; the
//! agent reads `/<name>` in the message it receives and expands it there (adam-rs
//! `adam-a2a-runtime`'s `commands`). A name is the agent's own: not a permission, not a boundary.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The URI of `commands/v1`: the prompt templates an agent serves, invoked as `/<name>` (ADR 0079).
pub const COMMANDS_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/commands/v1";

/// One command of a card's `commands/v1` entry: the agent's own name and description (untrusted
/// card text, drawn as text), and what follows the name, for a client to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSpec {
    /// The name a message invokes: `/name`. Not empty, without whitespace, and never starting with
    /// `/`.
    pub name: String,
    /// What the command does, for the person to read.
    pub description: String,
    /// What follows the name, for the composer to show as a hint (`<repo>`, a sentence); absent when
    /// the command takes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
}

/// The `params` of a card's `commands/v1` entry, well formed: the commands the agent serves, sorted
/// by name and each name once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandsOffer {
    commands: Vec<CommandSpec>,
}

impl CommandsOffer {
    /// Reads the `params` of the card's entry: `commands`, a non-empty array of `{name,
    /// description, argumentHint?}`. `name` is a non-empty string without whitespace that does not
    /// start with `/`, `description` a string that is not blank, `argumentHint` a string when
    /// present. A name listed twice keeps its **first** entry. Unknown members are ignored.
    /// `None` for anything else.
    pub fn from_params(params: &Value) -> Option<CommandsOffer> {
        let listed = params.get("commands")?.as_array()?;
        let mut commands: Vec<CommandSpec> = Vec::with_capacity(listed.len());
        for entry in listed {
            let name = entry.get("name")?.as_str()?;
            let description = entry.get("description")?.as_str()?;
            let argument_hint = match entry.get("argumentHint") {
                None | Some(Value::Null) => None,
                Some(hint) => Some(hint.as_str()?.to_owned()),
            };
            if !usable_name(name) || description.trim().is_empty() {
                return None;
            }
            if commands.iter().any(|c| c.name == name) {
                // the first entry of a name wins; a repeat adds nothing
                continue;
            }
            commands.push(CommandSpec {
                name: name.to_owned(),
                description: description.to_owned(),
                argument_hint,
            });
        }
        if commands.is_empty() {
            return None;
        }
        commands.sort_by_key(|c| c.name.clone());
        Some(CommandsOffer { commands })
    }

    /// The commands, sorted by name.
    pub fn commands(&self) -> &[CommandSpec] {
        &self.commands
    }

    /// The command called `name`, matched exactly: no trimming, no case folding.
    pub fn command(&self, name: &str) -> Option<&CommandSpec> {
        self.commands.iter().find(|c| c.name == name)
    }
}

/// Whether `name` is one a message can invoke: not empty, without whitespace, and not starting with
/// the `/` that invokes it.
fn usable_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('/') && !name.chars().any(char::is_whitespace)
}

/// What a live card says about `commands/v1`, read for the call it is read for and never kept
/// (ADR 0008).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CommandsListing {
    /// The card does not list the extension: the agent serves no command a client can offer.
    #[default]
    Absent,
    /// The card lists the extension with `params` that are not `{commands: [...]}` as the contract
    /// has them. Nothing can be offered (fail closed, ADR 0008): the capabilities document has no
    /// key, and the web draws no menu.
    Malformed,
    /// The card lists the extension, well formed.
    Offered(CommandsOffer),
}

impl CommandsListing {
    /// The offer, when the card has a well-formed one.
    pub fn offer(&self) -> Option<&CommandsOffer> {
        match self {
            CommandsListing::Offered(offer) => Some(offer),
            CommandsListing::Absent | CommandsListing::Malformed => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn good() -> Value {
        json!({
            "commands": [
                {"name": "review", "description": "Reviews a change.", "argumentHint": "<repo>"},
                {"name": "explain", "description": "Explains a file."},
                {"name": "test", "description": "Runs the tests."}
            ],
            "later": "ignored"
        })
    }

    #[test]
    fn well_formed_params_are_an_offer_sorted_by_name() {
        let offer = CommandsOffer::from_params(&good()).unwrap();
        let names: Vec<_> = offer.commands().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["explain", "review", "test"]);
        let review = offer.command("review").unwrap();
        assert_eq!(review.description, "Reviews a change.");
        assert_eq!(review.argument_hint.as_deref(), Some("<repo>"));
        assert_eq!(offer.command("explain").unwrap().argument_hint, None);
        // a name is matched exactly: no trimming, no case folding
        assert!(offer.command("Review").is_none());
        assert!(offer.command(" review").is_none());
    }

    #[test]
    fn it_serialises_as_the_capabilities_document_has_it() {
        let offer = CommandsOffer::from_params(&good()).unwrap();
        assert_eq!(
            serde_json::to_value(&offer).unwrap(),
            json!({"commands": [
                {"name": "explain", "description": "Explains a file."},
                {"name": "review", "description": "Reviews a change.", "argumentHint": "<repo>"},
                {"name": "test", "description": "Runs the tests."}
            ]})
        );
    }

    #[test]
    fn a_name_listed_twice_keeps_its_first_entry_and_the_list_is_sorted() {
        let offer = CommandsOffer::from_params(&json!({
            "commands": [
                {"name": "zeta", "description": "first zeta"},
                {"name": "alpha", "description": "only alpha"},
                {"name": "zeta", "description": "second zeta", "argumentHint": "<ignored>"}
            ]
        }))
        .unwrap();
        let names: Vec<_> = offer.commands().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["alpha", "zeta"]);
        let zeta = offer.command("zeta").unwrap();
        assert_eq!(zeta.description, "first zeta");
        assert_eq!(zeta.argument_hint, None);
    }

    #[test]
    fn an_argument_hint_that_is_null_is_no_hint() {
        let offer = CommandsOffer::from_params(&json!({
            "commands": [{"name": "a", "description": "d", "argumentHint": null}]
        }))
        .unwrap();
        assert_eq!(offer.command("a").unwrap().argument_hint, None);
    }

    #[test]
    fn a_listing_gives_the_offer_only_when_the_card_has_one() {
        let offered = CommandsListing::Offered(CommandsOffer::from_params(&good()).unwrap());
        assert!(offered.offer().is_some());
        assert_eq!(CommandsListing::default(), CommandsListing::Absent);
        assert!(CommandsListing::Absent.offer().is_none());
        assert!(CommandsListing::Malformed.offer().is_none());
    }

    #[test]
    fn params_that_are_not_well_formed_offer_nothing() {
        for bad in [
            json!(null),
            json!([]),
            json!({}),
            json!({"commands": {}}),
            json!({"commands": []}),
            json!({"commands": ["review"]}),
            json!({"commands": [{"description": "no name"}]}),
            json!({"commands": [{"name": "", "description": "d"}]}),
            json!({"commands": [{"name": 3, "description": "d"}]}),
            json!({"commands": [{"name": "two words", "description": "d"}]}),
            json!({"commands": [{"name": "/review", "description": "d"}]}),
            json!({"commands": [{"name": "review"}]}),
            json!({"commands": [{"name": "review", "description": ""}]}),
            json!({"commands": [{"name": "review", "description": "  "}]}),
            json!({"commands": [{"name": "review", "description": 1}]}),
            json!({"commands": [{"name": "review", "description": "d", "argumentHint": 2}]}),
            json!({"commands": [{"name": "ok", "description": "d"}, {"name": "", "description": "d"}]}),
        ] {
            assert_eq!(CommandsOffer::from_params(&bad), None, "{bad}");
        }
    }
}
