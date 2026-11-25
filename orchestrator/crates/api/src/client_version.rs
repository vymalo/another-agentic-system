//! The minimum application version a deployment serves (`server.minClientVersion`).
//!
//! A client states its version in the request header [`HEADER`] (`X-Client-Version`), dotted
//! decimal numbers such as `1.4.0`. When the deployment sets a minimum, a request of the resource
//! API whose client states a lower version, an unreadable one or none is refused with the problem
//! [`TYPE`] (status 426, `code: client_update_required`), which the application shows as "update
//! the application". With no minimum, the header is never read.

use std::cmp::Ordering;
use std::str::FromStr;

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

/// Most components a version may have.
const MAX_PARTS: usize = 4;
/// Most digits of one component.
const MAX_DIGITS: usize = 9;

/// A version: one to four dotted decimal components. `1.4` equals `1.4.0`.
#[derive(Debug, Clone)]
pub struct ClientVersion(Vec<u32>);

/// Why a text is not a [`ClientVersion`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "not a version: one to four dotted decimal numbers of at most nine digits, such as `1.4.0`"
)]
pub struct NotAVersion;

impl FromStr for ClientVersion {
    type Err = NotAVersion;

    fn from_str(text: &str) -> Result<Self, NotAVersion> {
        let parts: Vec<&str> = text.split('.').collect();
        if parts.is_empty() || parts.len() > MAX_PARTS {
            return Err(NotAVersion);
        }
        parts
            .into_iter()
            .map(|part| {
                if part.is_empty()
                    || part.len() > MAX_DIGITS
                    || !part.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(NotAVersion);
                }
                part.parse::<u32>().map_err(|_| NotAVersion)
            })
            .collect::<Result<Vec<u32>, _>>()
            .map(ClientVersion)
    }
}

impl PartialEq for ClientVersion {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ClientVersion {}

impl PartialOrd for ClientVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ClientVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        (0..MAX_PARTS)
            .map(|i| {
                let a = self.0.get(i).copied().unwrap_or(0);
                let b = other.0.get(i).copied().unwrap_or(0);
                a.cmp(&b)
            })
            .find(|o| *o != Ordering::Equal)
            .unwrap_or(Ordering::Equal)
    }
}

impl std::fmt::Display for ClientVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text: Vec<String> = self.0.iter().map(u32::to_string).collect();
        f.write_str(&text.join("."))
    }
}

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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn v(text: &str) -> ClientVersion {
        text.parse().unwrap()
    }

    #[test]
    fn versions_compare_numerically_and_pad_with_zeros() {
        assert!(v("1.10.0") > v("1.9.9"));
        assert_eq!(v("1.4"), v("1.4.0"));
        assert!(v("2") > v("1.99.99.99"));
        assert!(v("1.4.0") < v("1.4.0.1"));
    }

    #[test]
    fn what_is_not_a_version_is_refused() {
        for bad in [
            "",
            "1.",
            ".1",
            "1..2",
            "1.2.3.4.5",
            "v1.2",
            "1.2-beta",
            "1.2.x",
            "1234567890",
            "-1",
        ] {
            assert!(bad.parse::<ClientVersion>().is_err(), "{bad:?}");
        }
    }
}
