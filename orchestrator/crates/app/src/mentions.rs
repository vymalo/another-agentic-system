//! Mentions as structured references (ADR 0026, `mentions/v1`,
//! [`docs/api/mentions-v1.md`](../../../../docs/api/mentions-v1.md)): what is checked about them
//! **before anything is written**.
//!
//! Two parts. The first is **pure**: [`parse`] reads the `forwardedProps["vymalo.mentions"]` a
//! surface received into [`Mention`]s (the shape, **400**), and [`check`] holds them against the
//! message text (the label, the offsets, the order, **422**). Offsets are **UTF-16 code units**
//! (what a JavaScript string indexes), never inside a surrogate pair, and a label is exactly
//! `text[start..end]`: the text is never rewritten and a label is never read as an identity.
//! The second part asks the world, in [`App::check_mentions`]: every `agentId` must be one the
//! live registry lists, `cardUrl` (when given) must be the registry's, the person's roles must
//! let them invoke the agent (`agent.invoke`, ADR 0033), and the thread's own agent cannot be
//! mentioned in its own thread (**422**); a registry that cannot answer is **503** and nothing is
//! written.
//!
//! The same module resolves a mention for the agent that is told of it, when a delegation is sent
//! ([`App::mention_infos`]): the name and the card URL are read from the registry **then**.

use std::collections::BTreeMap;

use orch_core::{
    AgentId, MAX_MENTION_LABEL_UNITS, MAX_MENTIONS, Mention, is_valid_agent_id, utf16_len,
};
use orch_ports::{AgentTransport, MentionInfo, Ports, RegistryEntry};
use serde_json::Value;

use crate::{Access, App, AppError, Permission, Resource};

/// The key of a run's `forwardedProps` that carries the message's mentions (`mentions/v1`): an
/// array of references.
pub const THREAD_MENTIONS_KEY: &str = "vymalo.mentions";

/// The longest `cardUrl` a reference may carry, in bytes. A card URL longer than this is not one
/// the registry lists.
const MAX_CARD_URL_BYTES: usize = 2048;

/// Why a mention was refused: the two classes the contract tells apart.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MentionError {
    /// The member is not an array of at most [`MAX_MENTIONS`] references of the right shape:
    /// **400**.
    #[error("{0}")]
    Malformed(String),
    /// The references are well formed and do not hold against the text, the registry or the
    /// person's roles: **422**.
    #[error("{0}")]
    Invalid(String),
}

impl From<MentionError> for AppError {
    fn from(error: MentionError) -> Self {
        match error {
            MentionError::Malformed(detail) => AppError::Invalid(detail),
            MentionError::Invalid(detail) => AppError::Unprocessable(detail),
        }
    }
}

fn malformed(detail: impl Into<String>) -> MentionError {
    MentionError::Malformed(detail.into())
}

fn invalid(detail: impl Into<String>) -> MentionError {
    MentionError::Invalid(detail.into())
}

/// Reads `forwardedProps["vymalo.mentions"]`: an array of at most [`MAX_MENTIONS`] objects with
/// exactly `agentId`, `label`, `start` and `end` (strings, strings, non-negative integers), and
/// `cardUrl` (a string) when given. `null` is none. Whether the references fit the text is
/// [`check`]'s.
///
/// # Errors
/// [`MentionError::Malformed`] for anything else, naming the reference and the member.
pub fn parse(value: &Value) -> Result<Vec<Mention>, MentionError> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let array = value.as_array().ok_or_else(|| {
        malformed(format!(
            "{THREAD_MENTIONS_KEY} must be an array of at most {MAX_MENTIONS} references"
        ))
    })?;
    if array.len() > MAX_MENTIONS {
        return Err(malformed(format!(
            "{THREAD_MENTIONS_KEY} holds {} references; a message may have at most {MAX_MENTIONS}",
            array.len()
        )));
    }
    array
        .iter()
        .enumerate()
        .map(|(i, reference)| parse_one(i + 1, reference))
        .collect()
}

