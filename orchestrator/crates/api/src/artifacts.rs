//! `GET /api/threads/{threadId}/artifacts/{sha256}`: a file an agent handed over, from the artifact
//! store (ADR 0032, decision 8).
//!
//! Who may read it is [`orch_app::App::open_artifact`]'s (the permission `artifact.read` of ADR 0033
//! over the thread); this module only says **how** a file is sent, and that is the
//! whole of the safety of serving what an agent wrote:
//!
//! - the body is **streamed** from the store, never held whole (an SVG is the one exception: it is
//!   read to be sanitized, and only up to [`MAX_SVG_INLINE_BYTES`]);
//! - a file is shown **inline only** for the preview types (png, jpeg, gif, webp, svg, text/plain,
//!   application/json: [`orch_core::Preview`]) and only without `?download=1`; every other type,
//!   and every download, is an `attachment`;
//! - an inline SVG is the **sanitized** one ([`orch_svg_clean`]); one that cannot be sanitized, or is
//!   too large to be, is sent as an attachment instead; a download is the original bytes;
//! - every response carries `X-Content-Type-Options: nosniff`, a `Content-Security-Policy` that
//!   allows nothing but the file's own images and inline styles and **sandboxes** it,
//!   and `Cache-Control: private, max-age=31536000, immutable` (the URL is the content's hash, and
//!   the file is the person's);
//! - the file name in `Content-Disposition` is cleaned of everything that could end the header or
//!   mislead a person (a fallback of printable ASCII, and the real name in the RFC 5987 form).

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::Response;
use futures::StreamExt as _;
use orch_core::Preview;
use orch_ports::{ArtifactMeta, Ports, Principal};
use serde::Deserialize;

use crate::ApiState;
use crate::problem::{ApiError, Problem};
use crate::routes::parse_thread_id;

/// The most an SVG may be to be sanitized and sent inline: 2 MiB. A larger one is an attachment.
pub const MAX_SVG_INLINE_BYTES: u64 = 2 * 1024 * 1024;

/// The policy of every file response: nothing loads, nothing runs, and the document is sandboxed.
pub const CONTENT_SECURITY_POLICY: &str =
    "default-src 'none'; img-src 'self' data:; style-src 'unsafe-inline'; sandbox";

/// The cache rule of every file response.
pub const CACHE_CONTROL: &str = "private, max-age=31536000, immutable";

#[derive(Deserialize)]
pub(crate) struct DownloadQuery {
    download: Option<String>,
}

/// The cache rule of a file read through a share link: never kept (ADR 0040), so a revoked file is
/// not served again from the reader's browser. Every other safeguard of a file response applies.
pub(crate) const SHARED_CACHE_CONTROL: &str = "no-store";

/// `GET /api/threads/{threadId}/artifacts/{sha256}[?download=1]`.
pub(crate) async fn get_artifact<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path((thread, sha256)): Path<(String, String)>,
    crate::ApiQuery(query): crate::ApiQuery<DownloadQuery>,
) -> Result<Response, ApiError> {
    let download = query.download()?;
    let thread = parse_thread_id(&thread)?;
    let opened = state.app.open_artifact(&principal, thread, &sha256).await?;
    serve(opened, &sha256, download, CACHE_CONTROL, || {
        state.app.open_artifact(&principal, thread, &sha256)
    })
    .await
}

impl DownloadQuery {
    /// Whether `?download=1` was asked for; any other value is a 400.
    pub(crate) fn download(&self) -> Result<bool, ApiError> {
        match self.download.as_deref() {
            None | Some("0" | "false") => Ok(false),
            Some("1" | "true") => Ok(true),
            Some(_) => Err(Problem::bad_request("download must be 1").into()),
        }
    }
}

/// Sends a file already opened, with every header a file response always carries. `cache` is the
/// `Cache-Control` of the response: the immutable one of the owner's files, or `no-store` for a file
/// read through a link (a revoked file must not be kept by the reader's browser, ADR 0040).
/// `reopen` reads the file again, for an SVG that cannot be sent inline and goes as a download.
pub(crate) async fn serve<F, Fut>(
    (meta, stream): (ArtifactMeta, orch_ports::ByteStream),
    sha256: &str,
    download: bool,
    cache: &'static str,
    reopen: F,
) -> Result<Response, ApiError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<
            Output = Result<(ArtifactMeta, orch_ports::ByteStream), orch_app::AppError>,
        >,
{
    let inline = !download && Preview::of(&meta.media_type).is_some();

    // An SVG shown inline is read whole and cleaned; any trouble sends it as it is, as a download.
    if inline && meta.media_type == "image/svg+xml" {
        if meta.size <= MAX_SVG_INLINE_BYTES
            && let Some(cleaned) = clean_svg(stream).await
        {
            return Ok(respond(
                &meta,
                sha256,
                Disposition::Inline,
                Body::from(cleaned.clone()),
                cleaned.len() as u64,
                cache,
            ));
        }
        // Not served inline: read the file again for the attachment.
        let (meta, stream) = reopen().await?;
        return Ok(respond(
            &meta,
            sha256,
            Disposition::Attachment,
            streamed(stream),
            meta.size,
            cache,
        ));
    }

    let disposition = if inline {
        Disposition::Inline
    } else {
        Disposition::Attachment
    };
    Ok(respond(
        &meta,
        sha256,
        disposition,
        streamed(stream),
        meta.size,
        cache,
    ))
}

