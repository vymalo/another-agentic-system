//! The model and the reasoning effort of a message (ADR 0086, `model-pick/v1`,
//! [`docs/api/model-pick-v1.md`](../../../../docs/api/model-pick-v1.md)): what a run names, and what
//! is checked about it **before anything is written**.
//!
//! A pick is not a hint. It is read from `forwardedProps["vymalo.model"]` and
//! `["vymalo.reasoningEffort"]` ([`parse`], **400** for a value that is not one), resolved by
//! precedence (the message's own, then the thread's default, then nothing: [`ModelPick::over`]) and
//! checked against the **live** card of the thread's agent ([`App::resolve_pick`]): a choice the card
//! does not list is a **422** with `model_not_offered` or `reasoning_not_offered` and the list that
//! is offered, and nothing is written. A card that cannot be read is a **503**. The pick that
//! results is written on the `user_message` and carried by everything that continues it.

use orch_core::{AgentId, ModelId, ModelPick, ReasoningEffort};
use orch_ports::Ports;
use serde_json::Value;

use crate::{App, AppError};

/// The key of a run's `forwardedProps` that carries the message's model alias (`model-pick/v1`).
pub const THREAD_MODEL_KEY: &str = "vymalo.model";

/// The key of a run's `forwardedProps` that carries the message's reasoning effort (`reasoning/v1`).
pub const THREAD_REASONING_KEY: &str = "vymalo.reasoningEffort";

/// Why a pick was refused before it could be checked against anything: **400**.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PickRequestError(String);

impl From<PickRequestError> for AppError {
    fn from(error: PickRequestError) -> Self {
        AppError::Invalid(error.0)
    }
}

/// Reads the model alias a client named: a string of the alias's alphabet, or `null` for none.
///
/// # Errors
/// [`PickRequestError`] for anything else; the refusal names the key and never repeats the value.
pub fn parse_model(value: &Value) -> Result<Option<ModelId>, PickRequestError> {
    match value {
        Value::Null => Ok(None),
        Value::String(id) => ModelId::parse(id).map(Some).map_err(|_| {
            PickRequestError(format!(
                "{THREAD_MODEL_KEY} must be 1 to 64 characters of a-z, 0-9, '.', '_' and '-'"
            ))
        }),
        _ => Err(PickRequestError(format!(
            "{THREAD_MODEL_KEY} must be a model alias"
        ))),
    }
}

/// Reads the reasoning level a client named: one of the seven words, spelled exactly, or `null`.
///
/// # Errors
/// [`PickRequestError`] for anything else; the refusal lists the words.
pub fn parse_effort(value: &Value) -> Result<Option<ReasoningEffort>, PickRequestError> {
    let not_a_level = || {
        PickRequestError(format!(
            "{THREAD_REASONING_KEY} must be one of {}",
            ReasoningEffort::ALL.map(ReasoningEffort::as_str).join(", ")
        ))
    };
    match value {
        Value::Null => Ok(None),
        Value::String(word) => ReasoningEffort::parse(word)
            .map(Some)
            .ok_or_else(not_a_level),
        _ => Err(not_a_level()),
    }
}

/// Reads the two members of a request together; `None` when neither names anything.
///
/// # Errors
/// [`PickRequestError`] from either member.
pub fn parse(
    model: Option<&Value>,
    effort: Option<&Value>,
) -> Result<Option<ModelPick>, PickRequestError> {
    let model = model.map(parse_model).transpose()?.flatten();
    let reasoning_effort = effort.map(parse_effort).transpose()?.flatten();
    Ok(ModelPick {
        model,
        reasoning_effort,
    }
    .into_option())
}

impl<P: Ports> App<P> {
    /// The pick a message is written and sent with (ADR 0086): the **one** place that decides it,
    /// for a first message, a follow-up, a fork's first message and an edit. `named` is what the
    /// message names (or, for an edit that names none, what the replaced message carried);
    /// `thread_default` is the thread's own default. The message wins, then the thread, then
    /// nothing.
    ///
    /// With nothing named anywhere the answer is none and **no card is read**, so plain A2A is as it
    /// was. Otherwise the live card of `agent` is read now (never cached, ADR 0008) and every axis
    /// the pick names must be offered; the thread's default is checked here too, so a card that has
    /// since dropped it refuses the message that resolves to it and never falls back silently.
    ///
    /// # Errors
    /// [`AppError::PickNotOffered`] (**422**) when the card does not offer it, or lists the
    /// extension not at all or badly (fail closed), or the registry does not know the agent;
    /// [`AppError::CardUnavailable`] (**503**) when the card cannot be read;
    /// [`AppError::RegistryUnavailable`] when the registry cannot say where the agent is.
    pub(crate) async fn resolve_pick(
        &self,
        agent: &AgentId,
        named: Option<ModelPick>,
        thread_default: &ModelPick,
    ) -> Result<Option<ModelPick>, AppError> {
        let Some(pick) = named.unwrap_or_default().over(thread_default).into_option() else {
            return Ok(None);
        };
        let id = self.canonical_agent(agent);
        let listings = match self.resolve_agent(&id).await? {
            None => orch_core::PickListings::default(),
            Some(entry) => match self.card_pick_listings(&entry.endpoint).await {
                Ok(listings) => listings,
                Err(error) => {
                    tracing::warn!(agent = %id, error = %orch_core::report(&error), "the agent card could not be read to check the model pick");
                    return Err(AppError::CardUnavailable { agent: id });
                }
            },
        };
        listings.check(&pick).map_err(AppError::PickNotOffered)?;
        Ok(Some(pick))
    }
}
