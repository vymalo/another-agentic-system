//! The mentions of a message on the A2A client side (`mentions/v1`, ADR 0026, ADR 0008): what the
//! message carries when the live card lists the extension.
//!
//! The agent is told **who the person addressed**, as structured references, under the extension's
//! URI in the message metadata, and the URI is activated in `A2A-Extensions` and
//! `message.extensions`. Only an agent whose card, read for this very call, lists the URI exactly
//! gets any of it, and only when the message has mentions; any other agent gets the message text as
//! it is, with the labels in it, and nothing else. Nothing here runs an agent: the references are
//! data.
//!
//! The text is never rewritten, so `start` and `end` index it as the agent receives it. That is the
//! text the person wrote, except for the first task of a fork, whose agent is told the conversation
//! it continues in front of the message and in the same part ([`history_preamble`]): the offsets
//! move by what stands in front, counted in UTF-16 code units as they are.

use std::collections::BTreeSet;

use orch_core::{KnownExtension, history_preamble, utf16_len};
use orch_ports::{MentionInfo, SendContent, SendRequest};
use serde_json::{Map, Value, json};

/// The value of the extension's key in the message metadata, or `None` when the message gets
/// none: the card does not list the extension, the message is not text, or it mentions nobody.
///
/// `coordinate` (`{"tool": "ask_agent"}`) is present only when the card also lists
/// `thread-tools/v1`, a grant was minted for this message (`granted`: the agent has the endpoint
/// the tool is on) and the deployment's endpoint offers the tool (`asks`).
pub(crate) fn mentions_metadata(
    req: &SendRequest,
    card_extensions: &BTreeSet<KnownExtension>,
    granted: bool,
    asks: bool,
) -> Option<Value> {
    if req.mentions.is_empty()
        || !card_extensions.contains(&KnownExtension::Mentions)
        || !matches!(req.content, SendContent::Text(_))
    {
        return None;
    }
    let in_front = req
        .history
        .as_ref()
        .map_or(0, |history| utf16_len(&history_preamble(history)));
    let mut metadata = json!({
        "mentions": req
            .mentions
            .iter()
            .map(|mention| reference(mention, in_front))
            .collect::<Vec<_>>(),
    });
    if asks && granted && card_extensions.contains(&KnownExtension::ThreadTools) {
        metadata["coordinate"] = json!({"tool": "ask_agent"});
    }
    Some(metadata)
}