fn parse_one(n: usize, value: &Value) -> Result<Mention, MentionError> {
    let object = value
        .as_object()
        .ok_or_else(|| malformed(format!("mention {n} must be an object")))?;
    if let Some(unknown) = object.keys().find(|k| {
        !matches!(
            k.as_str(),
            "agentId" | "label" | "start" | "end" | "cardUrl"
        )
    }) {
        return Err(malformed(format!(
            "mention {n} has a member `{}` that a reference does not have",
            // an unknown name is a person's text: bounded before it is echoed
            unknown.chars().take(40).collect::<String>()
        )));
    }
    let text = |member: &str, required: bool| -> Result<Option<String>, MentionError> {
        match object.get(member) {
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(malformed(format!(
                "mention {n}: `{member}` must be a string"
            ))),
            None if required => Err(malformed(format!("mention {n}: `{member}` is required"))),
            None => Ok(None),
        }
    };
    let offset = |member: &str| -> Result<u32, MentionError> {
        let value = object
            .get(member)
            .ok_or_else(|| malformed(format!("mention {n}: `{member}` is required")))?;
        value
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| {
                malformed(format!(
                    "mention {n}: `{member}` must be a non-negative integer (UTF-16 code units)"
                ))
            })
    };
    let card_url = text("cardUrl", false)?;
    if card_url
        .as_ref()
        .is_some_and(|u| u.len() > MAX_CARD_URL_BYTES)
    {
        return Err(malformed(format!(
            "mention {n}: `cardUrl` is longer than {MAX_CARD_URL_BYTES} bytes"
        )));
    }
    Ok(Mention {
        agent_id: AgentId::new(text("agentId", true)?.unwrap_or_default()),
        label: text("label", true)?.unwrap_or_default(),
        start: offset("start")?,
        end: offset("end")?,
        card_url,
    })
}

/// Why an offset names no place in a text.
enum Offset {
    /// Past the end.
    Beyond,
    /// Between the two code units of a character outside the Basic Multilingual Plane.
    Split,
}

/// The byte index in `text` of the place `units` UTF-16 code units from its start.
fn byte_index(text: &str, units: usize) -> Result<usize, Offset> {
    let mut seen = 0usize;
    for (index, c) in text.char_indices() {
        if seen == units {
            return Ok(index);
        }
        seen += c.len_utf16();
        if units < seen {
            return Err(Offset::Split);
        }
    }
    if seen == units {
        Ok(text.len())
    } else {
        Err(Offset::Beyond)
    }
}

/// Holds `mentions` against the `text` of the message that carries them: at most [`MAX_MENTIONS`]
/// (**400**), and for each, in order: the label begins with `@` and has 2 to
/// [`MAX_MENTION_LABEL_UNITS`] UTF-16 code units, `start < end`, neither offset is past the end
/// or inside a surrogate pair, `text[start..end]` (in UTF-16 code units) **is** the label, and it
/// begins at or after the end of the one before (**422**). Offsets are not moved to grapheme
/// boundaries.
///
/// # Errors
/// [`MentionError::Invalid`] naming the first reference (from 1) that breaks a rule.
pub fn check(text: &str, mentions: &[Mention]) -> Result<(), MentionError> {
    if mentions.len() > MAX_MENTIONS {
        return Err(malformed(format!(
            "a message may have at most {MAX_MENTIONS} mentions, not {}",
            mentions.len()
        )));
    }
    let mut previous_end = 0u32;
    for (i, mention) in mentions.iter().enumerate() {
        let n = i + 1;
        let units = utf16_len(&mention.label);
        if !mention.label.starts_with('@') || !(2..=MAX_MENTION_LABEL_UNITS).contains(&units) {
            return Err(invalid(format!(
                "mention {n}: a label begins with @ and has 2 to {MAX_MENTION_LABEL_UNITS} UTF-16 code units"
            )));
        }
        if mention.start >= mention.end {
            return Err(invalid(format!("mention {n}: start must be before end")));
        }
        let place = |units: u32, which: &str| {
            byte_index(text, units as usize).map_err(|why| match why {
                Offset::Beyond => invalid(format!(
                    "mention {n}: {which} is past the end of the text (offsets are UTF-16 code units)"
                )),
                Offset::Split => invalid(format!(
                    "mention {n}: {which} falls inside a surrogate pair (offsets are UTF-16 code units)"
                )),
            })
        };
        let (from, to) = (place(mention.start, "start")?, place(mention.end, "end")?);
        if text.get(from..to) != Some(mention.label.as_str()) {
            return Err(invalid(format!(
                "mention {n}: the label is not the text at its offsets"
            )));
        }
        if mention.start < previous_end {
            return Err(invalid(format!(
                "mention {n}: mentions must be sorted by start and must not overlap"
            )));
        }
        previous_end = mention.end;
    }
    Ok(())
}

