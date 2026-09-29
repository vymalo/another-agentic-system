use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use orch_app::AppError;
use orch_ports::StoreError;
use serde::Serialize;

/// RFC 9457 problem details (`application/problem+json`), the contract's `Problem` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Problem {
    /// Always `about:blank` (the status says it all).
    #[serde(rename = "type")]
    pub r#type: String,
    /// Short summary.
    pub title: String,
    /// HTTP status.
    pub status: u16,
    /// Human-readable explanation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Problem {
    /// A problem with the canonical title of `status`.
    pub fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        Problem {
            r#type: "about:blank".to_owned(),
            title: status.canonical_reason().unwrap_or("Error").to_owned(),
            status: status.as_u16(),
            detail: Some(detail.into()),
        }
    }

    /// 400.
    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, detail)
    }

    /// 401.
    pub fn unauthorized(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, detail)
    }

    /// 404.
    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, detail)
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = serde_json::to_vec(&self).unwrap_or_default();
        let mut response = (status, body).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        response
    }
}

/// Adapter so handlers can `?` an [`AppError`].
pub(crate) struct ApiError(pub AppError);

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        ApiError(e)
    }
}

impl From<Problem> for ApiError {
    fn from(p: Problem) -> Self {
        ApiError(AppError::Invalid(p.detail.unwrap_or(p.title)))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self.0 {
            AppError::NotFound => Problem::not_found("no such thread"),
            AppError::Invalid(m) => Problem::bad_request(m),
            AppError::Finished => {
                Problem::new(StatusCode::CONFLICT, "Thread is finished; start a new one")
            }
            AppError::Transition(e) => Problem::new(StatusCode::CONFLICT, e.to_string()),
            AppError::Store(StoreError::Unavailable(e)) => {
                tracing::error!(error = %e, "store unavailable");
                Problem::new(StatusCode::SERVICE_UNAVAILABLE, "storage is unavailable")
            }
            e @ (AppError::Store(_) | AppError::Agent(_)) => {
                tracing::error!(error = %e, "request failed");
                Problem::new(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        }
        .into_response()
    }
}
