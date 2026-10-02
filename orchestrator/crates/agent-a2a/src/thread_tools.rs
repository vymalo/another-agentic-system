//! The thread-tools grant on the A2A client side (`thread-tools/v1`, ADR 0023, ADR 0008): what
//! the message carries when the live card lists the extension.
//!
//! The adapter mints the token **when it sends**, from the non-secret
//! [`ToolsGrant`] on the send request, and only when all three hold: the card read for this very
//! call lists `thread-tools/v1` (exact URI, fail closed), the request belongs to a thread (the
//! verifier's does not), and the adapter has an issuer, that is, keys and the URL agents reach
//! the orchestrator at ([`A2aConfig::thread_tools`](crate::A2aConfig::thread_tools)). The agent
//! then receives, under the extension's URI in the message metadata, `{url, token, expiresAt}`,
//! and the URI is activated in `A2A-Extensions` and `message.extensions`.
//!
//! The token goes into the message and nowhere else: not into the event log, not into the outbox
//! payload (neither can hold it: the request carries only the grant), not into a log line
//! ([`ThreadToolsGrant`] prints `[redacted]`, and nothing here logs the metadata). A card without
//! the URI, an adapter without keys and a request without a grant get exactly the message they
//! got before the extension existed.

use orch_core::{AttachedServer, KnownExtension, ToolsGrant};
use orch_thread_token::{ThreadToolsGrant, ThreadToolsIssuer};
use secrecy::ExposeSecret;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Mints the grant for a message, or `None` when the message gets none: the live card does not
/// list the extension, the request has no grant (the verifier's), or there is no issuer. A token
/// that cannot be minted (a grant whose fields disagree: a bug, never a condition of a request) is
/// logged without the grant's secrets and leaves the agent without tools, which every tool
/// degrades to; it never fails the message.
pub(crate) fn mint(
    issuer: Option<&ThreadToolsIssuer>,
    grant: Option<&ToolsGrant>,
    message_id: &str,
    card_extensions: &BTreeSet<KnownExtension>,
    now: jiff::Timestamp,
) -> Option<ThreadToolsGrant> {
    if !card_extensions.contains(&KnownExtension::ThreadTools) {
        return None;
    }
    let (issuer, grant) = (issuer?, grant?);
    match issuer.grant(grant, message_id, now) {
        Ok(minted) => Some(minted),
        Err(error) => {
            tracing::error!(
                thread = %grant.thread,
                agent = %grant.agent,
                %error,
                "no thread-tools token could be minted; the agent is given none"
            );
            None
        }
    }
}

/// The value of the extension's key in the message metadata: `{url, token, expiresAt}` and, when
/// servers are attached to the thread that the agent may use, `attached`
/// (`[{server, name, description?}]`, [`docs/api/thread-tools-v1.md`]): ids, names and
/// descriptions, **never a URL, a header or a credential** (`AttachedServer` has none). `expiresAt`
/// is the token's `exp` as RFC 3339.
///
/// [`docs/api/thread-tools-v1.md`]: ../../../../docs/api/thread-tools-v1.md#the-attached-member
pub fn thread_tools_metadata(grant: &ThreadToolsGrant, attached: &[AttachedServer]) -> Value {
    let mut metadata = json!({
        "url": grant.url,
        "token": grant.token.expose_secret(),
        "expiresAt": grant.expires_at.to_string(),
    });
    if !attached.is_empty() {
        let servers: Vec<Value> = attached
            .iter()
            .map(|s| {
                let mut server = json!({"server": s.id, "name": s.name});
                if let Some(description) = &s.description {
                    server["description"] = json!(description);
                }
                server
            })
            .collect();
        metadata["attached"] = Value::Array(servers);
    }
    metadata
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::time::Duration;

    use orch_core::{AgentId, ThreadId};
    use secrecy::SecretString;

    use super::*;

    fn issuer() -> ThreadToolsIssuer {
        let key = SecretString::from("k".repeat(32));
        ThreadToolsIssuer::new(
            orch_thread_token::ThreadToolsKeys::new(key, None).unwrap(),
            "http://orchestrator:8080",
            Duration::from_secs(7200),
        )
        .unwrap()
    }

    fn grant() -> ToolsGrant {
        ToolsGrant::main(ThreadId(uuid::Uuid::nil()), 2, AgentId::new("coder"))
    }

    fn offers(extensions: &[KnownExtension]) -> BTreeSet<KnownExtension> {
        extensions.iter().copied().collect()
    }

    fn now() -> jiff::Timestamp {
        jiff::Timestamp::from_second(1_790_856_000).unwrap()
    }

    #[test]
    fn a_token_is_minted_only_for_a_card_that_lists_the_extension_a_grant_and_an_issuer() {
        let (issuer, grant) = (issuer(), grant());
        let listed = offers(&[KnownExtension::ThreadTools]);
        let minted = mint(Some(&issuer), Some(&grant), "m-1", &listed, now()).unwrap();
        assert_eq!(
            minted.url,
            "http://orchestrator:8080/thread-tools/00000000-0000-0000-0000-000000000000/mcp"
        );
        // no issuer, no grant, a card without the extension (or with every other one)
        assert!(mint(None, Some(&grant), "m-1", &listed, now()).is_none());
        assert!(mint(Some(&issuer), None, "m-1", &listed, now()).is_none());
        for others in [
            offers(&[]),
            offers(&[
                KnownExtension::UiCatalog,
                KnownExtension::Steps,
                KnownExtension::Mentions,
                KnownExtension::TextStream,
            ]),
        ] {
            assert!(mint(Some(&issuer), Some(&grant), "m-1", &others, now()).is_none());
        }
        // a grant that disagrees is never minted, and does not fail the message
        let broken = ToolsGrant { depth: 3, ..grant };
        assert!(mint(Some(&issuer), Some(&broken), "m-1", &listed, now()).is_none());
    }

    #[test]
    fn the_metadata_is_the_url_the_token_and_the_expiry() {
        let minted = mint(
            Some(&issuer()),
            Some(&grant()),
            "m-1",
            &offers(&[KnownExtension::ThreadTools]),
            now(),
        )
        .unwrap();
        let metadata = thread_tools_metadata(&minted, &[]);
        assert_eq!(metadata["url"], json!(minted.url));
        assert_eq!(metadata["token"], json!(minted.token.expose_secret()));
        assert_eq!(metadata["expiresAt"], "2026-10-01T14:00:00Z");
        assert_eq!(metadata.as_object().unwrap().len(), 3);
        // and `Debug` of the grant it came from never shows the token
        assert!(!format!("{minted:?}").contains(minted.token.expose_secret()));
    }
}