impl<P: Ports> App<P> {
    /// Everything that is checked about the mentions of a message before it is written
    /// ([`check`], then the registry and the person's roles): see the module.
    ///
    /// `agent` is the thread's own agent, the one that reads the message. A message with no
    /// mentions is `Ok` at once and the registry is not asked.
    ///
    /// # Errors
    /// [`AppError::Invalid`] (the count), [`AppError::Unprocessable`] for a reference that does not
    /// hold, and [`AppError::RegistryUnavailable`] when the registry cannot say.
    pub(crate) async fn check_mentions(
        &self,
        access: &Access<'_>,
        agent: &AgentId,
        text: &str,
        mentions: &[Mention],
    ) -> Result<(), AppError> {
        if mentions.is_empty() {
            return Ok(());
        }
        check(text, mentions)?;
        for mention in mentions {
            let id = &mention.agent_id;
            // What a person may invoke does not depend on the registry, and is asked first, as
            // for a thread's own agent: an agent they may not use is not described to them as
            // unknown or known.
            if !access.allows(Permission::AgentInvoke, &Resource::Agent { id }) {
                return Err(AppError::Unprocessable(format!(
                    "you may not use '{}'",
                    shown(id)
                )));
            }
            let listed = if is_valid_agent_id(id.as_str()) {
                self.resolve_agent(id).await?
            } else {
                // every source holds its ids to this form: no agent has this one
                None
            };
            let Some(entry) = listed else {
                return Err(AppError::Unprocessable(format!(
                    "unknown agent '{}' in mentions",
                    shown(id)
                )));
            };
            if let Some(seen) = &mention.card_url
                && card_url_of(&entry.endpoint.transport) != Some(seen.as_str())
            {
                return Err(AppError::Unprocessable(format!(
                    "the card of '{id}' moved; refresh the agent list"
                )));
            }
            if id == agent {
                return Err(AppError::Unprocessable(
                    "an agent cannot be mentioned in its own thread".to_owned(),
                ));
            }
        }
        Ok(())
    }

    /// The mentions of a delegation as the addressed agent is told of them: each reference with
    /// its agent's name and card URL **as the registry gives them now** (`mentions/v1`, section
    /// 4). A mention that cannot be resolved (the registry cannot answer, or no longer lists the
    /// agent) goes with its id, label and offsets only: the delegation is never held back for it.
    pub async fn mention_infos(&self, mentions: &[Mention]) -> Vec<MentionInfo> {
        // what the registry says of each agent now, asked once per agent
        let mut resolved: BTreeMap<&AgentId, Option<RegistryEntry>> = BTreeMap::new();
        let mut infos = Vec::with_capacity(mentions.len());
        for mention in mentions {
            let id = &mention.agent_id;
            if !resolved.contains_key(id) {
                let entry = match self.resolve_agent(id).await {
                    Ok(entry) => entry,
                    Err(error) => {
                        tracing::warn!(agent = %id, %error, "a mentioned agent could not be resolved; it goes with its id and label only");
                        None
                    }
                };
                resolved.insert(id, entry);
            }
            let entry = resolved.get(id).and_then(Option::as_ref);
            infos.push(MentionInfo {
                agent_id: id.clone(),
                name: entry.map(|e| e.name.clone()),
                label: mention.label.clone(),
                start: mention.start,
                end: mention.end,
                card_url: entry
                    .and_then(|e| card_url_of(&e.endpoint.transport))
                    .map(str::to_owned),
            });
        }
        infos
    }
}

