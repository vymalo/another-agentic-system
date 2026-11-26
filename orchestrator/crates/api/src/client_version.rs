//! The minimum application version a deployment serves (`server.minClientVersion`).
//!
//! A client states its version in the request header [`HEADER`] (`X-Client-Version`), dotted
//! decimal numbers such as `1.4.0`. When the deployment sets a minimum, a request of the resource
//! API whose client states a lower version, an unreadable one or none is refused with the problem
//! [`TYPE`] (status 426, `code: client_update_required`), which the application shows as "update
//! the application". With no minimum, the header is never read.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::problem::Problem;

/// The request header a client states its version in.
pub const HEADER: &str = "x-client-version";

/// The `type` of the problem a client below the minimum gets. Stable: a client matches on it.
pub const TYPE: &str = "urn:vymalo:problem:client-update-required";

/// The `code` of that problem.
pub const CODE: &str = "client_update_required";

pub use orch_core::client_version::{ClientVersion, NotAVersion};

/// The problem a client below the minimum gets.
fn update_required(min: &ClientVersion) -> Problem {
    Problem::new(
        StatusCode::UPGRADE_REQUIRED,
        format!("update the application: this deployment needs version {min} or later"),
    )
    .with_code(CODE)
    .with_type(TYPE)
    .with_min_client_version(&min.to_string())
}

/// The layer: refuses a request whose client is below `min`.
pub(crate) async fn require_minimum(
    State(min): State<ClientVersion>,
    request: Request,
    next: Next,
) -> Response {
    let stated = request
        .headers()
        .get(HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|text| text.trim().parse::<ClientVersion>().ok());
    match stated {
        Some(version) if version >= min => next.run(request).await,
        _ => update_required(&min).into_response(),
    }
}
