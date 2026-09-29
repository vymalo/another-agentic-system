//! Requests refused before the stream: the problems of `docs/api/agui.md`.

use axum::http::{HeaderMap, StatusCode, header};
use orch_agui_projection::InputError;
use orch_api::{ApiError, Problem};

/// The problem a request that [`orch_agui_projection`] refuses becomes: 400, 409 or 422.
pub(crate) fn input_error(e: &InputError) -> ApiError {
    let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::BAD_REQUEST);
    Problem::new(status, e.to_string()).into()
}

/// The binding answers `text/event-stream`, so a client that asks for nothing it can read (for
/// example the optional protobuf framing) gets 406 instead of a stream it cannot parse. No
/// `Accept`, `*/*` and `text/*` accept.
pub(crate) fn check_accept(headers: &HeaderMap) -> Result<(), ApiError> {
    let Some(accept) = headers.get(header::ACCEPT) else {
        return Ok(());
    };
    let accepted = accept.to_str().is_ok_and(|accept| {
        accept.split(',').any(|range| {
            let media = range.split(';').next().unwrap_or("").trim();
            matches!(media, "text/event-stream" | "text/*" | "*/*")
        })
    });
    if accepted {
        Ok(())
    } else {
        Err(Problem::new(
            StatusCode::NOT_ACCEPTABLE,
            "this endpoint answers text/event-stream",
        )
        .into())
    }
}

/// The body is JSON, declared as such. Besides being the binding, this keeps a cross-site form
/// from posting to a cookie-authenticated endpoint: a `text/plain` body needs no CORS preflight,
/// `application/json` does.
pub(crate) fn check_json(headers: &HeaderMap) -> Result<(), ApiError> {
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        })
        .is_some_and(|media| media == "application/json" || media.ends_with("+json"));
    if json {
        Ok(())
    } else {
        Err(Problem::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the request body must be application/json",
        )
        .into())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn with(name: header::HeaderName, value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(name, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn accept_admits_the_stream_and_the_wildcards() {
        assert!(check_accept(&HeaderMap::new()).is_ok());
        for ok in [
            "text/event-stream",
            "*/*",
            "text/*",
            "application/json, text/event-stream;q=0.9",
            " text/event-stream ; charset=utf-8",
        ] {
            assert!(check_accept(&with(header::ACCEPT, ok)).is_ok(), "{ok}");
        }
        for bad in ["application/json", "application/vnd.ag-ui.event+proto", ""] {
            assert!(check_accept(&with(header::ACCEPT, bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_body_must_declare_json() {
        for ok in [
            "application/json",
            "application/json; charset=utf-8",
            "Application/JSON",
        ] {
            assert!(check_json(&with(header::CONTENT_TYPE, ok)).is_ok(), "{ok}");
        }
        assert!(check_json(&HeaderMap::new()).is_err());
        for bad in [
            "text/plain",
            "application/x-www-form-urlencoded",
            "text/json",
        ] {
            assert!(
                check_json(&with(header::CONTENT_TYPE, bad)).is_err(),
                "{bad}"
            );
        }
    }
}
