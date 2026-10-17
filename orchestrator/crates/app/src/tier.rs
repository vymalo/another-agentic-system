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

use orch_core::{AccessTier, AgentId};
use orch_ports::{AgentEndpoint, AgentError, Ports};
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

/// What a message is written and sent under, as [`App::resolve_tier`] decided when it was accepted.
///
/// At most one of the two is set. `tier` is a tier: the message is sent with it, if the live card
/// still honours it when it is sent. `cap` is the cap of the person's role, set **instead** when no
/// tier could be decided because the agent's card offered none; it travels with the message and
/// with everything the job sends on its behalf, and the dispatcher turns it into a tier when it
/// reads the live card ([`App::tier_for_send`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct TierPin {
    pub(crate) tier: Option<AccessTier>,
    pub(crate) cap: Option<AccessTier>,
}

impl TierPin {
    fn tier(tier: AccessTier) -> TierPin {
        TierPin {
            tier: Some(tier),
            cap: None,
        }
    }

    fn cap(cap: AccessTier) -> TierPin {
        TierPin {
            tier: None,
            cap: Some(cap),
        }
    }
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
    /// * **Neither**: with no cap the answer is none and no card is read, so plain A2A is as it
    ///   was. With a cap the live card is read (**503** when it cannot be), and an agent that offers
    ///   tiers is given the highest one not above the lower of its `default` and the cap (**422**
    ///   when it has none, or lists the extension badly). An agent whose card lists no offer, or
    ///   that the registry cannot place, is given **no tier but the cap**
    ///   ([`TierPin::cap`]): the cap is written with the message and kept on the job, and the
    ///   dispatcher decides again from the live card each time it sends, so an agent that is
    ///   redeployed with tiers is never run at its own default for a person capped below it.
    ///
    /// With a cap, the pin always holds a tier or the cap, so the job ([`orch_core::Job::tier`],
    /// [`orch_core::Job::tier_cap`]) and every row it writes keep it.
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
    ) -> Result<TierPin, AppError> {
        let id = self.canonical_agent(agent);
        let cap = access.max_tier_for(&id);
        if let Some(tier) = named {
            if cap.is_some_and(|cap| tier > cap) {
                return Err(AppError::Unprocessable(format!(
                    "the tier '{tier}' is above what your role allows"
                )));
            }
        } else if inherited.is_none() && cap.is_none() {
            return Ok(TierPin::default());
        }
        let Some(entry) = self.resolve_agent(&id).await? else {
            // an agent that is not there is refused when the message is sent: a named tier is
            // refused now; the others are left to that, with the cap kept so that nothing is
            // sent above it should the agent come back
            return match named {
                Some(tier) => Err(not_taken(tier)),
                None => Ok(TierPin { tier: None, cap }),
            };
        };
        let listing = match self.card_listing(&entry.endpoint).await {
            Ok(listing) => listing,
            Err(error) => {
                tracing::warn!(agent = %id, error = %orch_core::report(&error), "the agent card could not be read to decide the access tier");
                return Err(AppError::CardUnavailable { agent: id });
            }
        };
        match (named, inherited) {
            (Some(tier), _) if listing.offer().is_some_and(|offer| offer.honours(tier)) => {
                Ok(TierPin::tier(tier))
            }
            (Some(tier), _) => Err(not_taken(tier)),
            (None, Some(chosen)) => {
                let Some(offer) = listing.offer() else {
                    return Err(AppError::Unprocessable(format!(
                        "this agent no longer lists access tiers, so the tier '{chosen}' of this \
                         chat cannot be kept: start a new chat"
                    )));
                };
                let limit = cap.map_or(chosen, |cap| chosen.min(cap));
                offer
                    .highest_within(limit)
                    .map(TierPin::tier)
                    .ok_or_else(|| {
                        AppError::Unprocessable(format!(
                            "this agent has no tier at or below '{limit}' any more, so the tier \
                             '{chosen}' of this chat cannot be kept: start a new chat"
                        ))
                    })
            }
            (None, None) => match cap {
                // `cap` is some: with none and nothing inherited the answer was given above
                Some(cap) => match listing.under_cap(cap) {
                    Ok(Some(tier)) => Ok(TierPin::tier(tier)),
                    // the agent has no tier to give: keep the cap for when it has
                    Ok(None) => Ok(TierPin::cap(cap)),
                    Err(refusal) => Err(AppError::Unprocessable(refusal.to_string())),
                },
                None => Ok(TierPin::default()),
            },
        }
    }

    /// The tier a message is **sent** under, decided by the live card of `endpoint`, read now:
    /// the dispatcher's half of [`App::resolve_tier`], called at every send of a delegation, a
    /// steer or an ask. `tier` and `cap` are what the message was written with ([`TierPin`]).
    ///
    /// * Neither: `Ok(None)`, and no card is read.
    /// * A `tier`: it is sent if the card still honours it; otherwise
    ///   [`AgentError::Unsupported`] (nothing is sent, the work fails).
    /// * A `cap`: the highest tier the card offers within the lower of its default and the cap
    ///   ([`orch_core::TierListing::under_cap`]); a card that lists no tier gets the plain message; one that
    ///   has none that low, or lists the extension badly, is [`AgentError::Unsupported`].
    ///
    /// # Errors
    /// [`AgentError::Unsupported`] as above (a refusal, never retried); the error the read gave when
    /// the card could not be read (retried when it is transient).
    pub(crate) async fn tier_for_send(
        &self,
        endpoint: &AgentEndpoint,
        tier: Option<AccessTier>,
        cap: Option<AccessTier>,
    ) -> Result<Option<AccessTier>, AgentError> {
        match (tier, cap) {
            (None, None) => Ok(None),
            (Some(tier), _) => {
                let listing = self.card_listing(endpoint).await?;
                if listing.offer().is_some_and(|offer| offer.honours(tier)) {
                    Ok(Some(tier))
                } else {
                    Err(AgentError::Unsupported(format!(
                        "the agent no longer takes the access tier '{tier}'"
                    )))
                }
            }
            (None, Some(cap)) => self
                .card_listing(endpoint)
                .await?
                .under_cap(cap)
                .map_err(|refusal| AgentError::Unsupported(refusal.to_string())),
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
