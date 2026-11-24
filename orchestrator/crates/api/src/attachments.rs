//! `POST /api/threads/{threadId}/attachments`: a person's own file, sent from the browser in the
//! same origin (ADR 0067, wave 2 slice 12; `docs/api/attachments.md`).
//!
//! The route is the person's browser's face; the application decides ([`orch_app::App::store_attachment`]):
//! the permission, the size before anything is stored, the type and the name. This module only says
//! **how** the bytes arrive and how the reference is written back:
//!
//! - the body is the file itself, read in chunks and stopped at the deployment's cap before a byte
//!   over it is buffered, so an oversized body is refused before the store is called;
//! - the type is `Content-Type` (optional), the name `Content-Disposition`'s `filename` member
//!   (optional). The name never rides in the URL, so it is never in the request line or the log;
//! - a `201` names the file where every file of a thread is read — the artifact route — and
//!   carries that address in `Location`.

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures::StreamExt as _;
use orch_ports::{Ports, Principal};
use serde::Serialize;

use crate::ApiState;
use crate::problem::{ApiError, Problem};
use crate::routes::parse_thread_id;

/// What a kept file is: the content's hash, its size, the type it is served as, its name, and where
/// it is read. The same members as `docs/api/attachments.md`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentAnswer {
    sha256: String,
    size: u64,
    media_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    filename: Option<String>,
    url: String,
}

/// `POST /api/threads/{threadId}/attachments`.
pub(crate) async fn upload_attachment<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(thread): Path<String>,
    headers: HeaderMap,
    request: Request,
) -> Result<Response, ApiError> {
    let thread = parse_thread_id(&thread)?;
    let max = state.app.attachment_limit();
    let media_type = content_type(&headers)?;
    let filename = file_name(&headers)?;
    // The cheap half of the size rule: a body that says it is over the cap is refused before it is
    // read. The body is then read in chunks, so one that lies or has no length is stopped too.
    if let Some(declared) = content_length(&headers)
        && declared > max
    {
        return Err(too_large(max));
    }
    let bytes = read_body(request.into_body(), max).await?;
    let kept = state
        .app
        .store_attachment(
            &principal,
            thread,
            filename.as_deref(),
            media_type.as_deref(),
            &bytes,
        )
        .await?;
    let url = format!("/api/threads/{thread}/artifacts/{}", kept.sha256);
    let location = HeaderValue::from_str(&url).map_err(|_| {
        ApiError::from(Problem::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal error",
        ))
    })?;
    let mut response = (
        StatusCode::CREATED,
        axum::Json(AttachmentAnswer {
            sha256: kept.sha256,
            size: kept.size,
            media_type: kept.media_type,
            filename: kept.filename,
            url,
        }),
    )
        .into_response();
    response.headers_mut().insert(header::LOCATION, location);
    Ok(response)
}

/// The 413 the contract names, and the most bytes this deployment keeps.
fn too_large(max: u64) -> ApiError {
    Problem::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        format!("a file may be at most {max} bytes"),
    )
    .with_code("too_large")
    .into()
}

fn bad_filename() -> ApiError {
    Problem::bad_request("the file name is not usable")
        .with_code("bad_filename")
        .into()
}

fn content_length(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

/// `Content-Type` as it was sent, for the application to read (or refuse) as one media type. An
/// absent header is no declared type at all; one that is not text is `bad_media_type`'s.
fn content_type(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    match headers.get(header::CONTENT_TYPE) {
        None => Ok(None),
        Some(value) => value
            .to_str()
            .map(|text| Some(text.to_owned()))
            .map_err(|_| {
                Problem::bad_request("the media type is not one type/subtype")
                    .with_code("bad_media_type")
                    .into()
            }),
    }
}

/// The `filename` member of `Content-Disposition`, if there is one. The name is not in the URL, so
/// it is not in the request line and not in the orchestrator's log.
///
/// # Errors
/// A header that is not text, or a quoted name that never closes: `400 bad_filename`.
fn file_name(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(value) = headers.get(header::CONTENT_DISPOSITION) else {
        return Ok(None);
    };
    let text = value.to_str().map_err(|_| bad_filename())?;
    match filename_param(text) {
        None => Ok(None),
        Some(Ok(name)) => Ok(Some(name)),
        Some(Err(())) => Err(bad_filename()),
    }
}

/// The `filename` parameter of a `Content-Disposition` value, quoted (`"…"`, with `\` escaping the
/// next character) or a bare token. `None` when there is no `filename` parameter; `Some(Err(()))`
/// when the parameter is there and cannot be read (a quoted value that never closes).
fn filename_param(value: &str) -> Option<Result<String, ()>> {
    let mut rest = value;
    // the disposition type, up to the first ';'
    rest = rest.split_once(';').map_or("", |(_kind, rest)| rest);
    let mut found = None;
    while !rest.is_empty() {
        rest = rest.trim_start_matches([' ', '\t', ';']);
        if rest.is_empty() {
            break;
        }
        let (name, after_name) = match rest.split_once('=') {
            Some((name, after_name)) => (name.trim(), after_name),
            // a parameter with no value: skip it
            None => break,
        };
        let (read, after) = read_param_value(after_name);
        if name.eq_ignore_ascii_case("filename") {
            found = Some(read);
        }
        rest = after;
    }
    found
}

/// Reads one parameter value at the start of `value`, and returns it with what follows it. A quoted
/// value is read to its closing quote, a `\` taking the next character literally; a bare value runs
/// to the next `;`. A quoted value that never closes is `Err(())`.
fn read_param_value(value: &str) -> (Result<String, ()>, &str) {
    let value = value.trim_start();
    let Some(quoted) = value.strip_prefix('"') else {
        let (bare, after) = value.split_once(';').unwrap_or((value, ""));
        return (Ok(bare.trim().to_owned()), after);
    };
    let mut out = String::new();
    let mut chars = quoted.char_indices();
    while let Some((at, ch)) = chars.next() {
        match ch {
            '"' => return (Ok(out), &quoted[at + 1..]),
            '\\' => match chars.next() {
                Some((_, escaped)) => out.push(escaped),
                None => return (Err(()), ""),
            },
            other => out.push(other),
        }
    }
    (Err(()), "")
}

/// The body, read in chunks and stopped at `max`: the chunk that would pass it is a `413` and
/// nothing beyond is collected. A body that cannot be read is a `400`.
async fn read_body(body: Body, max: u64) -> Result<Vec<u8>, ApiError> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| {
            Problem::bad_request("the request body could not be read").with_code("bad_request")
        })?;
        if bytes.len() as u64 + chunk.len() as u64 > max {
            return Err(too_large(max));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
