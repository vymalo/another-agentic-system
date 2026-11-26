use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::problem::Problem;

/// `Json<T>` whose rejections are 400 problems (bad JSON, missing fields, wrong content type).
pub struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = Problem;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let typed = req.headers().contains_key(axum::http::header::CONTENT_TYPE);
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(json_problem(&rejection, typed)),
        }
    }
}

/// A body that names another media type is 415; one that names none stays a 400.
fn json_problem(rejection: &JsonRejection, typed: bool) -> Problem {
    if typed && matches!(rejection, JsonRejection::MissingJsonContentType(_)) {
        return Problem::new(
            axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "the body must be application/json or application/cbor",
        )
        .with_code("unsupported_media_type");
    }
    Problem::bad_request(rejection.body_text())
}

/// `Query<T>` whose rejections are 400 problems.
pub struct ApiQuery<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for ApiQuery<T> {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(ApiQuery(value)),
            Err(rejection) => Err(query_problem(&rejection)),
        }
    }
}

fn query_problem(rejection: &QueryRejection) -> Problem {
    Problem::bad_request(rejection.body_text())
}
