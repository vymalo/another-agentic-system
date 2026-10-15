//! The HTTP side of the transport, with the size of every response bounded **on the wire**.
//!
//! `rmcp` 3.5.0 reads a non-streamed answer with `response.bytes().await`, an error body with
//! `response.text().await`, and bounds a streamed answer **per event** only: a server that answers
//! a plain JSON `tools/list` of 300 MiB, or streams small events for ever, makes the orchestrator
//! buffer it all (a reviewer's PoC peaked at 629 MiB of resident memory for one such answer). The
//! reqwest client cannot be made to stop: it has no limit on a response body, and a
//! `connector_layer` must hand back the connection type it was given, which a crate outside
//! reqwest cannot name, so it cannot count the bytes of a connection either.
//!
//! The one seam rmcp offers is its HTTP backend, the trait
//! [`StreamableHttpClient`](rmcp::transport::streamable_http_client::StreamableHttpClient): the
//! transport is generic over it. [`WireClient`] implements it over a `reqwest::Client` and, for the
//! client of the servers people name, reads **at most a given number of bytes of any one response**
//! (the body of a JSON answer, of an error, and the whole of an event stream) and fails the request
//! with [`BodyError::TooLarge`] past it, having read at most the bound and one chunk. The response
//! is then dropped, which closes the connection, so the server is cut off.
//!
//! The implementation follows rmcp's own for `reqwest::Client` (the statuses, the challenge errors,
//! the empty `200` of a notification, the session id) and differs in four ways: the bound; the
//! error text of an unexpected status holds the status and **not the server's body** (a stranger's
//! words do not go into an error that might be logged); every `reqwest::Error` has its URL taken
//! out (`without_url`; a person's URL can carry a secret in its path); and the `server/discover`
//! rejection of a legacy server is not rewritten, because this client never sends it (`serve` opens
//! with `initialize`).
//!
//! The deployment's client keeps rmcp's implementation of the same trait, with the URL taken out
//! of its errors and no bound (its servers are the operator's choice).

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use http::{HeaderName, HeaderValue};
use reqwest::header::{ACCEPT, CONTENT_TYPE, WWW_AUTHENTICATE};
use reqwest::{Response, StatusCode};
use rmcp::model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage};
use rmcp::transport::common::http_header::{
    EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID, JSON_MIME_TYPE,
};
use rmcp::transport::streamable_http_client::{
    AuthRequiredError, InsufficientScopeError, SseError, StreamableHttpClient, StreamableHttpError,
    StreamableHttpPostResponse,
};
use sse_stream::{Sse, SseStream};

/// What the error of a request says when the answer was over the bound: [`from_http`] in `lib.rs`
/// knows it by this text.
pub(crate) const ANSWER_TOO_LARGE: &str = "the server's answer is larger than allowed";

/// The most of an error response's body that is read: a JSON-RPC error is a few hundred bytes, and
/// the rest of the body is not used. Capped by the bound of the client if that is smaller.
const ERROR_BODY_BYTES: usize = 64 * 1024;

