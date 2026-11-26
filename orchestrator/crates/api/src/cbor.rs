//! `application/cbor` as a second media type on every JSON body of the resource API (ADR 0100).
//!
//! One layer, not a change per handler. A request whose `Content-Type` is `application/cbor` is
//! decoded by ciborium into the same serde value the JSON path reads and handed on as JSON; an
//! answer whose body is JSON is encoded as CBOR when the request's `Accept` prefers it. JSON stays
//! the default for a client that says nothing, so a JSON-only client sees no change.
//!
//! What it leaves alone: any answer that is not `application/json` or `application/problem+json`
//! (the SSE streams, files, the metrics text), and any request body of another type (a handler
//! that wants a media type answers 415 itself).
//!
//! Refusals ride the problem envelope: 406 when `Accept` lists neither media type for a JSON
//! answer, 400 for CBOR that does not decode, nests deeper than [`MAX_DEPTH`], uses an
//! indefinite-length item or has a map key that is not text, 413 above [`MAX_BODY_BYTES`].

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use ciborium_ll::{Decoder, Header};

use crate::problem::Problem;

/// The CBOR media type.
pub const CBOR: &str = "application/cbor";
/// The same body limit as JSON (`edge_layers`).
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// How deep arrays and maps may nest in a CBOR request.
pub const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wants {
    Json,
    Cbor,
    Neither,
}

/// What the `Accept` header asks of a JSON answer: the listed range with the highest `q` wins, an
/// exact type beats a wildcard at the same `q`, JSON beats CBOR on a tie, and no header means JSON.
fn wants(headers: &HeaderMap) -> Wants {
    let Some(accept) = headers.get(header::ACCEPT) else {
        return Wants::Json;
    };
    let Ok(accept) = accept.to_str() else {
        return Wants::Json;
    };
    // (q in thousandths, specificity) of the best range for each media type.
    let mut json: Option<(u32, u8)> = None;
    let mut cbor: Option<(u32, u8)> = None;
    let mut any = false;
    for range in accept.split(',') {
        let mut parts = range.split(';');
        let essence = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        if essence.is_empty() {
            continue;
        }
        any = true;
        let q = parts
            .filter_map(|p| p.trim().strip_prefix("q="))
            .find_map(|v| v.trim().parse::<f32>().ok())
            .map_or(1000, |q| (q.clamp(0.0, 1.0) * 1000.0) as u32);
        let (json_spec, cbor_spec) = match essence.as_str() {
            "application/json" => (Some(3), None),
            "application/cbor" => (None, Some(3)),
            "application/*" => (Some(2), Some(2)),
            "*/*" => (Some(1), Some(1)),
            _ => (None, None),
        };
        for (slot, spec) in [(&mut json, json_spec), (&mut cbor, cbor_spec)] {
            if let Some(spec) = spec {
                let better = slot.is_none_or(|(bq, bs)| (spec, q) > (bs, bq));
                if better {
                    *slot = Some((q, spec));
                }
            }
        }
    }
    if !any {
        return Wants::Json;
    }
    let j = json.map_or(0, |(q, _)| q);
    let c = cbor.map_or(0, |(q, _)| q);
    match (j, c) {
        (0, 0) => Wants::Neither,
        (j, c) if c > j => Wants::Cbor,
        _ => Wants::Json,
    }
}

fn essence(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::CONTENT_TYPE)?
        .to_str()
        .ok()?
        .split(';')
        .next()
        .map(|s| s.trim().to_ascii_lowercase())
}

fn is_json_answer(response: &Response) -> bool {
    matches!(
        essence(response.headers()).as_deref(),
        Some("application/json" | "application/problem+json")
    )
}

fn problem(status: StatusCode, detail: &str) -> Response {
    Problem::new(status, detail).into_response()
}

/// The layer: decodes a CBOR request, encodes the answer when asked, answers 406 and 400.
pub async fn negotiate(mut request: Request, next: Next) -> Response {
    let wants = wants(request.headers());
    let sends_cbor = essence(request.headers()).as_deref() == Some(CBOR);
    let safe = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    let accepts_stream = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/event-stream"));
    // A write is not performed for a client that could not read the answer. A read is judged on
    // its answer, because only then is it known whether the answer is JSON at all.
    if wants == Wants::Neither && !safe && !accepts_stream {
        return not_acceptable();
    }
    if sends_cbor {
        let (mut parts, body) = request.into_parts();
        let bytes = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return problem(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "the request body is larger than the limit",
                );
            }
        };
        let json = match to_json(&bytes) {
            Ok(json) => json,
            Err(detail) => return problem(StatusCode::BAD_REQUEST, &detail),
        };
        parts.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        parts.headers.remove(header::CONTENT_LENGTH);
        request = Request::from_parts(parts, Body::from(json));
    }
    let response = next.run(request).await;
    if !is_json_answer(&response) {
        return response;
    }
    let mut response = match wants {
        Wants::Json => response,
        Wants::Neither => not_acceptable(),
        Wants::Cbor => encode_answer(response).await,
    };
    response
        .headers_mut()
        .append(header::VARY, HeaderValue::from_static("Accept"));
    response
}

fn not_acceptable() -> Response {
    problem(
        StatusCode::NOT_ACCEPTABLE,
        "the answer is application/json or application/cbor",
    )
}

