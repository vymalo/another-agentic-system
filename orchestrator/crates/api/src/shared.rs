//! The routes of a reader of a shared thread (ADR 0040, section 6): the signed-in ones under
//! `/api/shared/{token}` and the public ones, outside the identity layer, under
//! `/api/public/shared/{token}`.
//!
//! What a reader may read is decided by [`orch_app::App`] (`open_shared`, `open_public`, and the
//! reader projection); these handlers only serve it. **Every way a link can fail to be a readable
//! one is the same 404, with the same body**: an unknown token, a bad MAC, a private or revoked
//! thread, a cap that was lowered, a file that is not the thread's. The answers say nothing about
//! whether a thread exists.
//!
//! The public routes ([`guard`]) take no identity (the orchestrator does not run the authenticator
//! on them, and ignores an `Authorization` header), answer `Cache-Control: no-store` and
//! `X-Robots-Tag: noindex, nofollow`, and are rate limited ([`crate::limiter`]). The signed-in ones
//! answer `no-store` too, and a shared file is never cached.

use std::sync::Arc;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_ports::{Ports, Principal};

use crate::ApiState;
use crate::artifacts::{DownloadQuery, SHARED_CACHE_CONTROL, refuse_subresource, serve};
use crate::limiter::{Limited, LinkKey, PublicAccess, PublicLimiter};
use crate::problem::{ApiError, Problem};

type ApiResult<T> = Result<T, ApiError>;

/// `X-Robots-Tag` of every response about a shared thread.
const NOINDEX: &str = "noindex, nofollow";

/// What a response about a shared thread always carries: it is never kept, and never indexed. A
/// stream keeps `no-transform` beside `no-store`, so no proxy rewrites it.
pub(crate) fn harden(mut response: Response) -> Response {
    let stream = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if stream {
            "no-store, no-transform"
        } else {
            "no-store"
        }),
    );
    headers.insert(
        HeaderName::from_static("x-robots-tag"),
        HeaderValue::from_static(NOINDEX),
    );
    response
}

/// [`harden`] as a layer of the routes a signed-in reader uses, so the refusals carry it too.
pub(crate) async fn harden_response(response: Response) -> Response {
    harden(response)
}

/// `GET /api/shared/{token}`: the shared thread as a signed-in reader is given it
/// (`SharedThread`): no owner, no parent, no tools. 401 without an identity, 403 for roles that
/// hold no `thread.read`, 404 for every link that does not work.
pub(crate) async fn get_shared<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path(token): Path<String>,
) -> ApiResult<Response> {
    let read = state.app.open_shared(&principal, &token).await?;
    Ok(harden(Json(state.app.shared_view(&read)).into_response()))
}

/// `GET /api/shared/{token}/artifacts/{sha256}[?download=1]`: a file of the shared thread, for a
/// signed-in reader with `artifact.read`, and only one the thread's log names. Sent as a file always
/// is (sanitised SVG, `nosniff`, the sandboxing CSP, an attachment unless it is a preview type), and
/// never cached.
pub(crate) async fn get_shared_artifact<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    Path((token, sha256)): Path<(String, String)>,
    headers: HeaderMap,
    crate::ApiQuery(query): crate::ApiQuery<DownloadQuery>,
) -> ApiResult<Response> {
    refuse_subresource(&headers)?;
    let download = query.download()?;
    let opened = state
        .app
        .open_shared_artifact(&principal, &token, &sha256)
        .await?;
    let response = serve(opened, &sha256, download, SHARED_CACHE_CONTROL, || {
        state.app.open_shared_artifact(&principal, &token, &sha256)
    })
    .await?;
    Ok(harden(response))
}

/// `GET /api/public/shared/{token}`: the shared thread as anybody is given it, for a thread served
/// as `public` only. No identity is asked for. 404 for every link that does not work, an `internal`
/// one included.
pub(crate) async fn get_public_shared<P: Ports>(
    State(state): State<ApiState<P>>,
    Path(token): Path<String>,
) -> ApiResult<Response> {
    let read = state.app.open_public(&token).await?;
    Ok(Json(state.app.shared_view(&read)).into_response())
}

/// `GET /api/public/shared/{token}/artifacts/{sha256}[?download=1]`: a file of a public thread,
/// only when `sharing.public.files` says so, and only one the thread's log names.
pub(crate) async fn get_public_shared_artifact<P: Ports>(
    State(state): State<ApiState<P>>,
    Path((token, sha256)): Path<(String, String)>,
    headers: HeaderMap,
    crate::ApiQuery(query): crate::ApiQuery<DownloadQuery>,
) -> ApiResult<Response> {
    refuse_subresource(&headers)?;
    let download = query.download()?;
    let opened = state.app.open_public_artifact(&token, &sha256).await?;
    serve(opened, &sha256, download, SHARED_CACHE_CONTROL, || {
        state.app.open_public_artifact(&token, &sha256)
    })
    .await
}

/// The token segment of a public path, for the limiter's per-link key: what follows `/shared/`.
fn token_of(path: &str) -> &str {
    path.split_once("/shared/")
        .map_or("", |(_, rest)| rest.split('/').next().unwrap_or(""))
}

/// The answer to a request the limiter holds back: 429 with `Retry-After`.
fn too_many(limited: Limited) -> Response {
    let mut response = Problem::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too many requests: try again shortly",
    )
    .into_response();
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from(limited.retry_after_secs),
    );
    response
}

/// The answer to a request for a public stream when the link's streams, or all links' together, are
/// taken: 429 with `Retry-After` and `code: too_many_streams`. For a streaming handler, which takes its
/// [`StreamPermit`](crate::StreamPermit) from the request's [`PublicAccess`] after the link is known to work.
pub fn too_many_streams(limited: Limited) -> Response {
    let mut response = Problem::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too many open streams: try again shortly",
    )
    .with_code("too_many_streams")
    .into_response();
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from(limited.retry_after_secs),
    );
    response
}

/// The layer of everything under the public routes: the rate limit, the headers, and nothing that
/// reads a credential. With no limiter (the composition did not build one) every request is the
/// uniform 404: public sharing fails closed.
pub(crate) async fn guard(
    State(limiter): State<Option<Arc<PublicLimiter>>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(limiter) = limiter else {
        return harden(Problem::not_found("no such thread").into_response());
    };
    let link = LinkKey::of(token_of(request.uri().path()));
    if let Err(limited) = limiter.admit(link) {
        return harden(too_many(limited));
    }
    request
        .extensions_mut()
        .insert(PublicAccess::new(Arc::clone(&limiter), link));
    // The route that says where a browser signs in is asked once by every page of a deployment that
    // does not have it, and its 404 is no guess at a link: it does not charge the shared bucket.
    let guess = request.uri().path() != crate::public_auth::PATH;
    let response = next.run(request).await;
    if guess && response.status() == StatusCode::NOT_FOUND {
        // A guess has no link of its own to be charged to: the shared bucket pays for it.
        limiter.failed();
    }
    harden(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_the_segment_after_shared() {
        assert_eq!(token_of("/api/public/shared/ABC"), "ABC");
        assert_eq!(token_of("/api/public/shared/ABC/artifacts/ff"), "ABC");
        assert_eq!(token_of("/agui/public/shared/ABC/connect"), "ABC");
        assert_eq!(token_of("/api/public/shared/"), "");
        assert_eq!(token_of("/api/other"), "");
    }
}