/// Why a response body could not be read to its end.
#[derive(Debug, thiserror::Error)]
pub(crate) enum BodyError {
    /// The body is over the bound.
    #[error("the server's answer is larger than allowed")]
    TooLarge,
    /// The connection broke. The error holds no URL.
    #[error("the answer could not be read")]
    Transport(#[source] reqwest::Error),
}

/// Whether a response of one session was cut at the bound. rmcp reports a body that failed in
/// the middle of a stream as it likes (a closed transport, a cancelled request); this says what
/// happened, so the caller can answer "larger than allowed" whatever rmcp made of it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Cut(Arc<AtomicBool>);

impl Cut {
    fn trip(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether a response of the session was over the bound.
    pub(crate) fn tripped(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// `rmcp`'s HTTP backend over a `reqwest::Client`, optionally bounding every response (see the
/// module's docs).
#[derive(Debug, Clone)]
pub(crate) struct WireClient {
    http: reqwest::Client,
    /// The most bytes of one response; `None` for rmcp's own, unbounded reading.
    max_body: Option<usize>,
    cut: Cut,
}

impl WireClient {
    pub(crate) fn new(http: reqwest::Client, max_body: Option<usize>) -> Self {
        WireClient {
            http,
            max_body,
            cut: Cut::default(),
        }
    }

    /// The flag this client's responses trip when one is over the bound.
    pub(crate) fn cut(&self) -> Cut {
        self.cut.clone()
    }
}

/// An error of the HTTP client, with the URL taken out.
fn client_error(error: reqwest::Error) -> StreamableHttpError<reqwest::Error> {
    StreamableHttpError::Client(error.without_url())
}

/// The same for an error rmcp's own implementation made.
fn stripped(error: StreamableHttpError<reqwest::Error>) -> StreamableHttpError<reqwest::Error> {
    match error {
        StreamableHttpError::Client(error) => client_error(error),
        other => other,
    }
}

/// The custom headers on the request, refusing the names the transport sets itself
/// (`MCP-Protocol-Version` is reserved but allowed: the worker adds it after the handshake).
fn with_custom_headers(
    mut request: reqwest::RequestBuilder,
    custom_headers: HashMap<HeaderName, HeaderValue>,
) -> Result<reqwest::RequestBuilder, StreamableHttpError<reqwest::Error>> {
    for (name, value) in custom_headers {
        let lower = name.as_str();
        if lower.eq_ignore_ascii_case("accept")
            || lower.eq_ignore_ascii_case(HEADER_SESSION_ID)
            || lower.eq_ignore_ascii_case(HEADER_LAST_EVENT_ID)
        {
            return Err(StreamableHttpError::ReservedHeaderConflict(
                name.to_string(),
            ));
        }
        request = request.header(name, value);
    }
    Ok(request)
}

/// The error of a 401 or a 403 that carries a challenge, which says what the server wants.
fn challenge(
    response: &Response,
) -> Result<Option<StreamableHttpError<reqwest::Error>>, StreamableHttpError<reqwest::Error>> {
    let status = response.status();
    if status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN {
        return Ok(None);
    }
    let Some(header) = response.headers().get(WWW_AUTHENTICATE) else {
        return Ok(None);
    };
    let header = header
        .to_str()
        .map_err(|_| {
            StreamableHttpError::UnexpectedServerResponse(Cow::Borrowed(
                "invalid www-authenticate header value",
            ))
        })?
        .to_owned();
    Ok(Some(if status == StatusCode::UNAUTHORIZED {
        StreamableHttpError::AuthRequired(AuthRequiredError::new(header))
    } else {
        StreamableHttpError::InsufficientScope(InsufficientScopeError::new(header, None))
    }))
}

/// `stream`, cut off at `max` bytes: the first chunk that would take the total past `max` is not
/// passed on, the stream yields [`BodyError::TooLarge`] in its place, and then it ends. At most
/// `max` bytes and one chunk are ever held.
pub(crate) fn bounded<S, E>(
    stream: S,
    max: usize,
    cut: Cut,
) -> impl futures::Stream<Item = Result<Bytes, BodyError>> + Send
where
    S: futures::Stream<Item = Result<Bytes, E>> + Send,
    E: Into<BodyError>,
{
    stream.scan((0_usize, false), move |(seen, ended), item| {
        let next = if *ended {
            None
        } else {
            match item.map_err(Into::into) {
                Ok(chunk) if seen.saturating_add(chunk.len()) > max => {
                    *ended = true;
                    cut.trip();
                    Some(Err(BodyError::TooLarge))
                }
                Ok(chunk) => {
                    *seen += chunk.len();
                    Some(Ok(chunk))
                }
                Err(error) => {
                    *ended = true;
                    Some(Err(error))
                }
            }
        };
        futures::future::ready(next)
    })
}

impl From<reqwest::Error> for BodyError {
    fn from(error: reqwest::Error) -> Self {
        BodyError::Transport(error.without_url())
    }
}

/// The body of `response`, at most `max` bytes of it.
async fn read_bounded(response: Response, max: usize, cut: &Cut) -> Result<Vec<u8>, BodyError> {
    // a body that says how long it is, and is too long, is not started
    if response
        .content_length()
        .is_some_and(|length| length > u64::try_from(max).unwrap_or(u64::MAX))
    {
        cut.trip();
        return Err(BodyError::TooLarge);
    }
    let mut stream = std::pin::pin!(bounded(response.bytes_stream(), max, cut.clone()));
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk?);
    }
    Ok(body)
}

/// What a request fails with when the answer is over the bound.
fn too_large() -> StreamableHttpError<reqwest::Error> {
    StreamableHttpError::UnexpectedServerResponse(Cow::Borrowed(ANSWER_TOO_LARGE))
}

fn body_failure(error: BodyError) -> StreamableHttpError<reqwest::Error> {
    match error {
        BodyError::TooLarge => too_large(),
        BodyError::Transport(error) => StreamableHttpError::Client(error),
    }
}

/// `response`'s event stream, bounded to `max` bytes in all.
fn event_stream(
    response: Response,
    max: usize,
    cut: Cut,
) -> BoxStream<'static, Result<Sse, SseError>> {
    SseStream::from_bytes_stream(bounded(response.bytes_stream(), max, cut)).boxed()
}

impl StreamableHttpClient for WireClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        // the transport always calls the variant below, with its own limit
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            usize::MAX,
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Self::Error>> {
        let Some(max) = self.max_body else {
            return StreamableHttpClient::post_message_with_max_sse_event_size(
                &self.http,
                uri,
                message,
                session_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
            .map_err(stripped);
        };
        let mut request = self
            .http
            .post(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "));
        if let Some(token) = auth_header {
            request = request.bearer_auth(token);
        }
        request = with_custom_headers(request, custom_headers)?;
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        let response = request.json(&message).send().await.map_err(client_error)?;
        if let Some(refusal) = challenge(&response)? {
            return Err(refusal);
        }
        let status = response.status();
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == StatusCode::NOT_FOUND && session_was_attached {
            return Err(StreamableHttpError::SessionExpired);
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned());
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let expects_no_reply = matches!(
            message,
            ClientJsonRpcMessage::Notification(_)
                | ClientJsonRpcMessage::Response(_)
                | ClientJsonRpcMessage::Error(_)
        );
        // some servers answer an empty 200 to a notification
        if status.is_success() && response.content_length() == Some(0) && expects_no_reply {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        let is_json = content_type
            .as_deref()
            .is_some_and(|ct| ct.as_bytes().starts_with(JSON_MIME_TYPE.as_bytes()));
        if !status.is_success() {
            // A JSON-RPC error rides on a 4xx or 5xx now and then; anything else is the status
            // alone: the body is a stranger's text and the rest of the system needs only the code.
            // An error body that is too long is not "an answer larger than allowed": the status
            // is what the caller is told, so this read trips no flag of the session.
            if is_json
                && let Ok(body) =
                    read_bounded(response, max.min(ERROR_BODY_BYTES), &Cut::default()).await
                && let Ok(message @ JsonRpcMessage::Error(_)) =
                    serde_json::from_slice::<ServerJsonRpcMessage>(&body)
            {
                return Ok(StreamableHttpPostResponse::Json(message, session_id));
            }
            return Err(StreamableHttpError::UnexpectedServerResponse(Cow::Owned(
                format!("HTTP {status}"),
            )));
        }
        match content_type.as_deref() {
            Some(ct) if ct.as_bytes().starts_with(EVENT_STREAM_MIME_TYPE.as_bytes()) => {
                Ok(StreamableHttpPostResponse::Sse(
                    event_stream(response, max, self.cut.clone()),
                    session_id,
                ))
            }
            Some(_) if is_json => {
                let body = read_bounded(response, max, &self.cut)
                    .await
                    .map_err(body_failure)?;
                match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                    Ok(parsed) => Ok(StreamableHttpPostResponse::Json(parsed, session_id)),
                    // a notification does not wait for a reply: an unusable body is as good as none
                    Err(_) if expects_no_reply => Ok(StreamableHttpPostResponse::Accepted),
                    Err(_) => Err(StreamableHttpError::UnexpectedServerResponse(
                        Cow::Borrowed("the answer is not a JSON-RPC message"),
                    )),
                }
            }
            _ => Err(StreamableHttpError::UnexpectedContentType(content_type)),
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Self::Error>> {
        if self.max_body.is_none() {
            return StreamableHttpClient::delete_session(
                &self.http,
                uri,
                session_id,
                auth_header,
                custom_headers,
            )
            .await
            .map_err(stripped);
        }
        let mut request = self.http.delete(uri.as_ref());
        if let Some(token) = auth_header {
            request = request.bearer_auth(token);
        }
        request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        request = with_custom_headers(request, custom_headers)?;
        let response = request.send().await.map_err(client_error)?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        // the body is not read: the response is dropped, which ends the exchange
        response.error_for_status().map_err(client_error)?;
        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        self.get_stream_with_max_sse_event_size(
            uri,
            session_id,
            last_event_id,
            auth_header,
            custom_headers,
            usize::MAX,
        )
        .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Self::Error>> {
        let Some(max) = self.max_body else {
            return StreamableHttpClient::get_stream_with_max_sse_event_size(
                &self.http,
                uri,
                session_id,
                last_event_id,
                auth_header,
                custom_headers,
                max_sse_event_size,
            )
            .await
            .map_err(stripped);
        };
        let mut request = self
            .http
            .get(uri.as_ref())
            .header(ACCEPT, [EVENT_STREAM_MIME_TYPE, JSON_MIME_TYPE].join(", "));
        if let Some(session_id) = session_id {
            request = request.header(HEADER_SESSION_ID, session_id.as_ref());
        }
        if let Some(last_event_id) = last_event_id {
            request = request.header(HEADER_LAST_EVENT_ID, last_event_id);
        }
        if let Some(token) = auth_header {
            request = request.bearer_auth(token);
        }
        request = with_custom_headers(request, custom_headers)?;
        let response = request.send().await.map_err(client_error)?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportSse);
        }
        if let Some(refusal) = challenge(&response)? {
            return Err(refusal);
        }
        let response = response.error_for_status().map_err(client_error)?;
        match response
            .headers()
            .get(CONTENT_TYPE)
            .map(reqwest::header::HeaderValue::as_bytes)
        {
            Some(ct)
                if ct.starts_with(EVENT_STREAM_MIME_TYPE.as_bytes())
                    || ct.starts_with(JSON_MIME_TYPE.as_bytes()) => {}
            Some(ct) => {
                return Err(StreamableHttpError::UnexpectedContentType(Some(
                    String::from_utf8_lossy(ct).into_owned(),
                )));
            }
            None => return Err(StreamableHttpError::UnexpectedContentType(None)),
        }
        Ok(event_stream(response, max, self.cut.clone()))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use futures::stream;

    use super::*;

    fn chunks(sizes: &[usize]) -> Vec<Result<Bytes, reqwest::Error>> {
        sizes
            .iter()
            .map(|n| Ok(Bytes::from(vec![b'x'; *n])))
            .collect()
    }

    async fn drain(
        stream: impl futures::Stream<Item = Result<Bytes, BodyError>>,
    ) -> (usize, Option<BodyError>) {
        let mut stream = std::pin::pin!(stream);
        let mut passed = 0;
        while let Some(item) = stream.next().await {
            match item {
                Ok(chunk) => passed += chunk.len(),
                Err(error) => return (passed, Some(error)),
            }
        }
        (passed, None)
    }

    #[tokio::test]
    async fn a_body_within_the_bound_passes_whole() {
        let (passed, error) = drain(bounded(
            stream::iter(chunks(&[4, 4, 2])),
            10,
            Cut::default(),
        ))
        .await;
        assert_eq!((passed, error.is_none()), (10, true));
    }

    #[tokio::test]
    async fn the_chunk_that_would_pass_the_bound_is_not_passed_on_and_the_stream_ends() {
        // 4 + 4 fit; the third chunk would make 12 > 10: it is held back, whatever follows is not read
        let cut = Cut::default();
        let mut stream = std::pin::pin!(bounded(
            stream::iter(chunks(&[4, 4, 4, 4, 4])),
            10,
            cut.clone()
        ));
        let mut passed = 0;
        let mut errors = 0;
        while let Some(item) = stream.next().await {
            match item {
                Ok(chunk) => passed += chunk.len(),
                Err(BodyError::TooLarge) => errors += 1,
                Err(other) => panic!("{other}"),
            }
        }
        assert_eq!(passed, 8, "at most the bound, in whole chunks");
        assert_eq!(errors, 1, "one error, then the end of the stream");
        assert!(cut.tripped(), "the session knows a response was cut");
    }

    #[tokio::test]
    async fn an_endless_stream_is_cut_at_the_bound() {
        // an iterator that never ends: only the bound stops it
        let endless = stream::iter(std::iter::repeat_with(|| {
            Ok::<_, reqwest::Error>(Bytes::from(vec![b'x'; 1024]))
        }));
        let (passed, error) = drain(bounded(endless, 64 * 1024, Cut::default())).await;
        assert_eq!(passed, 64 * 1024);
        assert!(matches!(error, Some(BodyError::TooLarge)));
    }
}