/// The card URL of an agent, when it has one (only a remote A2A agent does).
fn card_url_of(transport: &AgentTransport) -> Option<&str> {
    match transport {
        AgentTransport::A2a { card_url, .. } => Some(card_url.as_str()),
        AgentTransport::Local { .. } => None,
    }
}

/// An id a person sent, bounded before it is echoed in a refusal.
fn shown(id: &AgentId) -> String {
    id.as_str().chars().take(63).collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn at(label: &str, start: u32, end: u32) -> Mention {
        Mention {
            agent_id: AgentId::new("mock-researcher"),
            label: label.to_owned(),
            start,
            end,
            card_url: None,
        }
    }

    /// The reference to `label` in `text`, found the way a JavaScript composer would count.
    fn found(text: &str, label: &str) -> Mention {
        let byte = text.find(label).expect("the label is in the text");
        let start = utf16_len(&text[..byte]);
        let end = start + utf16_len(label);
        at(
            label,
            u32::try_from(start).unwrap(),
            u32::try_from(end).unwrap(),
        )
    }

    #[test]
    fn a_reference_is_read_with_exactly_its_members() {
        let read = parse(&json!([
            {"agentId": "mock-researcher", "label": "@researcher", "start": 3, "end": 14,
             "cardUrl": "http://r/card"},
            {"agentId": "mock-coder", "label": "@coder", "start": 20, "end": 26}
        ]))
        .unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].card_url.as_deref(), Some("http://r/card"));
        assert_eq!(read[1].card_url, None);
        assert_eq!((read[1].start, read[1].end), (20, 26));
        assert_eq!(parse(&json!([])).unwrap(), vec![]);
        assert_eq!(parse(&Value::Null).unwrap(), vec![]);
    }

    #[test]
    fn what_is_not_an_array_of_references_is_malformed() {
        let ok = json!({"agentId": "a", "label": "@a", "start": 0, "end": 2});
        let many: Vec<Value> = std::iter::repeat_n(ok.clone(), MAX_MENTIONS + 1).collect();
        for bad in [
            json!("@a"),
            json!({"agentId": "a"}),
            json!(7),
            json!([1]),
            json!([null]),
            json!(many),
            // a member missing, of the wrong type, or one a reference does not have
            json!([{"label": "@a", "start": 0, "end": 2}]),
            json!([{"agentId": "a", "start": 0, "end": 2}]),
            json!([{"agentId": "a", "label": "@a", "end": 2}]),
            json!([{"agentId": "a", "label": "@a", "start": 0}]),
            json!([{"agentId": 1, "label": "@a", "start": 0, "end": 2}]),
            json!([{"agentId": "a", "label": 1, "start": 0, "end": 2}]),
            json!([{"agentId": "a", "label": "@a", "start": "0", "end": 2}]),
            json!([{"agentId": "a", "label": "@a", "start": -1, "end": 2}]),
            json!([{"agentId": "a", "label": "@a", "start": 0.5, "end": 2}]),
            json!([{"agentId": "a", "label": "@a", "start": 0, "end": 4294967296u64}]),
            json!([{"agentId": "a", "label": "@a", "start": 0, "end": 2, "cardUrl": null}]),
            json!([{"agentId": "a", "label": "@a", "start": 0, "end": 2, "cardUrl": 3}]),
            json!([{"agentId": "a", "label": "@a", "start": 0, "end": 2, "extra": true}]),
            json!([{"agentId": "a", "label": "@a", "start": 0, "end": 2, "cardUrl": "u".repeat(3000)}]),
        ] {
            assert!(
                matches!(parse(&bad), Err(MentionError::Malformed(_))),
                "{bad} is malformed"
            );
        }
        // at the cap it is read
        let cap: Vec<Value> = std::iter::repeat_n(ok, MAX_MENTIONS).collect();
        assert_eq!(parse(&json!(cap)).unwrap().len(), MAX_MENTIONS);
    }

    #[test]
    fn a_label_is_the_text_at_its_offsets() {
        let text = "ask @researcher to look";
        assert_eq!(check(text, &[at("@researcher", 4, 15)]), Ok(()));
        assert_eq!(check(text, &[]), Ok(()));
        for wrong in [
            at("@researcher", 5, 16),
            at("@researche", 4, 15),
            at("@Researcher", 4, 15),
            at("@researcher", 4, 14),
        ] {
            assert!(
                matches!(
                    check(text, std::slice::from_ref(&wrong)),
                    Err(MentionError::Invalid(_))
                ),
                "{wrong:?}"
            );
        }
    }

    #[test]
    fn a_label_begins_with_an_at_and_has_two_to_sixty_four_units() {
        // "@" alone is one unit, "researcher" has no "@"
        assert!(check("@ x", &[at("@", 0, 1)]).is_err());
        assert!(check("researcher", &[at("researcher", 0, 10)]).is_err());
        assert!(check("", &[at("", 0, 0)]).is_err());
        let longest = format!("@{}", "a".repeat(63));
        assert_eq!(utf16_len(&longest), 64);
        let text = format!("hey {longest} go");
        assert_eq!(check(&text, &[found(&text, &longest)]), Ok(()));
        let too_long = format!("@{}", "a".repeat(64));
        let text = format!("hey {too_long} go");
        assert!(matches!(
            check(&text, &[found(&text, &too_long)]),
            Err(MentionError::Invalid(_))
        ));
        // sixty-four units are sixty-four units, whatever they are made of: thirty-two
        // characters outside the Basic Multilingual Plane after the @ are sixty-five
        let emoji = format!("@{}", "\u{1F604}".repeat(32));
        assert_eq!(utf16_len(&emoji), 65);
        let text = format!("hey {emoji}");
        assert!(check(&text, &[found(&text, &emoji)]).is_err());
        let emoji = format!("@{}", "\u{1F604}".repeat(31));
        assert_eq!(utf16_len(&emoji), 63);
        let text = format!("hey {emoji}");
        assert_eq!(check(&text, &[found(&text, &emoji)]), Ok(()));
    }

    #[test]
    fn offsets_are_utf16_code_units_after_an_emoji() {
        // the emoji is two units in a JavaScript string and one scalar value
        let text = "\u{1F604} @researcher please";
        let mention = at("@researcher", 3, 14);
        assert_eq!(check(text, &[mention]), Ok(()));
        // counted in scalar values (2..13) or in UTF-8 bytes (5..16) it is wrong
        for wrong in [at("@researcher", 2, 13), at("@researcher", 5, 16)] {
            assert!(
                matches!(
                    check(text, std::slice::from_ref(&wrong)),
                    Err(MentionError::Invalid(_))
                ),
                "{wrong:?}"
            );
        }
        // several emoji in front, and between two mentions
        let text = "\u{1F468}\u{200D}\u{1F469} @a \u{1F680}\u{1F680} @b";
        assert_eq!(check(text, &[found(text, "@a"), found(text, "@b")]), Ok(()));
        assert_eq!(found(text, "@a").start, 6, "a family emoji is five units");
    }

    #[test]
    fn offsets_are_utf16_code_units_after_combining_marks() {
        // "e" and a combining acute accent: two scalar values, two units, one grapheme
        let text = "cafe\u{301} @researcher";
        let mention = found(text, "@researcher");
        assert_eq!((mention.start, mention.end), (6, 17));
        assert_eq!(check(text, &[mention]), Ok(()));
        // the same text counted in graphemes (5) is wrong
        assert!(check(text, &[at("@researcher", 5, 16)]).is_err());
        // a label may sit directly before a combining mark: offsets are not moved to graphemes,
        // so the mark stays outside the label and the reference is what it says
        let text = "@a\u{301}";
        assert_eq!(check(text, &[at("@a", 0, 2)]), Ok(()));
        // and a label that includes the mark is the text at 0..3
        assert_eq!(check(text, &[at("@a\u{301}", 0, 3)]), Ok(()));
        // a mention that begins on a combining mark is the producer's own: the label rule
        // catches it, because a label begins with `@`
        let text = "x\u{301}@a";
        assert_eq!(check(text, &[at("@a", 2, 4)]), Ok(()));
        assert!(check(text, &[at("@a", 1, 3)]).is_err());
    }

    #[test]
    fn an_offset_inside_a_surrogate_pair_is_refused() {
        let text = "\u{1F604}@a";
        // the pair is units 0 and 1: 1 is its middle
        assert_eq!(check(text, &[at("@a", 2, 4)]), Ok(()));
        let split_start = check(text, &[at("@a", 1, 3)]);
        let Err(MentionError::Invalid(why)) = split_start else {
            panic!("a start inside a pair is refused: {split_start:?}");
        };
        assert!(why.contains("surrogate pair"), "{why}");
        // an end inside a pair: the label itself holds an emoji, and the end cuts it
        let text = "@a\u{1F604} b";
        assert_eq!(check(text, &[at("@a\u{1F604}", 0, 4)]), Ok(()));
        let split_end = check(text, &[at("@a", 0, 3)]);
        let Err(MentionError::Invalid(why)) = split_end else {
            panic!("an end inside a pair is refused: {split_end:?}");
        };
        assert!(why.contains("surrogate pair"), "{why}");
    }

    #[test]
    fn offsets_are_inside_the_text_and_in_order() {
        let text = "@a @b";
        assert!(check(text, &[at("@b", 3, 5)]).is_ok());
        assert!(check(text, &[at("@b", 3, 6)]).is_err(), "past the end");
        assert!(check(text, &[at("@b", 9, 11)]).is_err(), "far past the end");
        assert!(
            check(text, &[at("@a", 2, 2)]).is_err(),
            "start is before end"
        );
        assert!(check(text, &[at("@a", 2, 0)]).is_err());
        // sorted, and not overlapping; touching is fine
        assert!(check(text, &[at("@a", 0, 2), at("@b", 3, 5)]).is_ok());
        assert!(
            check(text, &[at("@b", 3, 5), at("@a", 0, 2)]).is_err(),
            "out of order"
        );
        let text = "@a@b";
        assert!(
            check(text, &[at("@a", 0, 2), at("@b", 2, 4)]).is_ok(),
            "touching"
        );
        let text = "@a@a";
        assert!(
            check(text, &[at("@a", 0, 2), at("@a", 0, 2)]).is_err(),
            "the same twice"
        );
        // the same agent at two places is two mentions
        assert!(check(text, &[at("@a", 0, 2), at("@a", 2, 4)]).is_ok());
    }

    #[test]
    fn more_than_sixteen_are_malformed_not_unprocessable() {
        let text = "@a ".repeat(MAX_MENTIONS + 1);
        let all: Vec<Mention> = (0..=MAX_MENTIONS)
            .map(|i| {
                at(
                    "@a",
                    u32::try_from(i * 3).unwrap(),
                    u32::try_from(i * 3 + 2).unwrap(),
                )
            })
            .collect();
        assert!(matches!(
            check(&text, &all),
            Err(MentionError::Malformed(_))
        ));
        assert_eq!(check(&text, &all[..MAX_MENTIONS]), Ok(()));
    }

    #[test]
    fn the_two_classes_become_the_two_statuses() {
        assert!(matches!(
            AppError::from(MentionError::Malformed("x".into())),
            AppError::Invalid(_)
        ));
        assert!(matches!(
            AppError::from(MentionError::Invalid("x".into())),
            AppError::Unprocessable(_)
        ));
    }
}
