//! Maps the SDK's [`A2AError`] onto the ports' [`AgentError`].
//!
//! The client SDK reports every transport failure (connect, timeout, TLS, an HTTP error status
//! answered without a JSON-RPC body) as `INTERNAL_ERROR` and loses the HTTP status, so those
//! are told apart by their message prefix, which the SDK builds itself.

use a2a::{A2AError, error_code};
use orch_ports::AgentError;

/// Message prefixes the SDK uses when the request never produced a JSON-RPC answer.
const TRANSPORT_PREFIXES: [&str; 2] = ["HTTP request failed", "failed to fetch agent card"];

/// Classifies an SDK error.
///
/// - unknown task, not cancelable, unsupported: their own variants;
/// - the agent refused the request for good (invalid request/params, content type, missing or
///   unsupported extension or version): [`AgentError::Rejected`];
/// - the connection failed: [`AgentError::Unreachable`];
/// - anything else (an answer that is not JSON-RPC, e.g. a proxy's 401 or 502 page; a stream cut
///   mid-message; a server-side internal error): [`AgentError::Protocol`].
pub(crate) fn classify(err: A2AError) -> AgentError {
    let detail = err.message.clone();
    match err.code {
        error_code::TASK_NOT_FOUND => AgentError::TaskNotFound(detail),
        error_code::TASK_NOT_CANCELABLE => AgentError::NotCancelable(detail),
        error_code::UNSUPPORTED_OPERATION
        | error_code::PUSH_NOTIFICATION_NOT_SUPPORTED
        | error_code::EXTENDED_CARD_NOT_CONFIGURED
        | error_code::METHOD_NOT_FOUND => AgentError::Unsupported(detail),
        error_code::INVALID_REQUEST
        | error_code::INVALID_PARAMS
        | error_code::PARSE_ERROR
        | error_code::CONTENT_TYPE_NOT_SUPPORTED
        | error_code::EXTENSION_SUPPORT_REQUIRED
        | error_code::VERSION_NOT_SUPPORTED => AgentError::Rejected(detail),
        error_code::INTERNAL_ERROR if TRANSPORT_PREFIXES.iter().any(|p| detail.starts_with(p)) => {
            AgentError::Unreachable(detail)
        }
        _ => AgentError::Protocol(detail),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn err(code: i32, message: &str) -> A2AError {
        A2AError {
            code,
            message: message.to_owned(),
            details: None,
        }
    }

    #[test]
    fn protocol_codes_map_to_their_variants() {
        assert!(matches!(
            classify(err(error_code::TASK_NOT_FOUND, "x")),
            AgentError::TaskNotFound(_)
        ));
        assert!(matches!(
            classify(err(error_code::TASK_NOT_CANCELABLE, "x")),
            AgentError::NotCancelable(_)
        ));
        for code in [
            error_code::UNSUPPORTED_OPERATION,
            error_code::METHOD_NOT_FOUND,
            error_code::PUSH_NOTIFICATION_NOT_SUPPORTED,
        ] {
            assert!(matches!(
                classify(err(code, "x")),
                AgentError::Unsupported(_)
            ));
        }
        for code in [
            error_code::INVALID_REQUEST,
            error_code::INVALID_PARAMS,
            error_code::CONTENT_TYPE_NOT_SUPPORTED,
            error_code::EXTENSION_SUPPORT_REQUIRED,
            error_code::VERSION_NOT_SUPPORTED,
        ] {
            let mapped = classify(err(code, "x"));
            assert!(matches!(mapped, AgentError::Rejected(_)), "{code}");
            assert!(!mapped.is_retryable());
        }
    }

    #[test]
    fn transport_failures_are_unreachable_and_other_internal_errors_are_protocol() {
        let down = classify(err(
            error_code::INTERNAL_ERROR,
            "HTTP request failed: connection refused",
        ));
        assert!(matches!(down, AgentError::Unreachable(_)));
        assert!(down.is_retryable());

        let card = classify(err(
            error_code::INTERNAL_ERROR,
            "failed to fetch agent card: timeout",
        ));
        assert!(matches!(card, AgentError::Unreachable(_)));

        let proxy_page = classify(err(
            error_code::INTERNAL_ERROR,
            "failed to parse JSON-RPC response: expected value",
        ));
        assert!(matches!(proxy_page, AgentError::Protocol(_)));
        assert!(proxy_page.is_retryable());

        assert!(matches!(
            classify(err(error_code::INTERNAL_ERROR, "subscription fell behind")),
            AgentError::Protocol(_)
        ));
        assert!(matches!(
            classify(err(-1, "weird")),
            AgentError::Protocol(_)
        ));
    }
}