async fn encode_answer(response: Response) -> Response {
    let (mut parts, body) = response.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return problem(
            StatusCode::INTERNAL_SERVER_ERROR,
            "the answer could not be read",
        );
    };
    let is_problem = essence(&parts.headers).as_deref() == Some("application/problem+json");
    let value: serde_json::Value = if bytes.is_empty() {
        return Response::from_parts(parts, Body::from(bytes));
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(_) => return Response::from_parts(parts, Body::from(bytes)),
        }
    };
    let mut out = Vec::with_capacity(bytes.len());
    if ciborium::into_writer(&value, &mut out).is_err() {
        return Response::from_parts(parts, Body::from(bytes));
    }
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(if is_problem {
            "application/problem+cbor"
        } else {
            CBOR
        }),
    );
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.remove(header::ETAG);
    Response::from_parts(parts, Body::from(out))
}

/// CBOR to JSON bytes through serde, or the words of a 400.
fn to_json(cbor: &[u8]) -> Result<Bytes, String> {
    if cbor.is_empty() {
        return Ok(Bytes::new());
    }
    reject_indefinite(cbor)?;
    let value: serde_json::Value = ciborium::de::from_reader_with_recursion_limit(cbor, MAX_DEPTH)
        .map_err(|e| format!("the body is not valid CBOR for this API: {}", short(&e)))?;
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|_| "the body cannot be read as the API's JSON".to_owned())
}

fn short(e: &impl std::fmt::Display) -> String {
    let text = e.to_string();
    text.chars().take(120).collect()
}

/// Walks the items once and refuses an indefinite-length string, array or map, and nesting deeper
/// than [`MAX_DEPTH`], before serde reads the value.
fn reject_indefinite(cbor: &[u8]) -> Result<(), String> {
    const NO: &str = "the body uses an indefinite-length CBOR item, which this API does not read";
    let mut dec = Decoder::from(cbor);
    // Items still to read at each open level; a level closes at zero.
    let mut open: Vec<u64> = vec![1];
    let mut scratch = [0u8; 256];
    while let Some(left) = open.last_mut() {
        if *left == 0 {
            open.pop();
            continue;
        }
        *left -= 1;
        let header = dec
            .pull()
            .map_err(|_| "the body is not valid CBOR".to_owned())?;
        match header {
            Header::Array(None) | Header::Map(None) | Header::Bytes(None) | Header::Text(None) => {
                return Err(NO.to_owned());
            }
            Header::Break => return Err(NO.to_owned()),
            Header::Array(Some(n)) => push(&mut open, n as u64)?,
            Header::Map(Some(n)) => push(&mut open, (n as u64).saturating_mul(2))?,
            Header::Tag(_) => {
                // The tagged item is the next one: it does not count as a new item.
                if let Some(left) = open.last_mut() {
                    *left += 1;
                }
            }
            Header::Bytes(Some(n)) => {
                let mut segments = dec.bytes(Some(n));
                while let Some(mut segment) = segments
                    .pull()
                    .map_err(|_| "the body is not valid CBOR".to_owned())?
                {
                    while segment
                        .pull(&mut scratch)
                        .map_err(|_| "the body is not valid CBOR".to_owned())?
                        .is_some()
                    {}
                }
            }
            Header::Text(Some(n)) => {
                let mut segments = dec.text(Some(n));
                while let Some(mut segment) = segments
                    .pull()
                    .map_err(|_| "the body is not valid CBOR".to_owned())?
                {
                    while segment
                        .pull(&mut scratch)
                        .map_err(|_| "the body is not valid CBOR".to_owned())?
                        .is_some()
                    {}
                }
            }
            Header::Positive(_) | Header::Negative(_) | Header::Float(_) | Header::Simple(_) => {}
        }
    }
    Ok(())
}

fn push(open: &mut Vec<u64>, items: u64) -> Result<(), String> {
    if open.len() > MAX_DEPTH {
        return Err(format!("the body nests deeper than {MAX_DEPTH} levels"));
    }
    open.push(items);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn accept(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::ACCEPT, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn accept_negotiation() {
        assert_eq!(wants(&HeaderMap::new()), Wants::Json);
        assert_eq!(wants(&accept("*/*")), Wants::Json);
        assert_eq!(wants(&accept("application/json")), Wants::Json);
        assert_eq!(wants(&accept("application/cbor")), Wants::Cbor);
        assert_eq!(
            wants(&accept("application/cbor, application/json;q=0.5")),
            Wants::Cbor
        );
        assert_eq!(
            wants(&accept("application/cbor;q=0.2, */*;q=0.9")),
            Wants::Json
        );
        assert_eq!(
            wants(&accept("application/json, application/cbor")),
            Wants::Json
        );
        assert_eq!(wants(&accept("text/html")), Wants::Neither);
        assert_eq!(
            wants(&accept("application/json;q=0, text/html")),
            Wants::Neither
        );
    }

    #[test]
    fn cbor_in_json_out() {
        let mut cbor = Vec::new();
        ciborium::into_writer(
            &serde_json::json!({"a": [1, "x", null], "b": {"c": true}}),
            &mut cbor,
        )
        .unwrap();
        let json = to_json(&cbor).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"a": [1, "x", null], "b": {"c": true}})
        );
    }

    #[test]
    fn refusals() {
        // indefinite-length array: 0x9f 0x01 0xff
        assert!(
            to_json(&[0x9f, 0x01, 0xff])
                .unwrap_err()
                .contains("indefinite")
        );
        // indefinite-length text
        assert!(
            to_json(&[0x7f, 0x61, 0x61, 0xff])
                .unwrap_err()
                .contains("indefinite")
        );
        // truncated
        assert!(to_json(&[0xa1, 0x61]).is_err());
        // too deep: 100 nested arrays
        let deep: Vec<u8> = std::iter::repeat_n(0x81, 100).chain([0x00]).collect();
        assert!(to_json(&deep).unwrap_err().contains("nests"));
        // a map key that is not text
        assert!(to_json(&[0xa1, 0x01, 0x02]).is_err());
    }
}
