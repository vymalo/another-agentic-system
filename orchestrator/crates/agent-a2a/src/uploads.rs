//! The large files of a message on the A2A client side (`uploads/v1`, ADR 0058, ADR 0008): what
//! the message carries when the live card lists the extension.
//!
//! The agent is told what the deployment offers (`maxBytes`, `ttlSecs`) under the extension's URI in
//! the message metadata, and the URI is activated in `A2A-Extensions` and `message.extensions`. It
//! is activated only when **all** hold, and fail closed: the card read for this very call lists
//! `uploads/v1` and `thread-tools/v1` (a card that lists the first alone is read as listing
//! neither: the tool is on the second's endpoint), a thread-tools grant was minted for this message
//! (so the agent has the endpoint and the token), and the request's grant carries an offer, which
//! the dispatcher gives only when the deployment's store can presign and the caller is the
//! thread's addressed agent. Any other message is the message it was. Nothing here uploads
//! anything: the agent asks the thread tools for a slot.

use std::collections::BTreeSet;

use orch_core::KnownExtension;
use orch_ports::SendRequest;
use serde_json::{Value, json};

/// The value of the extension's key in the message metadata, `{maxBytes, ttlSecs}`, or `None` when
/// the message does not activate the extension. `granted` is whether a thread-tools grant was minted
/// for this message.
pub(crate) fn uploads_metadata(
    req: &SendRequest,
    card_extensions: &BTreeSet<KnownExtension>,
    granted: bool,
) -> Option<Value> {
    let grant = req.thread_tools.as_ref()?;
    let offer = grant.uploads.filter(|offer| offer.is_valid())?;
    if !granted
        || !grant.caller.is_main()
        || !card_extensions.contains(&KnownExtension::Uploads)
        || !card_extensions.contains(&KnownExtension::ThreadTools)
    {
        return None;
    }
    Some(json!({ "maxBytes": offer.max_bytes, "ttlSecs": offer.ttl_secs }))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use orch_core::{AgentId, Caller, ThreadId, ToolsGrant, UploadOffer};
    use orch_ports::{AgentEndpoint, SendContent};

    use super::*;

    const OFFER: UploadOffer = UploadOffer {
        max_bytes: 1 << 30,
        ttl_secs: 300,
    };

    fn request(grant: Option<ToolsGrant>) -> SendRequest {
        SendRequest {
            endpoint: AgentEndpoint::a2a(AgentId::new("coder"), "http://agent/card", None),
            message_id: "m-1".into(),
            context_id: None,
            task_id: None,
            reference_task_ids: Vec::new(),
            content: SendContent::Text("hi".into()),
            release: None,
            ui_catalog: None,
            thread_tools: grant,
            history: None,
            steer: false,
            mentions: Vec::new(),
            tier: None,
            tier_cap: None,
        }
    }

    fn grant() -> ToolsGrant {
        ToolsGrant::main(ThreadId(uuid::Uuid::nil()), 2, AgentId::new("coder"))
            .with_uploads(Some(OFFER))
    }

    fn lists(extensions: &[KnownExtension]) -> BTreeSet<KnownExtension> {
        extensions.iter().copied().collect()
    }

    const BOTH: [KnownExtension; 2] = [KnownExtension::Uploads, KnownExtension::ThreadTools];

    #[test]
    fn the_limits_are_told_when_everything_holds() {
        let metadata = uploads_metadata(&request(Some(grant())), &lists(&BOTH), true).unwrap();
        assert_eq!(
            metadata,
            json!({"maxBytes": 1_073_741_824u64, "ttlSecs": 300})
        );
    }

    #[test]
    fn each_condition_alone_keeps_the_extension_off() {
        let on = request(Some(grant()));
        // a card that lists uploads alone, thread tools alone, or neither
        for card in [
            lists(&[KnownExtension::Uploads]),
            lists(&[KnownExtension::ThreadTools]),
            lists(&[KnownExtension::Steps, KnownExtension::Mentions]),
            lists(&[]),
        ] {
            assert!(uploads_metadata(&on, &card, true).is_none(), "{card:?}");
        }
        // no grant was minted for this message (no issuer, no keys)
        assert!(uploads_metadata(&on, &lists(&BOTH), false).is_none());
        // a request with no grant (the verifier's), and a grant that offers nothing
        assert!(uploads_metadata(&request(None), &lists(&BOTH), true).is_none());
        let none = request(Some(grant().with_uploads(None)));
        assert!(uploads_metadata(&none, &lists(&BOTH), true).is_none());
        // numbers a message could not carry
        for bad in [
            UploadOffer {
                max_bytes: 0,
                ..OFFER
            },
            UploadOffer {
                ttl_secs: 5,
                ..OFFER
            },
        ] {
            let req = request(Some(ToolsGrant {
                uploads: Some(bad),
                ..grant()
            }));
            assert!(
                uploads_metadata(&req, &lists(&BOTH), true).is_none(),
                "{bad:?}"
            );
        }
        // an asked agent: the orchestrator keeps no file of it
        let ask = ToolsGrant {
            caller: Caller::Ask(1),
            depth: 1,
            ..grant()
        };
        assert!(uploads_metadata(&request(Some(ask)), &lists(&BOTH), true).is_none());
    }
}