/// The whole of a stored SVG, cleaned; `None` when it cannot be read or cleaned.
async fn clean_svg(mut stream: orch_ports::ByteStream) -> Option<Vec<u8>> {
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(piece) = stream.next().await {
        let piece = piece.ok()?;
        if bytes.len() as u64 + piece.len() as u64 > MAX_SVG_INLINE_BYTES {
            return None;
        }
        bytes.extend_from_slice(&piece);
    }
    orch_svg_clean::clean(&bytes).ok()
}

/// The store's stream as a response body. A piece that fails ends the response early (the
/// connection is cut, so the reader cannot take a prefix for the file); nothing of the error is
/// sent.
fn streamed(stream: orch_ports::ByteStream) -> Body {
    Body::from_stream(stream.map(|piece| {
        piece.map_err(|e| {
            tracing::warn!(error = %orch_core::report(&e), "an artifact could not be read to the end");
            std::io::Error::other("the file could not be read")
        })
    }))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Disposition {
    Inline,
    Attachment,
}

/// The response for a file, with every header it always carries.
fn respond(
    meta: &ArtifactMeta,
    sha256: &str,
    disposition: Disposition,
    body: Body,
    len: u64,
    cache: &'static str,
) -> Response {
    let mut response = Response::new(body);
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    let content_type = match (disposition, meta.media_type.as_str()) {
        (Disposition::Inline, kind @ ("text/plain" | "application/json")) => {
            format!("{kind}; charset=utf-8")
        }
        (_, kind) => kind.to_owned(),
    };
    // The stored type is plain visible ASCII (the store checked it), so this holds; the fallback
    // is the safe generic type.
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        disposition_header(disposition, meta.filename.as_deref(), sha256),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{sha256}\"")) {
        headers.insert(header::ETAG, etag);
    }
    response
}

/// `inline` or `attachment` with the file's name: `filename="<ASCII fallback>"` and, when the name
/// has anything else, `filename*=UTF-8''<percent-encoded>` (RFC 6266, RFC 5987).
fn disposition_header(
    disposition: Disposition,
    filename: Option<&str>,
    sha256: &str,
) -> HeaderValue {
    let kind = match disposition {
        Disposition::Inline => "inline",
        Disposition::Attachment => "attachment",
    };
    let name = filename
        .map(clean_name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| format!("artifact-{}", sha256.get(..8).unwrap_or("file")));
    let fallback: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                if matches!(c, '"' | '\\' | '%' | ';') {
                    '_'
                } else {
                    c
                }
            } else {
                '_'
            }
        })
        .collect();
    let mut value = format!("{kind}; filename=\"{fallback}\"");
    if fallback != name {
        value.push_str("; filename*=UTF-8''");
        value.push_str(&percent_encode(&name));
    }
    HeaderValue::from_str(&value).unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

/// A file name for a header: one name (no path), no control or direction character, at most 255
/// bytes. (The worker cleaned it when it kept the file; the store is not trusted for a header.)
fn clean_name(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let mut name: String = last
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200e}' | '\u{200f}')
        })
        .collect();
    name = name.trim_matches([' ', '.']).to_owned();
    while name.len() > 255 {
        name.pop();
    }
    name
}

/// RFC 5987 `attr-char`: everything but letters, digits and `!#$&+-.^_`|~` is `%XX`.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn text(d: Disposition, name: Option<&str>) -> String {
        disposition_header(d, name, &"ab".repeat(32))
            .to_str()
            .unwrap()
            .to_owned()
    }

    #[test]
    fn a_plain_name_is_quoted_and_an_unusual_one_has_both_forms() {
        assert_eq!(
            text(Disposition::Attachment, Some("chart.png")),
            "attachment; filename=\"chart.png\""
        );
        assert_eq!(
            text(Disposition::Inline, Some("chart.png")),
            "inline; filename=\"chart.png\""
        );
        assert_eq!(
            text(Disposition::Attachment, Some("Übersicht €.pdf")),
            "attachment; filename=\"_bersicht _.pdf\"; filename*=UTF-8''%C3%9Cbersicht%20%E2%82%AC.pdf"
        );
    }

    #[test]
    fn nothing_can_end_the_header_or_change_the_name() {
        for hostile in [
            "a\"; filename=\"b",
            "a\r\nSet-Cookie: x=1",
            "../../etc/passwd",
            "a\\b\\c.txt",
            "evil\u{202e}gnp.exe",
            "a;b%00c",
        ] {
            let value = text(Disposition::Attachment, Some(hostile));
            assert!(!value.contains('\r') && !value.contains('\n'), "{value}");
            // the quoted fallback is one token: its quote opens and closes it, and nothing inside
            // can end it or start another parameter
            assert_eq!(value.matches('"').count(), 2, "{value}");
            assert!(
                value.starts_with("attachment; filename=\"") && value.split("; ").count() <= 3,
                "{value}"
            );
            assert!(!value.contains(".."), "{value}");
            let fallback = value.split('"').nth(1).unwrap();
            assert!(
                !fallback.contains(['"', '\\', '%', ';', '/']) && fallback.is_ascii(),
                "{value}"
            );
        }
        assert_eq!(
            text(Disposition::Attachment, Some("../../etc/passwd")),
            "attachment; filename=\"passwd\""
        );
    }

    #[test]
    fn without_a_usable_name_the_file_is_named_by_its_hash() {
        for name in [None, Some(""), Some("..."), Some("dir/"), Some("\u{202e}")] {
            assert_eq!(
                text(Disposition::Attachment, name),
                "attachment; filename=\"artifact-abababab\"",
                "{name:?}"
            );
        }
    }
}
