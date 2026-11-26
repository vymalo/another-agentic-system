//! CBOR as a second media type of the JSON API (ADR 0102), negotiated in one place.
//!
//! A request body sent as `application/cbor` is checked, decoded by `ciborium` into the JSON value
//! the handlers already read and handed on as `application/json`; an answer is encoded as CBOR when
//! `Accept` prefers it. The handlers, their types and the schemas are the same for both encodings,
//! so the same request in either gives the same value. Server-sent events are never touched.

use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, VARY};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::problem::Problem;

/// The body limit of a request, the same as the JSON one (`edge_layers`).
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
/// The most an answer converted to CBOR may weigh.
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
/// How deeply a CBOR item may nest.
const MAX_DEPTH: usize = 64;

const CBOR: &str = "application/cbor";

/// What a client prefers for an answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Preference {
    Json,
    Cbor,
    /// `Accept` names nothing this API can serve.
    Neither,
}

fn media_type(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// The quality of `ranges` for the first of `names` that is listed, else a wildcard, else none.
fn quality(entries: &[(String, f32)], names: &[&str], wild: &[&str]) -> f32 {
    for set in [names, wild] {
        if let Some((_, q)) = entries.iter().find(|(m, _)| set.contains(&m.as_str())) {
            return *q;
        }
    }
    0.0
}

fn preference(headers: &HeaderMap) -> Preference {
    let Some(raw) = headers.get(ACCEPT).and_then(|v| v.to_str().ok()) else {
        return Preference::Json;
    };
    if raw.trim().is_empty() {
        return Preference::Json;
    }
    let entries: Vec<(String, f32)> = raw
        .split(',')
        .map(|part| {
            let q = part
                .split(';')
                .skip(1)
                .find_map(|p| p.trim().strip_prefix("q="))
                .and_then(|q| q.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            (media_type(part), q)
        })
        .collect();
    // A client reading the event stream is not asking for a body this layer converts.
    if entries.iter().any(|(m, _)| m == "text/event-stream") {
        return Preference::Json;
    }
    let json = quality(
        &entries,
        &["application/json", "application/problem+json"],
        &["application/*", "*/*"],
    );
    let cbor = quality(&entries, &[CBOR], &["application/*", "*/*"]);
    if cbor > json && cbor > 0.0 {
        Preference::Cbor
    } else if json > 0.0 {
        Preference::Json
    } else {
        Preference::Neither
    }
}

fn problem(status: StatusCode, code: &str, detail: &str) -> Response {
    Problem::new(status, detail).with_code(code).into_response()
}

/// Walks one CBOR item and everything inside it, refusing what the API does not take: an
/// indefinite length, a reserved or oversized header, nesting beyond [`MAX_DEPTH`], a length that
/// runs past the input. Returns the rest of the input.
fn skip_item(input: &[u8], depth: usize) -> Result<&[u8], &'static str> {
    if depth > MAX_DEPTH {
        return Err("the item nests too deeply");
    }
    let (&head, mut rest) = input.split_first().ok_or("the item is cut short")?;
    let major = head >> 5;
    let info = head & 0x1f;
    let argument = match info {
        0..=23 => u64::from(info),
        24..=27 => {
            let width = 1usize << (info - 24);
            if rest.len() < width {
                return Err("the item is cut short");
            }
            let (num, tail) = rest.split_at(width);
            rest = tail;
            num.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b))
        }
        31 => return Err("an indefinite-length item is not accepted"),
        _ => return Err("the item has a reserved header"),
    };
    match major {
        0 | 1 | 7 => Ok(rest),
        2 | 3 => {
            let len = usize::try_from(argument).map_err(|_| "the item is too long")?;
            if rest.len() < len {
                return Err("the item is cut short");
            }
            Ok(&rest[len..])
        }
        4 | 5 => {
            let count = if major == 5 {
                argument.checked_mul(2)
            } else {
                Some(argument)
            }
            .ok_or("the item is too long")?;
            if count > rest.len() as u64 {
                return Err("the item is cut short");
            }
            for _ in 0..count {
                rest = skip_item(rest, depth + 1)?;
            }
            Ok(rest)
        }
        _ => skip_item(rest, depth + 1),
    }
}

