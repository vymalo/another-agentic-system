//! The access tier of a message (ADR 0061, `docs/api/access-tier-v1.md`): how much the agent may
//! do on its own for **one** message, one of four words.
//!
//! The tier is a closed list (ADR 0004), ordered by how much the agent does alone: `plan` <
//! `edits` < `auto` < `bypass`. The orchestrator enforces none of it: it checks the shape, the
//! person's cap and what the live card honours (`orch_app::tier`), keeps the choice on the
//! `user_message` and sends it with that message. A card lists the extension with
//! `params: {tiers, default}`, read here as a [`TierOffer`].

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The URI of `access-tier/v1`: how much the agent may do on its own, per message (ADR 0061).
pub const ACCESS_TIER_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/access-tier/v1";

/// What an agent may do on its own for one message. Later is more: `Plan < Edits < Auto <
/// Bypass`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessTier {
    /// Read and propose. No change to a repository or to any external system.
    Plan,
    /// Change files, commit, push branches, open pull requests. Nothing that merges, creates,
    /// renames or deletes, and no settings.
    Edits,
    /// Everything the agent does as the normal course of its work, merging and creating
    /// included. What cannot be undone is outside it.
    Auto,
    /// Never asks for permission.
    Bypass,
}

impl AccessTier {
    /// Every tier, lowest first.
    pub const ALL: [AccessTier; 4] = [
        AccessTier::Plan,
        AccessTier::Edits,
        AccessTier::Auto,
        AccessTier::Bypass,
    ];

    /// The wire spelling: lower case.
    pub const fn as_str(self) -> &'static str {
        match self {
            AccessTier::Plan => "plan",
            AccessTier::Edits => "edits",
            AccessTier::Auto => "auto",
            AccessTier::Bypass => "bypass",
        }
    }

    /// The tier spelled exactly `word` (lower case); anything else, a near miss included, is none.
    pub fn parse(word: &str) -> Option<AccessTier> {
        AccessTier::ALL.into_iter().find(|t| t.as_str() == word)
    }
}

impl fmt::Display for AccessTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a live card says under `access-tier/v1` (`params`): the tiers the agent honours, lowest
/// first and each once, and the one it works under when a message names none.
///
/// Only a well-formed `params` is an offer: a card with bad `params` is a card that does not list
/// the extension (ADR 0008, fail closed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TierOffer {
    tiers: Vec<AccessTier>,
    default: AccessTier,
}

impl TierOffer {
    /// Reads the `params` of the card's entry: `tiers`, a non-empty array of the four words, each
    /// at most once, and `default`, one of them. Unknown members are ignored. `None` for anything
    /// else.
    pub fn from_params(params: &Value) -> Option<TierOffer> {
        let listed = params.get("tiers")?.as_array()?;
        let mut tiers = Vec::with_capacity(listed.len());
        for word in listed {
            let tier = AccessTier::parse(word.as_str()?)?;
            if tiers.contains(&tier) {
                return None;
            }
            tiers.push(tier);
        }
        if tiers.is_empty() {
            return None;
        }
        tiers.sort();
        let default = AccessTier::parse(params.get("default")?.as_str()?)?;
        tiers
            .contains(&default)
            .then_some(TierOffer { tiers, default })
    }

    /// The tiers the agent honours, lowest first.
    pub fn tiers(&self) -> &[AccessTier] {
        &self.tiers
    }

    /// What the agent works under when a message names no tier.
    pub fn default_tier(&self) -> AccessTier {
        self.default
    }

    /// Whether the agent honours `tier`.
    pub fn honours(&self, tier: AccessTier) -> bool {
        self.tiers.contains(&tier)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_tiers_are_ordered_by_how_much_the_agent_does_alone() {
        assert!(AccessTier::Plan < AccessTier::Edits);
        assert!(AccessTier::Edits < AccessTier::Auto);
        assert!(AccessTier::Auto < AccessTier::Bypass);
        let mut shuffled = [
            AccessTier::Bypass,
            AccessTier::Plan,
            AccessTier::Auto,
            AccessTier::Edits,
        ];
        shuffled.sort();
        assert_eq!(shuffled, AccessTier::ALL);
    }

    #[test]
    fn a_tier_is_its_lower_case_word_and_nothing_near_it() {
        for tier in AccessTier::ALL {
            assert_eq!(AccessTier::parse(tier.as_str()), Some(tier));
            assert_eq!(serde_json::to_value(tier).unwrap(), json!(tier.as_str()));
            assert_eq!(
                serde_json::from_value::<AccessTier>(json!(tier.as_str())).unwrap(),
                tier
            );
        }
        for near in ["Plan", "PLAN", " plan", "plan ", "planning", "yolo", ""] {
            assert_eq!(AccessTier::parse(near), None, "{near:?}");
            assert!(serde_json::from_value::<AccessTier>(json!(near)).is_err());
        }
    }

    #[test]
    fn an_offer_is_read_from_well_formed_params() {
        let offer = TierOffer::from_params(
            &json!({"tiers": ["bypass", "plan", "auto", "edits"], "default": "auto", "x": 1}),
        )
        .unwrap();
        assert_eq!(offer.tiers(), AccessTier::ALL);
        assert_eq!(offer.default_tier(), AccessTier::Auto);
        assert!(offer.honours(AccessTier::Bypass));
        let read_only = TierOffer::from_params(&json!({"tiers": ["plan"], "default": "plan"}));
        let read_only = read_only.unwrap();
        assert!(read_only.honours(AccessTier::Plan) && !read_only.honours(AccessTier::Edits));
        assert_eq!(
            serde_json::to_value(&offer).unwrap(),
            json!({"tiers": ["plan", "edits", "auto", "bypass"], "default": "auto"})
        );
    }

    #[test]
    fn params_that_are_not_well_formed_offer_nothing() {
        for bad in [
            json!(null),
            json!({}),
            json!({"default": "auto"}),
            json!({"tiers": ["plan"]}),
            json!({"tiers": [], "default": "plan"}),
            json!({"tiers": "plan", "default": "plan"}),
            json!({"tiers": ["plan", "plan"], "default": "plan"}),
            json!({"tiers": ["plan", "yolo"], "default": "plan"}),
            json!({"tiers": ["plan", 3], "default": "plan"}),
            json!({"tiers": ["Plan"], "default": "plan"}),
            json!({"tiers": ["plan"], "default": "auto"}),
            json!({"tiers": ["plan"], "default": 1}),
        ] {
            assert_eq!(TierOffer::from_params(&bad), None, "{bad}");
        }
    }
}
