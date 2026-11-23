//! The model and the reasoning effort a person picks for a message or a thread (ADR 0086,
//! `model-pick/v1`, [`docs/api/model-pick-v1.md`](../../../../docs/api/model-pick-v1.md)).
//!
//! Two A2A extensions, one per axis: the model is `model-pick/v1` (the card's
//! `params {choices: [{id, label}], default?}`, a message's `metadata[URI] = {"model": id}`) and the
//! effort is adam's own `reasoning/v1` (`params {efforts, default?}`, `metadata[URI] = {"effort": level}`).
//! A pick is **never** a hint: a choice the live card does not list is refused before anything is
//! written ([`PickListings::check`]), because a person who chose "deep" must not silently get the
//! default. Everything here is pure: the card is read by an adapter and handed in.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// The URI of `model-pick/v1` (ADR 0086).
pub const MODEL_PICK_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/model-pick/v1";

/// The URI of adam's `reasoning/v1`, used as it stands (ADR 0086).
pub const REASONING_EXTENSION: &str = "https://agents.vymalo.com/a2a/extensions/reasoning/v1";

/// The longest alias of a model choice, in bytes.
pub const MAX_MODEL_ID_LEN: usize = 64;

/// Why a string is not a model alias or an effort level. The message is the one a **400** carries.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0}")]
pub struct PickError(&'static str);

/// A neutral alias the agent defines for a model (`fast`, `deep`): 1 to [`MAX_MODEL_ID_LEN`]
/// characters of `a-z0-9._-`. Never a provider model id, and it carries no price.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ModelId(String);

impl ModelId {
    /// The alias `id`, when it is well formed.
    ///
    /// # Errors
    /// [`PickError`] for an empty, long or otherwise spelled alias.
    pub fn parse(id: &str) -> Result<ModelId, PickError> {
        let ok = !id.is_empty()
            && id.len() <= MAX_MODEL_ID_LEN
            && id.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            });
        ok.then(|| ModelId(id.to_owned())).ok_or(PickError(
            "a model must be 1 to 64 characters of a-z, 0-9, '.', '_' and '-'",
        ))
    }

    /// The alias.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for ModelId {
    type Error = PickError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        ModelId::parse(&id)
    }
}

impl From<ModelId> for String {
    fn from(id: ModelId) -> Self {
        id.0
    }
}

/// A reasoning level: the closed vocabulary of adam's `reasoning/v1` (adam-rs ADR 0063).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// `none`: no reasoning.
    #[serde(rename = "none")]
    Disabled,
    /// `minimal`.
    Minimal,
    /// `low`.
    Low,
    /// `medium`.
    Medium,
    /// `high`.
    High,
    /// `xhigh`.
    Xhigh,
    /// `max`.
    Max,
}

impl ReasoningEffort {
    /// Every level, lowest first.
    pub const ALL: [ReasoningEffort; 7] = [
        ReasoningEffort::Disabled,
        ReasoningEffort::Minimal,
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
        ReasoningEffort::Xhigh,
        ReasoningEffort::Max,
    ];

    /// The level spelled as the wire has it.
    pub fn as_str(self) -> &'static str {
        match self {
            ReasoningEffort::Disabled => "none",
            ReasoningEffort::Minimal => "minimal",
            ReasoningEffort::Low => "low",
            ReasoningEffort::Medium => "medium",
            ReasoningEffort::High => "high",
            ReasoningEffort::Xhigh => "xhigh",
            ReasoningEffort::Max => "max",
        }
    }

    /// The level `word` names, spelled exactly.
    pub fn parse(word: &str) -> Option<ReasoningEffort> {
        Self::ALL.into_iter().find(|level| level.as_str() == word)
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a person picked for one message, or as a thread's default: either axis, or both.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPick {
    /// The model alias, when one was picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelId>,
    /// The reasoning level, when one was picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
}

impl ModelPick {
    /// A pick of a model alias alone.
    pub fn model(id: &str) -> Result<ModelPick, PickError> {
        Ok(ModelPick {
            model: Some(ModelId::parse(id)?),
            reasoning_effort: None,
        })
    }

    /// A pick of an effort alone.
    pub fn effort(level: ReasoningEffort) -> ModelPick {
        ModelPick {
            model: None,
            reasoning_effort: Some(level),
        }
    }

