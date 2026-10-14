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
//! - a file of any other type is sent as **`application/octet-stream`**, whatever type it was kept
//!   with: the web's policy allows scripts and styles of its own origin (`script-src 'self'`), and a
//!   kept `text/javascript` or `text/css` loaded by a `<script>` or `<link>` of an injected page would
//!   otherwise run there (`nosniff` blocks a script or a style of any other type);
//! - a request the browser makes **for a script, a worker, a style or an object** (`Sec-Fetch-Dest`)
//!   is refused with a 403 before the file is opened: a file is a document, an image or a download;
//! - an inline SVG is the **sanitized** one ([`orch_svg_clean`]); one that cannot be sanitized, or is
//!   too large to be, is sent as an attachment instead; a download is the original bytes;
//! - every response carries `X-Content-Type-Options: nosniff`, a `Content-Security-Policy` that
//!   allows nothing but the file's own images and inline styles and **sandboxes** it,
//!   and `Cache-Control: private, max-age=31536000, immutable` (the URL is the content's hash, and
//!   the file is the person's);
//! - the file name in `Content-Disposition` is cleaned of everything that could end the header or
//!   mislead a person (a fallback of printable ASCII, and the real name in the RFC 5987 form).
//!
//! **A redirect** (ADR 0058 A1, `artifacts.s3.downloads: redirect`): [`deliver`] is the one place the
//! three file routes decide how a file they may read is sent. With a store that can sign and the
//! deployment's say-so, it answers `302` to a presigned `GET` whose signed `response-*` members carry
//! what the headers above would have said (the same disposition and name, the same type rule, a
//! cache of the URL's own life), after every check the route always did and without reading the
//! file; the bucket's host adds the headers a redirect cannot carry. An SVG shown inline is the one
//! file never redirected: it is sanitized here. A signing that fails is a 5xx, never the bytes by
//! another road.

use std::time::Duration;

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::Response;
use futures::StreamExt as _;
use orch_app::{App, AppError, ReadableFile};
use orch_core::Preview;
use orch_ports::{ArtifactMeta, Ports, PresignedGet, Principal, ResponseOverrides};
use serde::Deserialize;

use crate::ApiState;
use crate::problem::{ApiError, Problem};
use crate::routes::parse_thread_id;

/// The most an SVG may be to be sanitized and sent inline: 2 MiB. A larger one is an attachment.
pub const MAX_SVG_INLINE_BYTES: u64 = 2 * 1024 * 1024;

/// The one preview type that is read, cleaned and sent by this process, never by the bucket.
const SVG: &str = "image/svg+xml";

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

/// The `Sec-Fetch-Dest` values a file is never sent for: what a browser would run or apply.
const REFUSED_DESTINATIONS: [&str; 6] = [
    "script",
    "worker",
    "sharedworker",
    "serviceworker",
    "style",
    "object",
];

/// A 403 for a request the browser makes to run or apply what it fetches (`Sec-Fetch-Dest`). A
/// request without the header (a client that is not a browser, or an older one) goes on: the type a
/// file is sent with is the other half of the rule.
pub(crate) fn refuse_subresource(headers: &HeaderMap) -> Result<(), ApiError> {
    let dest = headers
        .get("sec-fetch-dest")
        .and_then(|v| v.to_str().ok())
        .map(str::to_ascii_lowercase);
    match dest {
        Some(d) if REFUSED_DESTINATIONS.contains(&d.as_str()) => Err(Problem::forbidden(
            "a file is not sent to be run or applied as a script, a worker, a style or an object",
        )
        .into()),
        _ => Ok(()),
    }
}

