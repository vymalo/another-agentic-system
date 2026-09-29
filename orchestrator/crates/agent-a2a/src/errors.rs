//! Maps the SDK's [`A2AError`] onto the ports' [`AgentError`].
//!
//! The client SDK reports every transport failure (connect, timeout, TLS, an HTTP error status
//! answered without a JSON-RPC body) as `INTERNAL_ERROR` and loses the HTTP status, so those
//! are told apart by their message prefix, which the SDK builds itself.

use a2a::{A2AError, error_code};
use orch_ports::AgentError;

/// Message prefixes the SDK uses when the request never produced a JSON-RPC answer.
const TRANSPORT_PREFIXES: [&str; 2] = ["HTTP request failed", "failed to fetch agent card"];

/// The HTTP status of an error the SDK built from a bare HTTP answer (`HTTP 401 Unauthorized:
/// ..`, the REST transport's spelling), if it is one.
fn http_status(message: &str) -> Option<u16> {
    let rest = message.strip_prefix("HTTP ")?;
    rest.get(..3)?.parse().ok()
}

/// Classifies an SDK error.
///
/// - unknown task, not cancelable, unsupported: their own variants;
/// - the agent refused the request for good (invalid request/params, content type, missing or
///   unsupported extension or version): [`AgentError::Rejected`];
/// - the connection failed, or the answer was a bare `HTTP 408`/`5xx`: [`AgentError::Unreachable`];
/// - a bare `HTTP 401`/`403`: [`AgentError::Unauthenticated`]; `HTTP 429`:
///   [`AgentError::RateLimited`] (the SDK drops the headers, so without `retry_after`);
/// - anything else (an answer that is not JSON-RPC, e.g. a proxy's page over JSON-RPC, whose
///   status the SDK drops; a stream cut mid-message; a server-side internal error):
///   [`AgentError::Protocol`].
///
/// The peer's own message is kept for the variants that carry peer text. For the others the
/// SDK error is boxed as the `source`, so it reaches the operator's log and not the chat.
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
        error_code::INTERNAL_ERROR => match http_status(&detail) {
            Some(401 | 403) => AgentError::unauthenticated("the agent answered HTTP 401 or 403"),
            Some(429) => AgentError::RateLimited { retry_after: None },
            Some(408 | 500..=599) => {
                AgentError::unreachable("the agent answered a server error").with_source(err)
            }
            _ if TRANSPORT_PREFIXES.iter().any(|p| detail.starts_with(p)) => {
                AgentError::unreachable("the request to the agent failed").with_source(err)
            }
            _ => AgentError::protocol("unexpected answer from the agent").with_source(err),
        },
        _ => AgentError::protocol("unexpected answer from the agent").with_source(err),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use orch_core::{Classify as _, ErrorClass};

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
        assert!(matches!(down, AgentError::Unreachable { .. }));
        assert!(down.is_retryable());

        let card = classify(err(
            error_code::INTERNAL_ERROR,
            "failed to fetch agent card: timeout",
        ));
        assert!(matches!(card, AgentError::Unreachable { .. }));

        let proxy_page = classify(err(
            error_code::INTERNAL_ERROR,
            "failed to parse JSON-RPC response: expected value",
        ));
        assert!(matches!(proxy_page, AgentError::Protocol { .. }));
        assert!(proxy_page.is_retryable());

        assert!(matches!(
            classify(err(error_code::INTERNAL_ERROR, "subscription fell behind")),
            AgentError::Protocol { .. }
        ));
        assert!(matches!(
            classify(err(-1, "weird")),
            AgentError::Protocol { .. }
        ));
    }

    #[test]
    fn bare_http_statuses_of_the_rest_transport_are_told_apart() {
        for (message, class) in [
            ("HTTP 401 Unauthorized: nope", ErrorClass::Unauthenticated),
            ("HTTP 403 Forbidden: ", ErrorClass::Unauthenticated),
            ("HTTP 429 Too Many Requests: slow", ErrorClass::RateLimited),
            ("HTTP 503 Service Unavailable: x", ErrorClass::Transient),
            ("HTTP 408 Request Timeout: x", ErrorClass::Transient),
            ("HTTP 404 Not Found: x", ErrorClass::Transient),
        ] {
            let mapped = classify(err(error_code::INTERNAL_ERROR, message));
            assert_eq!(mapped.class(), class, "{message}");
        }
    }

    #[test]
    fn the_sdk_error_is_the_source_and_never_in_the_public_text() {
        let mapped = classify(err(
            error_code::INTERNAL_ERROR,
            "HTTP request failed: error sending request for url (http://10.1.2.3:9/a2a)",
        ));
        let source = std::error::Error::source(&mapped).expect("a source");
        assert!(source.downcast_ref::<A2AError>().is_some());
        assert!(!mapped.to_string().contains("10.1.2.3"));
        assert!(!mapped.public_detail().contains("10.1.2.3"));
        assert!(orch_core::report(&mapped).contains("10.1.2.3"));
    }

    #[test]
    fn a_status_message_that_is_not_http_is_not_mistaken_for_one() {
        assert_eq!(http_status("HTTP 401 Unauthorized"), Some(401));
        assert_eq!(http_status("HTTP request failed: x"), None);
        assert_eq!(http_status("HTTP 4"), None);
    }
}