    /// Whether it names nothing.
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.reasoning_effort.is_none()
    }

    /// `Some(self)` unless it names nothing: how a pick is carried, so an empty one is never kept.
    pub fn into_option(self) -> Option<ModelPick> {
        (!self.is_empty()).then_some(self)
    }

    /// The pick a message is sent with: what it names, axis by axis, and for an axis it names
    /// nothing about, the thread's default. The message wins, then the thread, then nothing
    /// (the agent's own default).
    #[must_use]
    pub fn over(self, thread_default: &ModelPick) -> ModelPick {
        ModelPick {
            model: self.model.or_else(|| thread_default.model.clone()),
            reasoning_effort: self.reasoning_effort.or(thread_default.reasoning_effort),
        }
    }
}

/// One model an agent offers: its alias and the name a person reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelChoice {
    /// The alias.
    pub id: ModelId,
    /// The label shown to a person; the alias when the card gave none.
    pub label: String,
}

/// What `model-pick/v1` offers: the choices and, when the card says so, the agent's default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPickOffer {
    /// The choices, in the card's order, without a repeated alias.
    pub choices: Vec<ModelChoice>,
    /// The agent's own default, one of the choices.
    pub default: Option<ModelId>,
}

impl ModelPickOffer {
    /// Reads the card's `params`: `choices` is required, a list of objects with an `id` (a valid
    /// alias) and optional `label`; `default`, when present, names a choice. `None` when it is
    /// anything else, so a card that lists the extension badly offers nothing.
    pub fn from_params(params: &Value) -> Option<ModelPickOffer> {
        let mut choices: Vec<ModelChoice> = Vec::new();
        for item in params.get("choices")?.as_array()? {
            let id = ModelId::parse(item.get("id")?.as_str()?).ok()?;
            let label = match item.get("label") {
                None | Some(Value::Null) => id.as_str().to_owned(),
                Some(label) => label.as_str()?.to_owned(),
            };
            if choices.iter().any(|c| c.id == id) {
                return None;
            }
            choices.push(ModelChoice { id, label });
        }
        let default = match params.get("default") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let id = ModelId::parse(value.as_str()?).ok()?;
                choices.iter().any(|c| c.id == id).then_some(Some(id))?
            }
        };
        Some(ModelPickOffer { choices, default })
    }

    /// Whether the agent offers `id`.
    pub fn offers(&self, id: &ModelId) -> bool {
        self.choices.iter().any(|c| &c.id == id)
    }

    /// The aliases, in order.
    pub fn ids(&self) -> Vec<String> {
        self.choices.iter().map(|c| c.id.to_string()).collect()
    }
}

/// What `reasoning/v1` offers: the levels the agent's model takes and its default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffortOffer {
    /// The levels, in the card's order.
    pub efforts: Vec<ReasoningEffort>,
    /// The agent's own default, one of the levels.
    pub default: Option<ReasoningEffort>,
}

impl EffortOffer {
    /// Reads the card's `params`: `efforts` is a list of levels of the closed vocabulary (a word
    /// outside it makes the listing malformed); `default`, when present, is one of them.
    pub fn from_params(params: &Value) -> Option<EffortOffer> {
        let mut efforts: Vec<ReasoningEffort> = Vec::new();
        for item in params.get("efforts")?.as_array()? {
            let level = ReasoningEffort::parse(item.as_str()?)?;
            if !efforts.contains(&level) {
                efforts.push(level);
            }
        }
        let default = match params.get("default") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let level = ReasoningEffort::parse(value.as_str()?)?;
                efforts.contains(&level).then_some(Some(level))?
            }
        };
        Some(EffortOffer { efforts, default })
    }

    /// Whether the agent takes `level`.
    pub fn takes(&self, level: ReasoningEffort) -> bool {
        self.efforts.contains(&level)
    }

    /// The levels as the wire spells them.
    pub fn words(&self) -> Vec<String> {
        self.efforts.iter().map(ToString::to_string).collect()
    }
}

/// What a live card says about one extension, read for the call it is read for and never kept
/// (ADR 0008).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Listing<T> {
    /// The card does not list the extension.
    #[default]
    Absent,
    /// The card lists it with `params` that cannot be read: nothing can be said to such an agent.
    Malformed,
    /// The card lists it, well formed.
    Offered(T),
}

