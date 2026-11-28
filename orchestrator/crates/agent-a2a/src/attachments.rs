//! The files of a message on the A2A client side (`docs/api/attachments.md`, "The message side"):
//! one **URL part** per file, with its `mediaType` and `filename`, and never the bytes.
//!
//! The URL is a signed link and a credential while it lives. It is asked for here, while the
//! message is being sent, and goes into the message and nowhere else: not into the request, the
//! outbox, the event log or a log line. Nothing in this module logs a part, a link or a file.
//!
//! A file is never dropped: a message whose every file cannot be given a part is not sent.

use a2a::Part;
use orch_core::Classify;
use orch_ports::{AgentError, AttachmentLinks, SendContent, SendRequest};

/// The parts of the request's files, in the request's order; empty for a message with none, which
/// asks for no link at all.
///
/// # Errors
/// Nothing may be sent then. [`AgentError::Unsupported`] when the content cannot carry files (only
/// a text message does: a UI action and a session's start and close are one part);
/// [`AgentError::NotSent`] when this adapter was given nothing to make links with, or the store
/// will never make one; [`AgentError::Unreachable`] (retryable) when the store failed for now.
pub(crate) async fn attachment_parts(
    req: &SendRequest,
    links: Option<&dyn AttachmentLinks>,
) -> Result<Vec<Part>, AgentError> {
    if req.attachments.is_empty() {
        return Ok(Vec::new());
    }
    if !matches!(req.content, SendContent::Text(_)) {
        return Err(AgentError::Unsupported(
            "only a text message can carry attachments".to_owned(),
        ));
    }
    let Some(links) = links else {
        return Err(AgentError::not_sent(
            "this deployment cannot make a link for an attachment",
        ));
    };
    let mut parts = Vec::with_capacity(req.attachments.len());
    for file in &req.attachments {
        let signed = links.link(&file.key).await.map_err(|error| {
            // the store's error holds no URL and no credential (`ArtifactError`)
            tracing::warn!(%error, "no link for an attachment; the message is not sent");
            if error.is_retryable() {
                AgentError::unreachable("the artifact store made no link for an attachment")
                    .with_source(error)
            } else {
                AgentError::not_sent("the artifact store cannot make a link for an attachment")
            }
        })?;
        let part = Part::url(signed.url).with_media_type(file.media_type.clone());
        parts.push(match &file.filename {
            Some(name) => part.with_filename(name.clone()),
            None => part,
        });
    }
    Ok(parts)
}
