use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::problem::Problem;

/// `Json<T>` whose rejections are 400 problems (bad JSON, missing fields, wrong content type).
pub(crate) struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = Problem;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(json_problem(&rejection)),
        }
    }
}

fn json_problem(rejection: &JsonRejection) -> Problem {
    Problem::bad_request(rejection.body_text())
}

/// `Query<T>` whose rejections are 400 problems.
pub(crate) struct ApiQuery<T>(pub T);

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
