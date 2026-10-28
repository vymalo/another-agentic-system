//! `GET /api/me/usage?month=YYYY-MM`: the caller's own month of the usage ledger, priced (ADR 0065,
//! decision 9).
//!
//! The route has **no owner parameter** and none can be added by accident: the query struct refuses
//! any key but `month`, and the handler reads the person from the authenticated principal alone, so
//! another person's usage is not a thing the route can be asked for (as `GET /api/threads` lists the
//! caller's threads and nobody else's, ADR 0039). Like `GET /api/me` it asks only for an identity: a
//! person whose roles grant nothing reads their own usage too.
//!
//! The pricing is `orch_core::price_month` (integers, no floating point); this module only writes the
//! answer: every amount is a **string with exactly six decimals**, never a JSON number.

use axum::Extension;
use axum::Json;
use axum::extract::State;
use axum::http::{HeaderValue, header};
use axum::response::{IntoResponse, Response};
use orch_core::{
    LinePrice, ModelKey, ModelTokens, PricedMonth, SandboxShape, UnmeasuredSandbox, UsageMonth,
};
use orch_ports::{Ports, Principal};
use serde::{Deserialize, Serialize};

use crate::ApiState;
use crate::extract::ApiQuery;
use crate::problem::{ApiError, Problem};

/// The query of the route: `month` and nothing else (a key it does not know is a 400, `owner`
/// included).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UsageQuery {
    month: Option<String>,
}

/// `GET /api/me/usage`.
pub(crate) async fn my_usage<P: Ports>(
    State(state): State<ApiState<P>>,
    Extension(principal): Extension<Principal>,
    ApiQuery(query): ApiQuery<UsageQuery>,
) -> Result<Response, ApiError> {
    let month = query
        .month
        .map(|text| text.parse::<UsageMonth>())
        .transpose()
        .map_err(|_| Problem::bad_request("month must be written YYYY-MM, such as 2026-10"))?;
    let priced = state.app.usage_month(&principal.user, month).await?;
    let mut response = Json(MyUsage::of(&priced)).into_response();
    // What a person used changes with every call of their agents: never kept.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

/// Contract `MyUsage`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MyUsage<'a> {
    month: String,
    currency: Option<String>,
    prices: Option<String>,
    tasks: u64,
    models: Vec<ModelLine<'a>>,
    sandbox: Vec<SandboxLine<'a>>,
    unmeasured: Vec<UnmeasuredLine>,
    total: Total<'a>,
}

/// Contract `UsageModelLine`: the counts as `ModelTokens` writes them (the provider and the parts
/// absent when unknown), and the price.
#[derive(Serialize)]
struct ModelLine<'a> {
    #[serde(flatten)]
    usage: &'a ModelTokens,
    price: Option<String>,
}

/// Contract `UsageSandboxLine`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SandboxLine<'a> {
    environment: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cpu_millis: &'a Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    memory_bytes: &'a Option<u64>,
    commands: u64,
    milliseconds: u64,
    price: Option<String>,
}

/// Contract `UsageUnmeasured`.
#[derive(Serialize)]
struct UnmeasuredLine {
    environment: &'static str,
    tasks: u64,
}

/// Contract `UsageTotal`.
#[derive(Serialize)]
struct Total<'a> {
    amount: String,
    currency: Option<String>,
    complete: bool,
    unpriced: Unpriced<'a>,
}

/// Contract `UsageUnpriced`.
#[derive(Serialize)]
struct Unpriced<'a> {
    models: Vec<UnpricedModel<'a>>,
    sandbox: u64,
    unmeasured: u64,
}

#[derive(Serialize)]
struct UnpricedModel<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    provider: &'a Option<String>,
    model: &'a str,
}

impl<'a> MyUsage<'a> {
    fn of(priced: &'a PricedMonth) -> Self {
        let price = |p: LinePrice| p.amount().map(|amount| amount.to_string());
        MyUsage {
            month: priced.month.to_string(),
            currency: priced.currency.map(|c| c.to_string()),
            prices: priced.prices.as_ref().map(ToString::to_string),
            tasks: priced.tasks,
            models: priced
                .models
                .iter()
                .map(|line| ModelLine {
                    usage: &line.usage,
                    price: price(line.price),
                })
                .collect(),
            sandbox: priced
                .sandbox
                .iter()
                .map(|line| sandbox_line(&line.shape, price(line.price)))
                .collect(),
            unmeasured: priced.unmeasured.iter().map(unmeasured_line).collect(),
            total: Total {
                amount: priced.total.amount.to_string(),
                currency: priced.total.currency.map(|c| c.to_string()),
                complete: priced.total.complete,
                unpriced: Unpriced {
                    models: priced
                        .total
                        .unpriced
                        .models
                        .iter()
                        .map(|ModelKey { provider, model }| UnpricedModel { provider, model })
                        .collect(),
                    sandbox: priced.total.unpriced.sandbox,
                    unmeasured: priced.total.unpriced.unmeasured,
                },
            },
        }
    }
}

fn sandbox_line(shape: &SandboxShape, price: Option<String>) -> SandboxLine<'_> {
    SandboxLine {
        environment: shape.environment.as_str(),
        cpu_millis: &shape.cpu_millis,
        memory_bytes: &shape.memory_bytes,
        commands: shape.commands,
        milliseconds: shape.milliseconds,
        price,
    }
}

fn unmeasured_line(line: &UnmeasuredSandbox) -> UnmeasuredLine {
    UnmeasuredLine {
        environment: line.environment.as_str(),
        tasks: line.tasks,
    }
}
