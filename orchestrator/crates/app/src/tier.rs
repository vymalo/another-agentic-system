//! The access tier of a message (ADR 0061, `access-tier/v1`,
//! [`docs/api/access-tier-v1.md`](../../../../docs/api/access-tier-v1.md)): what is checked about it
//! **before anything is written**, and what the dispatcher asks of the live card when it sends.
//!
//! The orchestrator enforces no tier: the agent does. It checks that the choice is one of four
//! words ([`parse`], **400**), that the person's roles allow it (`auth.roles.<role>.maxTier`,
//! **422**), and that the live card of the thread's agent lists `access-tier/v1` with the tier among
//! its `tiers` (**422**); a card that cannot be read is **503** and nothing is written
//! ([`App::check_tier`]). A tier is never silently dropped: ignoring it would run the agent under
//! its default for a person who asked for something narrower.

use orch_core::{AccessTier, AgentId};
use orch_ports::{AgentCardInfo, AgentError, Ports};
use serde_json::Value;

use crate::{Access, App, AppError};

/// The key of a run's `forwardedProps` that carries the message's access tier (`access-tier/v1`): one
/// of `plan`, `edits`, `auto`, `bypass`.
pub const THREAD_TIER_KEY: &str = "vymalo.tier";

/// Why a tier was refused before it could be checked against anything: **400**.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct TierError(String);

impl From<TierError> for AppError {
    fn from(error: TierError) -> Self {
        AppError::Invalid(error.0)
    }
}

/// Reads `forwardedProps["vymalo.tier"]`: one of the four words, spelled exactly, or `null` for
/// none. Whether the person may pick it, and whether the agent takes it, is [`App::check_tier`]'s.
///
/// # Errors
/// [`TierError`] for anything else: a number, an object, another case, an unknown word.
pub fn parse(value: &Value) -> Result<Option<AccessTier>, TierError> {
    match value {
        Value::Null => Ok(None),
        Value::String(word) => AccessTier::parse(word).map(Some).ok_or_else(not_a_tier),
        _ => Err(not_a_tier()),
    }
}

fn not_a_tier() -> TierError {
    TierError(format!(
        "{THREAD_TIER_KEY} must be one of {}",
        AccessTier::ALL.map(AccessTier::as_str).join(", ")
    ))
}

/// What a live card says about a tier a message names.
#[derive(Debug)]
pub(crate) enum TierCard {
    /// The card lists `access-tier/v1` with well-formed params and the tier is among its `tiers`.
    Honoured,
    /// The card was read, and it does not honour the tier: it does not list the extension, lists it
    /// badly, or does not list this tier.
    NotHonoured,
    /// The card could not be read in time.
    Unreadable(AgentError),
}

impl TierCard {
    pub(crate) fn of(card: &AgentCardInfo, tier: AccessTier) -> TierCard {
        if card
            .access_tier
            .as_ref()
            .is_some_and(|offer| offer.honours(tier))
        {
            TierCard::Honoured
        } else {
            TierCard::NotHonoured
        }
    }
}

impl<P: Ports> App<P> {
    /// Everything that is checked about the access tier of a message before it is written: the
    /// person's cap, then the live card of `agent`, the thread's own agent (the one that reads the
    /// message). A message with no tier is `Ok` at once and no card is read.
    ///
    /// # Errors
    /// [`AppError::Unprocessable`] when the person's roles do not allow the tier or the agent does
    /// not take it, and [`AppError::CardUnavailable`] when the card cannot be read (or the registry
    /// cannot say where it is: [`AppError::RegistryUnavailable`]).
    pub(crate) async fn check_tier(
        &self,
        access: &Access<'_>,
        agent: &AgentId,
        tier: Option<AccessTier>,
    ) -> Result<(), AppError> {
        let Some(tier) = tier else { return Ok(()) };
        if access.max_tier().is_some_and(|cap| tier > cap) {
            return Err(AppError::Unprocessable(format!(
                "the tier '{tier}' is above what your role allows"
            )));
        }
        let id = self.canonical_agent(agent);
        let refused =
            || AppError::Unprocessable(format!("this agent does not take the tier '{tier}'"));
        let Some(entry) = self.resolve_agent(&id).await? else {
            return Err(refused());
        };
        match self.tier_card(&entry.endpoint, tier).await {
            TierCard::Honoured => Ok(()),
            TierCard::NotHonoured => Err(refused()),
            TierCard::Unreadable(error) => {
                tracing::warn!(agent = %id, error = %orch_core::report(&error), "the agent card could not be read to check the access tier");
                Err(AppError::CardUnavailable { agent: id })
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_tier_is_one_of_four_words_or_none() {
        assert_eq!(parse(&json!(null)), Ok(None));
        for tier in AccessTier::ALL {
            assert_eq!(parse(&json!(tier.as_str())), Ok(Some(tier)));
        }
    }

    #[test]
    fn anything_else_is_malformed_and_names_the_key() {
        for bad in [
            json!(1),
            json!(true),
            json!([]),
            json!({"tier": "plan"}),
            json!("Plan"),
            json!("yolo"),
            json!(""),
            json!(" plan"),
        ] {
            let error = parse(&bad).unwrap_err();
            assert!(
                error.to_string().contains("vymalo.tier")
                    && error.to_string().contains("plan, edits, auto, bypass"),
                "{bad}: {error}"
            );
            assert!(matches!(AppError::from(error), AppError::Invalid(_)));
        }
    }
}