impl<T> Listing<T> {
    /// The offer, when the card has a well-formed one.
    pub fn offer(&self) -> Option<&T> {
        match self {
            Listing::Offered(offer) => Some(offer),
            Listing::Absent | Listing::Malformed => None,
        }
    }
}

/// What a card says about both axes of a pick.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PickListings {
    /// `model-pick/v1`.
    pub model: Listing<ModelPickOffer>,
    /// `reasoning/v1`.
    pub effort: Listing<EffortOffer>,
}

/// Why a pick cannot be sent to an agent: the axis, and what the card offers (empty when it lists
/// the extension not at all, or badly).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PickRefusal {
    /// The model is not one the card offers (`model_not_offered`).
    #[error("this agent does not offer the model '{picked}'")]
    ModelNotOffered {
        /// The alias that was picked.
        picked: String,
        /// The aliases the card lists.
        offered: Vec<String>,
    },
    /// The effort is not one the card offers (`reasoning_not_offered`).
    #[error("this agent does not take the reasoning effort '{picked}'")]
    ReasoningNotOffered {
        /// The level that was picked.
        picked: String,
        /// The levels the card lists.
        offered: Vec<String>,
    },
}

impl PickRefusal {
    /// The stable code a client acts on.
    pub fn code(&self) -> &'static str {
        match self {
            PickRefusal::ModelNotOffered { .. } => "model_not_offered",
            PickRefusal::ReasoningNotOffered { .. } => "reasoning_not_offered",
        }
    }

    /// What the card offers.
    pub fn offered(&self) -> &[String] {
        match self {
            PickRefusal::ModelNotOffered { offered, .. }
            | PickRefusal::ReasoningNotOffered { offered, .. } => offered,
        }
    }
}