/// `GET /api/threads/{threadId}/artifacts/{sha256}[?download=1]`.
pub(crate) async fn get_artifact<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path((thread, sha256)): Path<(String, String)>,
    headers: HeaderMap,
    crate::ApiQuery(query): crate::ApiQuery<DownloadQuery>,
) -> Result<Response, ApiError> {
    refuse_subresource(&headers)?;
    let download = query.download()?;
    let thread = parse_thread_id(&thread)?;
    let file = state
        .app
        .readable_artifact(&principal, thread, &sha256)
        .await?;
    deliver(&state.app, &file, &sha256, download, CACHE_CONTROL).await
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

/// Sends a file the caller may read ([`ReadableFile`]): by a `302` to a presigned `GET` when the
/// deployment redirects and the store can sign ([`App::redirect_ttl`]), else streamed. Every route
/// has run its checks before it calls this, and a refusal is never a redirect.
///
/// The file is not read to be redirected: the store says its type, size and name (a `HEAD`), the
/// route decides the headers ([`redirect_overrides`]) and the store signs them into the URL. An
/// SVG shown inline is the one file that is never redirected, so it is read and cleaned by [`serve`]
/// as ever. A signing that fails is the error it is (a 5xx), with no fallback to streaming: that
/// would hide a broken redirect behind the very load it exists to take off the orchestrator.
pub(crate) async fn deliver<P: Ports>(
    app: &App<P>,
    file: &ReadableFile,
    sha256: &str,
    download: bool,
    cache: &'static str,
) -> Result<Response, ApiError> {
    if let Some(ttl) = app.redirect_ttl(file) {
        let meta = app.readable_meta(file).await?;
        if let Some(overrides) = redirect_overrides(&meta, sha256, download, ttl)? {
            let signed = app.presign_download(file, &overrides, ttl).await?;
            return redirection(&signed);
        }
    }
    let opened = app.open_readable(file).await?;
    serve(opened, sha256, download, cache, || app.open_readable(file)).await
}

/// What a redirected download is signed to carry, or `None` when it is not redirected (an SVG shown
/// inline, which is sanitized here): the headers [`respond`] would have sent, as the three members
/// of the signature. An inline preview keeps its type (`text/plain` and `application/json` with a
/// charset), a download of one keeps its type too, every other type is `application/octet-stream`,
/// and the cache is the URL's own life.
///
/// An SVG is shown inline only up to [`MAX_SVG_INLINE_BYTES`], the most that is read to be cleaned:
/// a larger one is sent by [`serve`] as an attachment, so it is redirected as one, with the headers
/// [`serve`] would have sent, and its bytes are never read here.
///
/// # Errors
/// An internal error when the headers built cannot be signed (a header cannot carry them): a bug,
/// never a reason to send the file by another road.
fn redirect_overrides(
    meta: &ArtifactMeta,
    sha256: &str,
    download: bool,
    ttl: Duration,
) -> Result<Option<ResponseOverrides>, ApiError> {
    let inline = !download && Preview::of(&meta.media_type).is_some();
    let svg = meta.media_type == SVG;
    if inline && svg && meta.size <= MAX_SVG_INLINE_BYTES {
        return Ok(None);
    }
    let disposition = if inline && !svg {
        Disposition::Inline
    } else {
        Disposition::Attachment
    };
    checked(ResponseOverrides {
        content_disposition: Some(disposition_text(
            disposition,
            meta.filename.as_deref(),
            sha256,
        )),
        content_type: Some(content_type(disposition, &meta.media_type)),
        cache_control: Some(format!("private, max-age={}", ttl.as_secs())),
    })
    .map(Some)
}

/// The overrides, if every value is text a header can carry. They are built from a cleaned name and
/// fixed words, so a refusal is a bug of ours: a 500, never the bytes sent by another road.
fn checked(overrides: ResponseOverrides) -> Result<ResponseOverrides, ApiError> {
    overrides.check().map_err(|e| {
        ApiError::from(AppError::internal(format!(
            "a download's headers cannot be signed: {e}"
        )))
    })?;
    Ok(overrides)
}

/// The `302`: the presigned URL, and a response that is kept by nobody and sends no referrer on. It has no
/// body, and none of the headers of a file (those are the bucket's answer to the `GET` it points at).
fn redirection(signed: &PresignedGet) -> Result<Response, ApiError> {
    let location = HeaderValue::from_str(&signed.url)
        .map_err(|_| ApiError::from(AppError::internal("a presigned URL is not a header value")))?;
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::FOUND;
    let headers = response.headers_mut();
    headers.insert(header::LOCATION, location);
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    // A redirect's `Referrer-Policy` governs the request it sends the browser to (Fetch, HTTP-redirect
    // fetch; Referrer Policy 8.2): the page that asked may be `/s/<token>`, a capability that has no
    // business at the files host.
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    Ok(response)
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
    if inline && meta.media_type == SVG {
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
    let content_type = content_type(disposition, &meta.media_type);
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

/// The `Content-Type` a file is sent with, whether the bytes or the bucket send it: a preview type
/// keeps its own (a shown `text/plain` or `application/json` says its charset), and every other type
/// is `application/octet-stream` whatever it was kept with.
fn content_type(disposition: Disposition, media_type: &str) -> String {
    match (disposition, media_type) {
        (Disposition::Inline, kind @ ("text/plain" | "application/json")) => {
            format!("{kind}; charset=utf-8")
        }
        // a preview type is neither a script nor a style, inline or downloaded
        (_, kind) if Preview::of(kind).is_some() => kind.to_owned(),
        // anything else might be one (`text/javascript`, `text/css`): bytes to save, nothing more
        _ => "application/octet-stream".to_owned(),
    }
}

/// `inline` or `attachment` with the file's name: `filename="<ASCII fallback>"` and, when the name
/// has anything else, `filename*=UTF-8''<percent-encoded>` (RFC 6266, RFC 5987). The header of a
/// streamed file ([`disposition_header`]) and the signed `response-content-disposition` of a
/// redirected one say this same text.
fn disposition_text(disposition: Disposition, filename: Option<&str>, sha256: &str) -> String {
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
    value
}

/// [`disposition_text`] as a header value. Its text is printable ASCII by construction.
fn disposition_header(
    disposition: Disposition,
    filename: Option<&str>,
    sha256: &str,
) -> HeaderValue {
    HeaderValue::from_str(&disposition_text(disposition, filename, sha256))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
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

    fn meta(media_type: &str, filename: Option<&str>) -> ArtifactMeta {
        ArtifactMeta::of(media_type, filename.map(str::to_owned), b"x")
    }

    fn overrides(media_type: &str, filename: Option<&str>, download: bool) -> ResponseOverrides {
        redirect_overrides(
            &meta(media_type, filename),
            &"ab".repeat(32),
            download,
            Duration::from_secs(300),
        )
        .unwrap()
        .unwrap()
    }

    #[test]
    fn a_redirect_signs_what_the_response_would_have_said() {
        // inline for a preview type, with its own type (and a charset for the two texts)
        for (media_type, shown) in [
            ("image/png", "image/png"),
            ("image/jpeg", "image/jpeg"),
            ("text/plain", "text/plain; charset=utf-8"),
            ("application/json", "application/json; charset=utf-8"),
        ] {
            let o = overrides(media_type, Some("a b.x"), false);
            assert_eq!(
                o.content_disposition.as_deref(),
                Some("inline; filename=\"a b.x\"")
            );
            assert_eq!(o.content_type.as_deref(), Some(shown));
            assert_eq!(o.cache_control.as_deref(), Some("private, max-age=300"));
            // the same preview, as a download: an attachment, and its type with no charset
            let o = overrides(media_type, Some("a b.x"), true);
            assert_eq!(
                o.content_disposition.as_deref(),
                Some("attachment; filename=\"a b.x\"")
            );
            assert_eq!(o.content_type.as_deref(), Some(media_type));
        }
        // any other type is an attachment of octet-stream, whatever it was kept with
        for media_type in [
            "text/javascript",
            "text/css",
            "text/html",
            "application/pdf",
        ] {
            for download in [false, true] {
                let o = overrides(media_type, None, download);
                assert_eq!(
                    o.content_disposition.as_deref(),
                    Some("attachment; filename=\"artifact-abababab\"")
                );
                assert_eq!(o.content_type.as_deref(), Some("application/octet-stream"));
            }
        }
        // the life of the URL is the cache's
        let o = redirect_overrides(
            &meta("application/zip", None),
            &"ab".repeat(32),
            true,
            Duration::from_secs(60),
        )
        .unwrap()
        .unwrap();
        assert_eq!(o.cache_control.as_deref(), Some("private, max-age=60"));
        // the name has both forms, as a streamed file's does, and the overrides can be signed
        let o = overrides("application/pdf", Some("Übersicht €.pdf"), true);
        assert_eq!(
            o.content_disposition.as_deref(),
            Some(
                "attachment; filename=\"_bersicht _.pdf\"; filename*=UTF-8''%C3%9Cbersicht%20%E2%82%AC.pdf"
            )
        );
        o.check().unwrap();
        // even a name at the longest a store keeps, in its widest form
        let o = overrides("application/zip", Some(&"é".repeat(127)), true);
        o.check().unwrap();
    }

    #[test]
    fn an_svg_shown_inline_is_the_one_file_that_is_not_redirected() {
        let svg = meta("image/svg+xml", Some("mark.svg"));
        let sha = "ab".repeat(32);
        let ttl = Duration::from_secs(300);
        assert!(
            redirect_overrides(&svg, &sha, false, ttl)
                .unwrap()
                .is_none()
        );
        let o = redirect_overrides(&svg, &sha, true, ttl).unwrap().unwrap();
        assert_eq!(
            o.content_disposition.as_deref(),
            Some("attachment; filename=\"mark.svg\"")
        );
        assert_eq!(o.content_type.as_deref(), Some("image/svg+xml"));
    }

    #[test]
    fn an_svg_too_large_to_be_cleaned_is_not_shown_inline_so_it_is_redirected_as_an_attachment() {
        let sha = "ab".repeat(32);
        let ttl = Duration::from_secs(300);
        let sized = |size| ArtifactMeta {
            size,
            ..meta("image/svg+xml", Some("big.svg"))
        };
        // at the limit it is cleaned and shown here; past it `serve` would send it as an attachment
        assert!(
            redirect_overrides(&sized(MAX_SVG_INLINE_BYTES), &sha, false, ttl)
                .unwrap()
                .is_none()
        );
        // past the limit it is not shown inline, so it is redirected
        let o = redirect_overrides(&sized(MAX_SVG_INLINE_BYTES + 1), &sha, false, ttl)
            .unwrap()
            .unwrap();
        assert_eq!(
            o.content_disposition.as_deref(),
            Some("attachment; filename=\"big.svg\"")
        );
        assert_eq!(o.content_type.as_deref(), Some("image/svg+xml"));
        // the size of any other file changes nothing
        let big_png = ArtifactMeta {
            size: MAX_SVG_INLINE_BYTES * 10,
            ..meta("image/png", Some("big.png"))
        };
        let o = redirect_overrides(&big_png, &sha, false, ttl)
            .unwrap()
            .unwrap();
        assert_eq!(
            o.content_disposition.as_deref(),
            Some("inline; filename=\"big.png\"")
        );
    }

    #[test]
    fn the_name_a_redirect_signs_is_the_text_of_the_header_a_stream_sends() {
        let sha = "ab".repeat(32);
        for name in [
            None,
            Some("chart.png"),
            Some("Übersicht €.pdf"),
            Some("a\"; filename=\"b"),
            Some("../../etc/passwd"),
            Some("evil\u{202e}gnp.exe"),
            Some(&"é".repeat(127)),
        ] {
            for disposition in [Disposition::Inline, Disposition::Attachment] {
                assert_eq!(
                    disposition_text(disposition, name, &sha),
                    disposition_header(disposition, name, &sha)
                        .to_str()
                        .unwrap(),
                    "{name:?}"
                );
            }
        }
    }

    #[test]
    fn an_override_a_header_cannot_carry_is_a_500_and_never_the_file_by_another_road() {
        let ok = ResponseOverrides {
            content_disposition: Some("attachment; filename=\"a.zip\"".to_owned()),
            content_type: Some("application/octet-stream".to_owned()),
            cache_control: Some("private, max-age=300".to_owned()),
        };
        assert!(checked(ok.clone()).is_ok());
        for bad in [
            ResponseOverrides {
                content_disposition: Some("attachment\r\nx-evil: 1".to_owned()),
                ..ok.clone()
            },
            ResponseOverrides {
                content_type: Some(String::new()),
                ..ok.clone()
            },
            ResponseOverrides {
                cache_control: Some("x".repeat(3000)),
                ..ok
            },
        ] {
            match checked(bad) {
                Err(ApiError::App(AppError::Internal { .. })) => {}
                Err(_) => panic!("expected an internal error"),
                Ok(_) => panic!("an override a header cannot carry was accepted"),
            }
        }
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
