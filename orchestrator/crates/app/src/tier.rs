//! The access tier of a message (ADR 0061, `access-tier/v1`,
//! [`docs/api/access-tier-v1.md`](../../../../docs/api/access-tier-v1.md)): what is checked about it
//! **before anything is written**, and what the dispatcher asks of the live card when it sends.
//!
//! The orchestrator enforces no tier: the agent does. It checks that the choice is one of four
//! words ([`parse`], **400**), that the person's roles allow it (`auth.roles.<role>.maxTier`,
//! **422**), and that the live card of the thread's agent lists `access-tier/v1` with the tier among
//! its `tiers` (**422**); a card that cannot be read is **503** and nothing is written
//! ([`App::resolve_tier`]). A tier is never silently dropped: ignoring it would run the agent under
//! its default for a person who asked for something narrower.

use orch_core::{AccessTier, AgentId, TierOffer};
use orch_ports::{AgentError, Ports};
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
/// none. Whether the person may pick it, and whether the agent takes it, is [`App::resolve_tier`]'s.
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
    pub(crate) fn of(offer: Option<&TierOffer>, tier: AccessTier) -> TierCard {
        if offer.is_some_and(|offer| offer.honours(tier)) {
            TierCard::Honoured
        } else {
            TierCard::NotHonoured
        }
    }
}

/// The highest tier the card honours that is not above `limit`: what a person who may go no
/// higher than `limit` gets from an agent that offers `offer`. `None` when the agent has no tier
/// that low.
fn highest_within(offer: &TierOffer, limit: AccessTier) -> Option<AccessTier> {
    offer.tiers().iter().copied().rfind(|tier| *tier <= limit)
}

impl<P: Ports> App<P> {
    /// The access tier a message is written and sent under (ADR 0061): the **one** place that
    /// decides it, for a first message, a follow-up, a fork's first message and an edit. `named` is
    /// the tier the person named; `inherited` is the tier of the job (or of the message an edit
    /// replaces) that a message which cannot name one continues.
    ///
    /// * A **named** tier is checked: the person's cap for this agent (**422** "above what your role
    ///   allows"), then the live card of `agent`, the thread's own (**422** "does not take the
    ///   tier", **503** when the card cannot be read). It is returned as it is.
    /// * An **inherited** tier is lowered to the person's cap and then to the highest tier the live
    ///   card still honours (never raised); a card that no longer lists the extension, or has no tier
    ///   that low, is a **422** that says what to do, and nothing is written.
    /// * **Neither**: with no cap the answer is `None` and no card is read, so plain A2A is as it
    ///   was. With a cap the live card is read (**503** when it cannot be), and an agent that offers
    ///   tiers is given the highest one not above the lower of its `default` and the cap (**422**
    ///   "this agent has no tier your role allows" when none is). An agent whose card lists no
    ///   offer is given none: the cap cannot be said to it, and the agent works as it always did.
    ///
    /// With a cap, the tier returned is `Some` whenever the agent offers tiers, so the message is
    /// sent with it and logged with it, and the job's tier ([`orch_core::Job::tier`]) is pinned: a
    /// person capped at `edits` is never run at an agent's `auto` by leaving the chip alone.
    ///
    /// # Errors
    /// [`AppError::Unprocessable`] as above, [`AppError::CardUnavailable`] when a card that is
    /// needed cannot be read, and [`AppError::RegistryUnavailable`] when the registry cannot say
    /// where the agent is.
    pub(crate) async fn resolve_tier(
        &self,
        access: &Access<'_>,
        agent: &AgentId,
        named: Option<AccessTier>,
        inherited: Option<AccessTier>,
    ) -> Result<Option<AccessTier>, AppError> {
        let id = self.canonical_agent(agent);
        let cap = access.max_tier_for(&id);
        if let Some(tier) = named {
            if cap.is_some_and(|cap| tier > cap) {
                return Err(AppError::Unprocessable(format!(
                    "the tier '{tier}' is above what your role allows"
                )));
            }
        } else if inherited.is_none() && cap.is_none() {
            return Ok(None);
        }
        let Some(entry) = self.resolve_agent(&id).await? else {
            // an agent that is not there is refused when the message is sent: a named tier is
            // refused now, the others are left to that
            return match named {
                Some(tier) => Err(not_taken(tier)),
                None => Ok(None),
            };
        };
        let offer = match self.card_offer(&entry.endpoint).await {
            Ok(offer) => offer,
            Err(error) => {
                tracing::warn!(agent = %id, error = %orch_core::report(&error), "the agent card could not be read to decide the access tier");
                return Err(AppError::CardUnavailable { agent: id });
            }
        };
        match (named, inherited, offer) {
            (Some(tier), _, offer) if offer.as_ref().is_some_and(|o| o.honours(tier)) => {
                Ok(Some(tier))
            }
            (Some(tier), _, _) => Err(not_taken(tier)),
            (None, Some(chosen), Some(offer)) => {
                let limit = cap.map_or(chosen, |cap| chosen.min(cap));
                highest_within(&offer, limit).map(Some).ok_or_else(|| {
                    AppError::Unprocessable(format!(
                        "this agent has no tier at or below '{limit}' any more, so the tier \
                         '{chosen}' of this chat cannot be kept: start a new chat"
                    ))
                })
            }
            (None, Some(chosen), None) => Err(AppError::Unprocessable(format!(
                "this agent no longer lists access tiers, so the tier '{chosen}' of this chat \
                 cannot be kept: start a new chat"
            ))),
            (None, None, Some(offer)) => {
                // `cap` is some: with none and nothing inherited the answer was `None` above
                let limit = cap.map_or(offer.default_tier(), |cap| offer.default_tier().min(cap));
                highest_within(&offer, limit).map(Some).ok_or_else(|| {
                    AppError::Unprocessable("this agent has no tier your role allows".to_owned())
                })
            }
            // the cap cannot be said to an agent that offers no tier
            (None, None, None) => Ok(None),
        }
    }
}

fn not_taken(tier: AccessTier) -> AppError {
    AppError::Unprocessable(format!("this agent does not take the tier '{tier}'"))
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