/// One reference as the agent reads it: `agentId`, `name` and `cardUrl` when the dispatcher could
/// read them from the registry, `label`, `start` and `end`.
fn reference(mention: &MentionInfo, in_front: usize) -> Value {
    let shift = u32::try_from(in_front).unwrap_or(u32::MAX);
    let mut reference = Map::new();
    reference.insert("agentId".to_owned(), json!(mention.agent_id));
    if let Some(name) = &mention.name {
        reference.insert("name".to_owned(), json!(name));
    }
    reference.insert("label".to_owned(), json!(mention.label));
    reference.insert(
        "start".to_owned(),
        json!(mention.start.saturating_add(shift)),
    );
    reference.insert("end".to_owned(), json!(mention.end.saturating_add(shift)));
    if let Some(card_url) = &mention.card_url {
        reference.insert("cardUrl".to_owned(), json!(card_url));
    }
    Value::Object(reference)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use orch_core::{AgentId, ForkHistory, HistoryEntry, HistoryRole};
    use orch_ports::AgentEndpoint;

    use super::*;

    fn mention(id: &str, name: Option<&str>) -> MentionInfo {
        MentionInfo {
            agent_id: AgentId::new(id),
            name: name.map(str::to_owned),
            label: format!("@{id}"),
            start: 4,
            end: 4 + 1 + u32::try_from(id.len()).unwrap(),
            card_url: name.map(|_| format!("http://{id}:8080/.well-known/agent-card.json")),
        }
    }

    fn request(mentions: Vec<MentionInfo>) -> SendRequest {
        SendRequest {
            endpoint: AgentEndpoint::a2a(AgentId::new("chat"), "http://chat/card", None),
            message_id: "m".to_owned(),
            context_id: "c".to_owned(),
            task_id: None,
            reference_task_ids: Vec::new(),
            content: SendContent::Text("ask @mock-researcher".to_owned()),
            release: None,
            ui_catalog: None,
            thread_tools: None,
            history: None,
            steer: false,
            mentions,
        }
    }

    fn lists(extensions: &[KnownExtension]) -> BTreeSet<KnownExtension> {
        extensions.iter().copied().collect()
    }

    #[test]
    fn a_card_that_lists_the_extension_is_told_the_references() {
        let req = request(vec![mention("mock-researcher", Some("Mock researcher"))]);
        let metadata =
            mentions_metadata(&req, &lists(&[KnownExtension::Mentions]), false, false).unwrap();
        assert_eq!(
            metadata,
            json!({"mentions": [{
                "agentId": "mock-researcher",
                "name": "Mock researcher",
                "label": "@mock-researcher",
                "start": 4,
                "end": 20,
                "cardUrl": "http://mock-researcher:8080/.well-known/agent-card.json"
            }]})
        );
    }

    #[test]
    fn an_unresolved_mention_goes_with_its_id_label_and_offsets_only() {
        let req = request(vec![mention("gone", None)]);
        let metadata =
            mentions_metadata(&req, &lists(&[KnownExtension::Mentions]), false, false).unwrap();
        assert_eq!(
            metadata["mentions"][0],
            json!({"agentId": "gone", "label": "@gone", "start": 4, "end": 9})
        );
    }

    #[test]
    fn a_card_without_the_extension_a_message_without_mentions_and_an_action_get_nothing() {
        let req = request(vec![mention("mock-researcher", Some("Mock researcher"))]);
        // every other extension, and none
        for others in [
            lists(&[]),
            lists(&[
                KnownExtension::ThreadTools,
                KnownExtension::UiCatalog,
                KnownExtension::Steps,
                KnownExtension::TextStream,
            ]),
        ] {
            assert_eq!(mentions_metadata(&req, &others, true, true), None);
        }
        let none = request(vec![]);
        assert_eq!(
            mentions_metadata(&none, &lists(&[KnownExtension::Mentions]), true, true),
            None
        );
        let mut action = req;
        action.content = SendContent::UiAction {
            action: orch_core::UiActionData {
                surface_id: "s".into(),
                name: "go".into(),
                source_component_id: "b".into(),
                context: Map::new(),
                version: orch_core::UiVersion::V0_9_1,
                run_id: None,
            },
            at: jiff::Timestamp::UNIX_EPOCH,
        };
        assert_eq!(
            mentions_metadata(&action, &lists(&[KnownExtension::Mentions]), true, true),
            None
        );
    }

    #[test]
    fn coordinate_needs_the_card_the_grant_and_an_endpoint_that_offers_the_tool() {
        let req = request(vec![mention("mock-researcher", Some("Mock researcher"))]);
        let both = lists(&[KnownExtension::Mentions, KnownExtension::ThreadTools]);
        let with = |card: &BTreeSet<KnownExtension>, granted, asks| {
            mentions_metadata(&req, card, granted, asks).unwrap()
        };
        assert_eq!(
            with(&both, true, true)["coordinate"],
            json!({"tool": "ask_agent"})
        );
        assert!(
            with(&both, false, true).get("coordinate").is_none(),
            "no grant"
        );
        assert!(
            with(&both, true, false).get("coordinate").is_none(),
            "no ask_agent yet"
        );
        let only = lists(&[KnownExtension::Mentions]);
        assert!(
            with(&only, true, true).get("coordinate").is_none(),
            "no thread-tools"
        );
    }

    #[test]
    fn the_first_task_of_a_fork_has_its_offsets_moved_past_the_history_in_front_of_the_text() {
        let mut req = request(vec![mention("mock-researcher", Some("Mock researcher"))]);
        req.history = Some(ForkHistory {
            entries: vec![HistoryEntry {
                role: HistoryRole::Person,
                name: "person".to_owned(),
                // an emoji in the history: two UTF-16 units, four bytes
                text: "hi \u{1F604}".to_owned(),
            }],
            omitted: 0,
        });
        let preamble = history_preamble(req.history.as_ref().unwrap());
        let in_front = utf16_len(&preamble);
        assert!(
            in_front < preamble.len(),
            "the emoji makes bytes and units differ"
        );
        let metadata =
            mentions_metadata(&req, &lists(&[KnownExtension::Mentions]), false, false).unwrap();
        assert_eq!(metadata["mentions"][0]["start"], 4 + in_front);
        assert_eq!(metadata["mentions"][0]["end"], 20 + in_front);
    }
}