/// CBOR to the JSON value the handlers read, or why not.
fn cbor_to_json(bytes: &[u8]) -> Result<Vec<u8>, &'static str> {
    if !skip_item(bytes, 0)?.is_empty() {
        return Err("the body holds more than one item");
    }
    let mut reader = bytes;
    let value: serde_json::Value =
        ciborium::de::from_reader_with_recursion_limit(&mut reader, MAX_DEPTH)
            .map_err(|_| "the body is not CBOR that this API reads (text keys, no byte strings)")?;
    serde_json::to_vec(&value).map_err(|_| "the body cannot be read")
}

fn append_vary(headers: &mut HeaderMap) {
    let listed = headers
        .get_all(VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| {
            v.split(',')
                .any(|t| t.trim() == "*" || t.trim().eq_ignore_ascii_case("accept"))
        });
    if !listed {
        headers.append(VARY, HeaderValue::from_static("Accept"));
    }
}

/// The negotiation, as a middleware over the resource API.
pub async fn negotiate(mut request: Request, next: Next) -> Response {
    let wants = preference(request.headers());
    if wants == Preference::Neither {
        return problem(
            StatusCode::NOT_ACCEPTABLE,
            "not_acceptable",
            "this API answers application/json and application/cbor",
        );
    }

    let sent_cbor = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| media_type(v) == CBOR);
    if sent_cbor {
        let (mut parts, body) = request.into_parts();
        let Ok(bytes) = to_bytes(body, MAX_REQUEST_BYTES).await else {
            return problem(
                StatusCode::PAYLOAD_TOO_LARGE,
                "payload_too_large",
                "the body is too large",
            );
        };
        let json = match cbor_to_json(&bytes) {
            Ok(json) => json,
            Err(why) => return problem(StatusCode::BAD_REQUEST, "bad_cbor", why),
        };
        parts
            .headers
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        parts
            .headers
            .insert(CONTENT_LENGTH, HeaderValue::from(json.len()));
        request = Request::from_parts(parts, Body::from(json));
    }

    let response = next.run(request).await;
    finish(response, wants).await
}

async fn finish(response: Response, wants: Preference) -> Response {
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| media_type(v) == "application/json");
    if !is_json {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    append_vary(&mut parts.headers);
    if wants != Preference::Cbor {
        return Response::from_parts(parts, body);
    }
    let Ok(bytes) = to_bytes(body, MAX_RESPONSE_BYTES).await else {
        return problem(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the answer could not be encoded",
        );
    };
    let mut out = Vec::with_capacity(bytes.len());
    let encoded = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| ciborium::ser::into_writer(&v, &mut out).ok());
    if encoded.is_none() {
        return problem(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the answer could not be encoded",
        );
    }
    parts.headers.remove(CONTENT_LENGTH);
    parts
        .headers
        .insert(CONTENT_TYPE, HeaderValue::from_static(CBOR));
    Response::from_parts(parts, Body::from(out))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn headers(accept: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(a) = accept {
            h.insert(ACCEPT, HeaderValue::from_str(a).unwrap());
        }
        h
    }

    #[test]
    fn accept_is_negotiated() {
        assert_eq!(preference(&headers(None)), Preference::Json);
        assert_eq!(preference(&headers(Some("*/*"))), Preference::Json);
        assert_eq!(
            preference(&headers(Some("application/json"))),
            Preference::Json
        );
        assert_eq!(
            preference(&headers(Some("application/cbor"))),
            Preference::Cbor
        );
        assert_eq!(
            preference(&headers(Some("application/cbor, application/json;q=0.5"))),
            Preference::Cbor
        );
        assert_eq!(
            preference(&headers(Some("application/json, application/cbor;q=0.1"))),
            Preference::Json
        );
        assert_eq!(preference(&headers(Some("text/html"))), Preference::Neither);
        assert_eq!(
            preference(&headers(Some("application/cbor;q=0, application/json;q=0"))),
            Preference::Neither
        );
        assert_eq!(
            preference(&headers(Some("text/event-stream"))),
            Preference::Json
        );
    }

    #[test]
    fn refuses_what_the_api_does_not_take() {
        // indefinite array, nested too deep, cut short, trailing item
        assert!(cbor_to_json(&[0x9f, 0xff]).is_err());
        assert!(cbor_to_json(&[0x81; 100]).is_err());
        assert!(cbor_to_json(&[0x82, 0x01]).is_err());
        assert!(cbor_to_json(&[0x01, 0x02]).is_err());
        assert!(
            cbor_to_json(&[0x41, 0x00]).is_err(),
            "a byte string is not JSON"
        );
        assert_eq!(
            cbor_to_json(&[0xa1, 0x61, 0x61, 0x01]).unwrap(),
            br#"{"a":1}"#
        );
    }
}
