use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use orch_core::UserId;

use crate::problem::Problem;

/// Header set by oauth2-proxy after login.
pub const IDENTITY_HEADER: &str = "x-auth-request-email";

/// Identity configuration.
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    /// Identity injected when the header is absent (`AUTH_DEV_USER`). `None` means fail closed.
    pub dev_user: Option<UserId>,
}

/// Rejects requests without an identity (401) and stores the [`UserId`] in the request
/// extensions. A present-but-malformed header is refused even when a dev user is configured.
pub(crate) async fn require_identity(
    State(cfg): State<Arc<AuthConfig>>,
    mut req: Request,
    next: Next,
) -> Response {
    let user = match req.headers().get(IDENTITY_HEADER) {
        Some(value) => {
            let email = value
                .to_str()
                .ok()
                .map(str::trim)
                .filter(|e| !e.is_empty() && e.contains('@'));
            match email {
                Some(email) => UserId::new(email),
                None => {
                    return Problem::unauthorized("invalid X-Auth-Request-Email").into_response();
                }
            }
        }
        None => match &cfg.dev_user {
            Some(user) => user.clone(),
            None => {
                return Problem::unauthorized("missing X-Auth-Request-Email").into_response();
            }
        },
    };
    req.extensions_mut().insert(user);
    next.run(req).await
}
