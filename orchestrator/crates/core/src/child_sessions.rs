//! What a live card says under `child-sessions/v1` (ADR 0072, `docs/api/child-sessions-v1.md`):
//! the helpers a session can be started with, and how many may be open at once in one thread.
//!
//! A card lists the extension with `params: {agents, maxOpen}`, read here as a
//! [`ChildSessionsOffer`]. Only well-formed `params` are an offer: a card that lists the URI badly
//! offers nothing (ADR 0008, fail closed).

use serde::Serialize;
use serde_json::Value;

/// The URI of `child-sessions/v1`: helpers an agent runs as sessions a client can open, talk to
/// and close (ADR 0072).
pub const CHILD_SESSIONS_EXTENSION: &str =
    "https://agents.vymalo.com/a2a/extensions/child-sessions/v1";

/// One helper a session can be started with: the agent's own name and description (untrusted
/// card text, drawn as text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChildAgent {
    /// The helper's name, as a `start` names it; never empty.
    pub name: String,
    /// What the helper does, for the person.
    pub description: String,
}

/// The `params` of a card's `child-sessions/v1` entry, well formed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionsOffer {
    agents: Vec<ChildAgent>,
    max_open: u32,
}

impl ChildSessionsOffer {
    /// Reads the `params` of the card's entry: `agents`, a non-empty array of `{name,
    /// description}` (both strings, `name` not blank and each name once), and `maxOpen`, a whole
    /// number of at least 1. Unknown members are ignored. `None` for anything else.
    pub fn from_params(params: &Value) -> Option<ChildSessionsOffer> {
        let listed = params.get("agents")?.as_array()?;
        let mut agents: Vec<ChildAgent> = Vec::with_capacity(listed.len());
        for entry in listed {
            let name = entry.get("name")?.as_str()?;
            let description = entry.get("description")?.as_str()?;
            if name.trim().is_empty() || agents.iter().any(|a| a.name == name) {
                return None;
            }
            agents.push(ChildAgent {
                name: name.to_owned(),
                description: description.to_owned(),
            });
        }
        if agents.is_empty() {
            return None;
        }
        let max_open = whole_at_least_one(params.get("maxOpen")?)?;
        Some(ChildSessionsOffer { agents, max_open })
    }

    /// The helpers, in the card's order.
    pub fn agents(&self) -> &[ChildAgent] {
        &self.agents
    }

    /// The most sessions open at once in one parent context.
    pub fn max_open(&self) -> u32 {
        self.max_open
    }

    /// Whether a session can be started with the helper `name`.
    pub fn offers_agent(&self, name: &str) -> bool {
        self.agents.iter().any(|a| a.name == name)
    }
}

/// A JSON number that is a whole number in `1..=u32::MAX`; a card's numbers may come back as
/// doubles from an SDK, so `16.0` counts as 16.
fn whole_at_least_one(value: &Value) -> Option<u32> {
    let n = match value.as_u64() {
        Some(n) => n,
        None => {
            let f = value.as_f64()?;
            if f.fract() != 0.0 || f < 1.0 || f > f64::from(u32::MAX) {
                return None;
            }
            // whole and in range, checked above
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let n = f as u64;
            n
        }
    };
    u32::try_from(n).ok().filter(|n| *n >= 1)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn good() -> Value {
        json!({
            "agents": [
                {"name": "planner", "description": "Breaks a request into steps."},
                {"name": "researcher", "description": ""}
            ],
            "maxOpen": 16,
            "later": "ignored"
        })
    }

    #[test]
    fn well_formed_params_are_an_offer() {
        let offer = ChildSessionsOffer::from_params(&good()).unwrap();
        assert_eq!(offer.max_open(), 16);
        let names: Vec<_> = offer.agents().iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["planner", "researcher"]);
        assert_eq!(
            offer.agents()[0].description,
            "Breaks a request into steps."
        );
        assert!(offer.offers_agent("planner"));
        assert!(!offer.offers_agent("Planner"));
        assert!(!offer.offers_agent("writer"));
        // a name is matched exactly: no trimming, no case folding
        assert!(!offer.offers_agent(" planner"));
    }

    #[test]
    fn it_serialises_as_the_capabilities_document_has_it() {
        let offer = ChildSessionsOffer::from_params(&good()).unwrap();
        assert_eq!(
            serde_json::to_value(&offer).unwrap(),
            json!({
                "agents": [
                    {"name": "planner", "description": "Breaks a request into steps."},
                    {"name": "researcher", "description": ""}
                ],
                "maxOpen": 16
            })
        );
    }

    #[test]
    fn names_and_descriptions_are_kept_as_written() {
        // a name is the agent's own: not trimmed, and names that differ only in case are two names
        let offer = ChildSessionsOffer::from_params(&json!({
            "agents": [
                {"name": " planner ", "description": "  spaced  "},
                {"name": "Planner", "description": ""},
                {"name": "planner", "description": ""}
            ],
            "maxOpen": 1
        }))
        .unwrap();
        let names: Vec<_> = offer.agents().iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, [" planner ", "Planner", "planner"]);
        assert_eq!(offer.agents()[0].description, "  spaced  ");
        assert_eq!(offer.max_open(), 1);
    }

    #[test]
    fn a_whole_double_is_a_whole_number() {
        let mut params = good();
        params["maxOpen"] = json!(4.0);
        assert_eq!(
            ChildSessionsOffer::from_params(&params).unwrap().max_open(),
            4
        );
        params["maxOpen"] = json!(1);
        assert_eq!(
            ChildSessionsOffer::from_params(&params).unwrap().max_open(),
            1
        );
    }

    #[test]
    fn bad_params_are_no_offer() {
        let bad: Vec<(&str, Value)> = vec![
            ("not an object", json!([])),
            ("no agents", json!({"maxOpen": 16})),
            ("agents not a list", json!({"agents": {}, "maxOpen": 16})),
            ("no agent", json!({"agents": [], "maxOpen": 16})),
            (
                "a name twice",
                json!({"agents": [{"name": "a", "description": ""}, {"name": "a", "description": "x"}], "maxOpen": 16}),
            ),
            (
                "a blank name",
                json!({"agents": [{"name": " ", "description": ""}], "maxOpen": 16}),
            ),
            (
                "a name that is not text",
                json!({"agents": [{"name": 3, "description": ""}], "maxOpen": 16}),
            ),
            (
                "no description",
                json!({"agents": [{"name": "a"}], "maxOpen": 16}),
            ),
            (
                "an entry that is not an object",
                json!({"agents": ["a"], "maxOpen": 16}),
            ),
            (
                "no maxOpen",
                json!({"agents": [{"name": "a", "description": ""}]}),
            ),
            (
                "maxOpen zero",
                json!({"agents": [{"name": "a", "description": ""}], "maxOpen": 0}),
            ),
            (
                "maxOpen negative",
                json!({"agents": [{"name": "a", "description": ""}], "maxOpen": -1}),
            ),
            (
                "maxOpen a fraction",
                json!({"agents": [{"name": "a", "description": ""}], "maxOpen": 1.5}),
            ),
            (
                "maxOpen as text",
                json!({"agents": [{"name": "a", "description": ""}], "maxOpen": "16"}),
            ),
            (
                "maxOpen too large",
                json!({"agents": [{"name": "a", "description": ""}], "maxOpen": 4_294_967_296_u64}),
            ),
        ];
        for (why, params) in bad {
            assert_eq!(ChildSessionsOffer::from_params(&params), None, "{why}");
        }
    }
}