impl PickListings {
    /// Checks `pick` against the card: every axis it names must be listed and offered. Fails
    /// **closed**: a card that lists neither well formed refuses any pick on that axis.
    ///
    /// # Errors
    /// [`PickRefusal`] naming the first axis (model, then effort) that the card does not offer.
    pub fn check(&self, pick: &ModelPick) -> Result<(), PickRefusal> {
        if let Some(model) = &pick.model {
            let offer = self.model.offer();
            if !offer.is_some_and(|o| o.offers(model)) {
                return Err(PickRefusal::ModelNotOffered {
                    picked: model.to_string(),
                    offered: offer.map(ModelPickOffer::ids).unwrap_or_default(),
                });
            }
        }
        if let Some(level) = pick.reasoning_effort {
            let offer = self.effort.offer();
            if !offer.is_some_and(|o| o.takes(level)) {
                return Err(PickRefusal::ReasoningNotOffered {
                    picked: level.to_string(),
                    offered: offer.map(EffortOffer::words).unwrap_or_default(),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn listings() -> PickListings {
        PickListings {
            model: Listing::Offered(
                ModelPickOffer::from_params(&json!({
                    "choices": [{"id": "fast", "label": "Fast"}, {"id": "deep"}],
                    "default": "fast"
                }))
                .unwrap(),
            ),
            effort: Listing::Offered(
                EffortOffer::from_params(&json!({"efforts": ["low", "high"], "default": "low"}))
                    .unwrap(),
            ),
        }
    }

    #[test]
    fn an_alias_is_small_and_neutral() {
        assert!(ModelId::parse("fast").is_ok());
        assert!(ModelId::parse("deep-2.1_x").is_ok());
        for bad in ["", "Fast", "a b", "gpt/4", &"a".repeat(65)] {
            assert!(ModelId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_message_wins_then_the_thread_axis_by_axis() {
        let thread = ModelPick {
            model: Some(ModelId::parse("fast").unwrap()),
            reasoning_effort: Some(ReasoningEffort::Low),
        };
        let message = ModelPick::model("deep").unwrap();
        let resolved = message.over(&thread);
        assert_eq!(resolved.model.unwrap().as_str(), "deep");
        assert_eq!(resolved.reasoning_effort, Some(ReasoningEffort::Low));
        assert!(ModelPick::default().over(&ModelPick::default()).is_empty());
    }

    #[test]
    fn a_pick_the_card_lists_is_accepted_and_one_it_does_not_is_refused_with_the_offer() {
        let ok = ModelPick {
            model: Some(ModelId::parse("deep").unwrap()),
            reasoning_effort: Some(ReasoningEffort::High),
        };
        assert_eq!(listings().check(&ok), Ok(()));
        let refusal = listings()
            .check(&ModelPick::model("huge").unwrap())
            .unwrap_err();
        assert_eq!(refusal.code(), "model_not_offered");
        assert_eq!(refusal.offered(), ["fast", "deep"]);
        let refusal = listings()
            .check(&ModelPick::effort(ReasoningEffort::Max))
            .unwrap_err();
        assert_eq!(refusal.code(), "reasoning_not_offered");
        assert_eq!(refusal.offered(), ["low", "high"]);
    }

    #[test]
    fn a_card_that_lists_nothing_refuses_a_pick_with_an_empty_offer() {
        let none = PickListings::default();
        let refusal = none.check(&ModelPick::model("fast").unwrap()).unwrap_err();
        assert!(refusal.offered().is_empty());
        assert!(none.check(&ModelPick::default()).is_ok());
    }

    #[test]
    fn params_that_cannot_be_read_offer_nothing() {
        for bad in [
            json!({}),
            json!({"choices": "fast"}),
            json!({"choices": [{"id": "Fast"}]}),
            json!({"choices": [{"id": "a"}, {"id": "a"}]}),
            json!({"choices": [{"id": "a"}], "default": "b"}),
        ] {
            assert!(ModelPickOffer::from_params(&bad).is_none(), "{bad}");
        }
        for bad in [
            json!({}),
            json!({"efforts": ["extreme"]}),
            json!({"efforts": ["low"], "default": "high"}),
        ] {
            assert!(EffortOffer::from_params(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_pick_serialises_as_its_members_and_omits_an_axis_it_does_not_name() {
        let pick = ModelPick {
            model: Some(ModelId::parse("deep").unwrap()),
            reasoning_effort: Some(ReasoningEffort::Disabled),
        };
        assert_eq!(
            serde_json::to_value(&pick).unwrap(),
            json!({"model": "deep", "reasoningEffort": "none"})
        );
        assert_eq!(
            serde_json::to_value(ModelPick::effort(ReasoningEffort::Xhigh)).unwrap(),
            json!({"reasoningEffort": "xhigh"})
        );
        assert!(serde_json::from_value::<ModelPick>(json!({"model": "Bad Id"})).is_err());
    }
}

/// What an agent's card lists for a pick, as a screen reads it (`GET /api/agents`): the model
/// aliases and the reasoning levels the card offers, with the agent's own defaults. An axis the
/// card does not list well is empty, and a client that finds neither draws no picker. It is a
/// listing, never a check: a request that names something else is still refused by
/// [`PickListings::check`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPick {
    /// The model aliases the card offers, in its order. Omitted when it offers none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<AgentModelChoice>,
    /// The agent's own default alias, when the card names one. Omitted otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_default: Option<String>,
    /// The reasoning levels the card offers, in its order. Omitted when it offers none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub efforts: Vec<String>,
    /// The agent's own default level, when the card names one. Omitted otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_default: Option<String>,
}

/// One model alias a card offers, with the label to show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelChoice {
    /// The alias, which a run sends.
    pub id: String,
    /// The label a screen shows; the alias when the card gave none.
    pub label: String,
}

impl AgentPick {
    /// The listing a screen reads, or `None` when the card offers neither axis: an extension it
    /// does not list, or lists badly, says nothing to a client (ADR 0008, fail closed).
    #[must_use]
    pub fn of(listings: &PickListings) -> Option<AgentPick> {
        let model = listings.model.offer();
        let effort = listings.effort.offer();
        if model.is_none() && effort.is_none() {
            return None;
        }
        Some(AgentPick {
            models: model
                .map(|offer| {
                    offer
                        .choices
                        .iter()
                        .map(|choice| AgentModelChoice {
                            id: choice.id.as_str().to_owned(),
                            label: choice.label.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            model_default: model
                .and_then(|offer| offer.default.as_ref())
                .map(|id| id.as_str().to_owned()),
            efforts: effort
                .map(|offer| {
                    offer
                        .efforts
                        .iter()
                        .map(|level| level.as_str().to_owned())
                        .collect()
                })
                .unwrap_or_default(),
            effort_default: effort
                .and_then(|offer| offer.default)
                .map(|level| level.as_str().to_owned()),
        })
    }
}
