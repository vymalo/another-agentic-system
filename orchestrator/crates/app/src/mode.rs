//! The mode of a message or a thread (ADR 0109, `agent-mode/v1`,
//! [`docs/api/agent-mode-v1.md`](../../../../docs/api/agent-mode-v1.md)): what a run names, and
//! what is checked about it **before anything is written**.
//!
//! A mode is not a hint. It is read from `forwardedProps["vymalo.mode"]` ([`parse_mode`], **400**
//! for a value that is not one), resolved by precedence (the message's own, then the thread's
//! default, then nothing: [`App::resolve_mode`]) and checked against the **live** card of the
//! thread's agent: a mode the card does not list is a **422** with `mode_not_offered` and the list
//! that is offered, and nothing is written. A card that cannot be read is a **503**. The mode that
//! results is written on the `user_message` and carried by everything that continues it.

use orch_core::{AgentId, ModeName};
use orch_ports::Ports;
use serde_json::Value;

use crate::{App, AppError};

/// The key of a run's `forwardedProps` that carries the message's mode (`agent-mode/v1`).
pub const THREAD_MODE_KEY: &str = "vymalo.mode";

/// Why a mode was refused before it could be checked against anything: **400**.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ModeRequestError(String);

impl From<ModeRequestError> for AppError {
    fn from(error: ModeRequestError) -> Self {
        AppError::Invalid(error.0)
    }
}

/// Reads the mode a client named: a non-blank string of at most 64 bytes with no control
/// character, or `null` for none.
///
/// # Errors
/// [`ModeRequestError`] for anything else; the refusal names the key and never repeats the value.
pub fn parse_mode(value: &Value) -> Result<Option<ModeName>, ModeRequestError> {
    match value {
        Value::Null => Ok(None),
        Value::String(name) => ModeName::parse(name).map(Some).map_err(|_| {
            ModeRequestError(format!(
                "{THREAD_MODE_KEY} must be 1 to 64 bytes, not blank and with no control character"
            ))
        }),
        _ => Err(ModeRequestError(format!(
            "{THREAD_MODE_KEY} must be a mode name"
        ))),
    }
}

impl<P: Ports> App<P> {
    /// The mode a message is written and sent with (ADR 0109): the **one** place that decides it,
    /// for a first message, a follow-up, a fork's first message and an edit. `named` is what the
    /// message names (or, for an edit that names none, what the replaced message carried);
    /// `thread_default` is the thread's own default. The message wins, then the thread, then
    /// nothing.
    ///
    /// With nothing named anywhere the answer is none and **no card is read**, so plain A2A is as it
    /// was. Otherwise the live card of `agent` is read now (never cached, ADR 0008) and the mode
    /// must be offered; the thread's default is checked here too, so a card that has since dropped
    /// it refuses the message that resolves to it and never falls back silently.
    ///
    /// # Errors
    /// [`AppError::ModeNotOffered`] (**422**) when the card does not offer it, or lists the
    /// extension not at all or badly (fail closed), or the registry does not know the agent;
    /// [`AppError::CardUnavailable`] (**503**) when the card cannot be read;
    /// [`AppError::RegistryUnavailable`] when the registry cannot say where the agent is.
    pub(crate) async fn resolve_mode(
        &self,
        agent: &AgentId,
        named: Option<ModeName>,
        thread_default: Option<&ModeName>,
    ) -> Result<Option<ModeName>, AppError> {
        let Some(mode) = named.or_else(|| thread_default.cloned()) else {
            return Ok(None);
        };
        let id = self.canonical_agent(agent);
        let listing = match self.resolve_agent(&id).await? {
            None => orch_core::ModeListing::default(),
            Some(entry) => match self.card_pick_listings(&entry.endpoint).await {
                Ok(listings) => listings.mode,
                Err(error) => {
                    tracing::warn!(agent = %id, error = %orch_core::report(&error), "the agent card could not be read to check the mode");
                    return Err(AppError::CardUnavailable { agent: id });
                }
            },
        };
        orch_core::check_mode(&listing, Some(&mode)).map_err(AppError::ModeNotOffered)?;
        Ok(Some(mode))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn a_mode_is_read_as_its_name_and_null_as_none() {
        assert_eq!(parse_mode(&Value::Null).unwrap(), None);
        assert_eq!(
            parse_mode(&Value::String("plan".to_owned())).unwrap(),
            Some(ModeName::parse("plan").unwrap())
        );
    }

    #[test]
    fn a_value_that_is_not_a_mode_name_is_refused_naming_the_key_never_the_value() {
        for bad in [
            Value::Bool(true),
            serde_json::json!(3),
            Value::Array(vec![]),
            Value::String("   ".to_owned()),
            Value::String("a\nb".to_owned()),
            Value::String("a".repeat(65)),
        ] {
            let error = parse_mode(&bad).unwrap_err();
            let message = error.to_string();
            assert!(message.contains(THREAD_MODE_KEY), "{message}");
            assert!(
                !message.contains("a\nb"),
                "the value is never repeated: {message}"
            );
        }
    }
}
