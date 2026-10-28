//! Pricing a month of usage (ADR 0065, decisions 5 to 7).
//!
//! The ledger keeps **counts, not money** ([`UsageMonthTotals`]); the price of a month is put on
//! them when somebody asks, with the table the deployment configured ([`PriceTable`]), and the
//! answer says which table that was ([`PriceTableId`]). This module is pure: no async, no I/O, no
//! floating point. Money is an integer number of **millionths of the currency's unit**
//! ([`Micros`]), and every price is a whole number of them.
//!
//! * **A model line** costs `(input - cached) * input_price + cached * cached_price + output *
//!   output_price`, the prices per million tokens, divided by 1 000 000 **once**, rounded half up.
//!   Cached tokens are a part of the input (capped at it, so an agent that says more cached than
//!   input cannot make a count negative), cache-write tokens are a part of the input too and are
//!   priced as input, and reasoning tokens are a part of the output and priced as output: neither
//!   is a charge of its own. A line is matched to the table entry with the same provider (none
//!   matches none) and the same model, exactly.
//! * **A sandbox line** (a shape) costs `hours * (cores * cpu_core_hour + gib * gib_hour)` with
//!   `hours = milliseconds / 3 600 000`, `cores = cpu_millis / 1000` and `gib = memory_bytes /
//!   2^30`, computed as one exact fraction and rounded half up once. A shape that lacks a CPU or a
//!   memory request is **unpriced**: not reserving is not the same as reserving nothing.
//! * **Arithmetic is checked** (`u128`). A line whose arithmetic overflows is unpriced, with
//!   [`Unpriced::Overflow`]; it is never a panic and never a wrong number.
//! * **Nothing is free by omission.** A model without a price, a shape without a price, a task
//!   whose sandbox was not measured (`local`, `devcontainer`) and every line when no table is
//!   configured are said so, and the total is then not [`complete`](PricedTotal::complete). The
//!   amount of a total is the sum of the lines that *are* priced.

use std::fmt;

use sha2::{Digest, Sha256};

use crate::usage::{ModelTokens, TokenCounts};
use crate::usage_record::{SandboxShape, UnmeasuredSandbox, UsageMonth, UsageMonthTotals};

/// Millionths of a currency unit in one unit.
pub const MICROS_PER_UNIT: u128 = 1_000_000;

/// The dearest price a table may hold, per million tokens or per hour: 1 000 000 currency units,
/// in [`Micros`]. A price is a number somebody typed; this keeps a slip of the finger from making
/// every line overflow, and keeps the arithmetic far from the edge of `u128`.
pub const MAX_PRICE_MICROS: u64 = 1_000_000 * 1_000_000;

/// Most entries of a table's models. The configuration's bound.
pub const MAX_PRICED_MODELS: usize = 256;

const TOKENS_PER_PRICE: u128 = 1_000_000;
const MS_PER_HOUR: u128 = 3_600_000;
const MILLICORES_PER_CORE: u128 = 1_000;
const BYTES_PER_GIB: u128 = 1 << 30;

/// An amount of money in millionths of the currency's unit: `1_234` is `0.001234`. Never negative.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Micros(u128);

impl Micros {
    /// No money.
    pub const ZERO: Micros = Micros(0);

    /// `n` millionths of a unit.
    pub const fn new(n: u128) -> Micros {
        Micros(n)
    }

    /// The number of millionths.
    pub const fn get(self) -> u128 {
        self.0
    }
}

impl fmt::Display for Micros {
    /// The amount as a decimal with exactly six fraction digits: `0.001234`, `12.000000`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}.{:06}",
            self.0 / MICROS_PER_UNIT,
            self.0 % MICROS_PER_UNIT
        )
    }
}

/// An ISO 4217 shaped currency code: exactly three upper-case ASCII letters. Whether the code
/// names a currency is not checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Currency([u8; 3]);

/// Text that is not a currency code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a currency code: expected exactly three upper-case ASCII letters")]
pub struct CurrencyError;

impl Currency {
    /// The currency `code`: exactly three characters, each `A` to `Z`.
    ///
    /// # Errors
    ///
    /// [`CurrencyError`] for anything else.
    pub fn new(code: &str) -> Result<Currency, CurrencyError> {
        match code.as_bytes() {
            &[a, b, c] if [a, b, c].iter().all(u8::is_ascii_uppercase) => Ok(Currency([a, b, c])),
            _ => Err(CurrencyError),
        }
    }

    /// The code, such as `EUR`.
    pub fn as_str(&self) -> &str {
        // three ASCII letters, checked by the constructor
        std::str::from_utf8(&self.0).unwrap_or_default()
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The price of a model: per million tokens, in [`Micros`]. All three are required: a model with
/// no cache price says its input price twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelPrice {
    /// The provider as the agent reports it; `None` matches the lines that report none.
    pub provider: Option<String>,
    /// The model, matched exactly.
    pub model: String,
    /// Per million input tokens that were not read from a cache (cache writes included).
    pub input: Micros,
    /// Per million input tokens read from a provider cache.
    pub cached_input: Micros,
    /// Per million output tokens (reasoning tokens included).
    pub output: Micros,
}

/// The price of a sandbox: per hour of what its pods reserved, in [`Micros`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SandboxPrice {
    /// Per CPU core-hour of the request.
    pub cpu_core_hour: Micros,
    /// Per GiB-hour of the memory request.
    pub gib_hour: Micros,
}

/// Why a table was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PriceTableError {
    /// Two entries name the same provider and model.
    #[error("two entries price the same provider and model")]
    DuplicateModel {
        /// The provider of both.
        provider: Option<String>,
        /// The model of both.
        model: String,
    },
    /// A price is above [`MAX_PRICE_MICROS`].
    #[error("a price is above the bound of 1000000 currency units")]
    PriceTooHigh,
    /// More than [`MAX_PRICED_MODELS`] entries.
    #[error("more than 256 priced models")]
    TooManyModels,
}

/// A short, stable name of a [`PriceTable`]: `sha256:` and the first 16 hex digits of the SHA-256
/// of its canonical form. The same parsed table has the same id in every process, whatever the
/// order of its entries or how its prices were written (`0.40` and `0.4`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PriceTableId(String);

impl PriceTableId {
    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PriceTableId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The prices of a deployment (`usage.prices`): one currency, the models, and optionally the
/// sandbox. Built by [`PriceTable::new`], which refuses what a person could not have meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceTable {
    currency: Currency,
    /// Ordered by provider (none first) and model; no two alike.
    models: Vec<ModelPrice>,
    sandbox: Option<SandboxPrice>,
    id: PriceTableId,
}

impl PriceTable {
    /// The table of `currency`, `models` (any order) and `sandbox`.
    ///
    /// # Errors
    ///
    /// [`PriceTableError`] for two entries of one provider and model, a price above
    /// [`MAX_PRICE_MICROS`], or more than [`MAX_PRICED_MODELS`] entries.
    pub fn new(
        currency: Currency,
        mut models: Vec<ModelPrice>,
        sandbox: Option<SandboxPrice>,
    ) -> Result<PriceTable, PriceTableError> {
        if models.len() > MAX_PRICED_MODELS {
            return Err(PriceTableError::TooManyModels);
        }
        let too_high = |m: Micros| m.get() > u128::from(MAX_PRICE_MICROS);
        if models
            .iter()
            .any(|m| too_high(m.input) || too_high(m.cached_input) || too_high(m.output))
            || sandbox.is_some_and(|s| too_high(s.cpu_core_hour) || too_high(s.gib_hour))
        {
            return Err(PriceTableError::PriceTooHigh);
        }
        models.sort_by(|a, b| {
            (a.provider.as_deref(), &a.model).cmp(&(b.provider.as_deref(), &b.model))
        });
        if let Some(pair) = models
            .windows(2)
            .find(|w| w[0].provider == w[1].provider && w[0].model == w[1].model)
        {
            return Err(PriceTableError::DuplicateModel {
                provider: pair[0].provider.clone(),
                model: pair[0].model.clone(),
            });
        }
        let id = table_id(currency, &models, sandbox);
        Ok(PriceTable {
            currency,
            models,
            sandbox,
            id,
        })
    }

    /// The currency every price is in.
    pub fn currency(&self) -> Currency {
        self.currency
    }

    /// The id of this table.
    pub fn id(&self) -> &PriceTableId {
        &self.id
    }

    /// The model entries, ordered by provider (none first) and model.
    pub fn models(&self) -> &[ModelPrice] {
        &self.models
    }

    /// The sandbox prices, when the table has them.
    pub fn sandbox(&self) -> Option<SandboxPrice> {
        self.sandbox
    }

    fn model_price(&self, line: &ModelTokens) -> Option<&ModelPrice> {
        self.models
            .iter()
            .find(|p| p.provider == line.provider && p.model == line.model)
    }
}

/// The canonical text of a table, one line each, text as JSON strings, numbers as integers. Not a
/// wire format: only its hash is ever seen.
fn table_id(
    currency: Currency,
    models: &[ModelPrice],
    sandbox: Option<SandboxPrice>,
) -> PriceTableId {
    let text = |s: &str| serde_json::to_string(s).unwrap_or_default();
    let mut canonical = String::from("usage-prices/v1\n");
    canonical.push_str(&format!("currency:{currency}\n"));
    for m in models {
        let provider = m
            .provider
            .as_deref()
            .map_or_else(|| "null".to_owned(), text);
        canonical.push_str(&format!(
            "model:{provider}:{}:{}:{}:{}\n",
            text(&m.model),
            m.input.get(),
            m.cached_input.get(),
            m.output.get()
        ));
    }
    match sandbox {
        Some(s) => canonical.push_str(&format!(
            "sandbox:{}:{}\n",
            s.cpu_core_hour.get(),
            s.gib_hour.get()
        )),
        None => canonical.push_str("sandbox:none\n"),
    }
    let digest = Sha256::digest(canonical.as_bytes());
    let mut id = String::from("sha256:");
    for byte in digest.iter().take(8) {
        id.push_str(&format!("{byte:02x}"));
    }
    PriceTableId(id)
}

/// Why a line has no price. Closed (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unpriced {
    /// The deployment configured no prices.
    NoTable,
    /// The table has no entry for the line's provider and model.
    NoModelPrice,
    /// The table has no sandbox prices.
    NoSandboxPrice,
    /// The shape lacks its CPU or its memory request.
    NoRequests,
    /// The arithmetic would overflow `u128`.
    Overflow,
}

impl Unpriced {
    /// A word for a log line.
    pub const fn as_str(self) -> &'static str {
        match self {
            Unpriced::NoTable => "no_table",
            Unpriced::NoModelPrice => "no_model_price",
            Unpriced::NoSandboxPrice => "no_sandbox_price",
            Unpriced::NoRequests => "no_requests",
            Unpriced::Overflow => "overflow",
        }
    }
}

/// The price of one line: an amount, or the reason there is none. Never a zero in place of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinePrice {
    /// The line costs this much.
    Priced(Micros),
    /// The line cannot be priced.
    Unpriced(Unpriced),
}

impl LinePrice {
    /// The amount, when the line is priced.
    pub fn amount(self) -> Option<Micros> {
        match self {
            LinePrice::Priced(amount) => Some(amount),
            LinePrice::Unpriced(_) => None,
        }
    }
}

/// A provider and model, what an unpriced line is named by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelKey {
    /// The provider, when the line has one.
    pub provider: Option<String>,
    /// The model.
    pub model: String,
}

/// A model line with its price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricedModel {
    /// The month's counts for the provider and model.
    pub usage: ModelTokens,
    /// What they cost.
    pub price: LinePrice,
}

/// A sandbox shape with its price.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricedSandbox {
    /// The month's sandbox time of the shape.
    pub shape: SandboxShape,
    /// What it costs.
    pub price: LinePrice,
}

/// What a total could not price.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnpricedSummary {
    /// The model lines with no price, in the order of the lines.
    pub models: Vec<ModelKey>,
    /// How many sandbox shapes have no price.
    pub sandbox: u64,
    /// How many tasks ran their commands where the sandbox is not measured.
    pub unmeasured: u64,
}

/// The total of a priced month.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricedTotal {
    /// The sum of the lines that are priced.
    pub amount: Micros,
    /// The currency of the amount; `None` when no prices are configured.
    pub currency: Option<Currency>,
    /// Whether the amount is everything the month used: no line is unpriced and nothing is
    /// unmeasured. A month with no usage is complete.
    pub complete: bool,
    /// What is missing from the amount.
    pub unpriced: UnpricedSummary,
}

/// A month of usage with the table's prices put on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricedMonth {
    /// The month.
    pub month: UsageMonth,
    /// The currency of the prices; `None` when no prices are configured.
    pub currency: Option<Currency>,
    /// The table that priced it; `None` when no prices are configured.
    pub prices: Option<PriceTableId>,
    /// How many tasks the month holds.
    pub tasks: u64,
    /// The model lines, in the order of the totals.
    pub models: Vec<PricedModel>,
    /// The measured sandbox shapes, in the order of the totals.
    pub sandbox: Vec<PricedSandbox>,
    /// The tasks whose sandbox was not measured; never priced.
    pub unmeasured: Vec<UnmeasuredSandbox>,
    /// The total.
    pub total: PricedTotal,
}

/// Puts `table`'s prices on `totals` (`None`: nothing is priced and every line says so).
///
/// Pure and total: it never fails and never panics, whatever the counts.
pub fn price_month(table: Option<&PriceTable>, totals: &UsageMonthTotals) -> PricedMonth {
    let models: Vec<PricedModel> = totals
        .models
        .iter()
        .map(|line| PricedModel {
            usage: line.clone(),
            price: match table {
                None => LinePrice::Unpriced(Unpriced::NoTable),
                Some(table) => match table.model_price(line) {
                    None => LinePrice::Unpriced(Unpriced::NoModelPrice),
                    Some(price) => token_cost(&line.tokens, price),
                },
            },
        })
        .collect();
    let sandbox: Vec<PricedSandbox> = totals
        .sandbox
        .iter()
        .map(|shape| PricedSandbox {
            shape: shape.clone(),
            price: match table {
                None => LinePrice::Unpriced(Unpriced::NoTable),
                Some(table) => match table.sandbox {
                    None => LinePrice::Unpriced(Unpriced::NoSandboxPrice),
                    Some(price) => sandbox_cost(shape, price),
                },
            },
        })
        .collect();

    let mut amount = Micros::ZERO;
    let mut summed = true;
    let mut add = |price: LinePrice| {
        if let LinePrice::Priced(line) = price {
            match amount.0.checked_add(line.0) {
                Some(sum) => amount = Micros(sum),
                None => {
                    amount = Micros(u128::MAX);
                    summed = false;
                }
            }
        }
    };
    models.iter().for_each(|l| add(l.price));
    sandbox.iter().for_each(|l| add(l.price));

    let unpriced = UnpricedSummary {
        models: models
            .iter()
            .filter(|l| l.price.amount().is_none())
            .map(|l| ModelKey {
                provider: l.usage.provider.clone(),
                model: l.usage.model.clone(),
            })
            .collect(),
        sandbox: sandbox
            .iter()
            .filter(|l| l.price.amount().is_none())
            .count() as u64,
        unmeasured: totals
            .unmeasured
            .iter()
            .fold(0_u64, |n, u| n.saturating_add(u.tasks)),
    };
    let complete =
        summed && unpriced.models.is_empty() && unpriced.sandbox == 0 && unpriced.unmeasured == 0;
    let currency = table.map(PriceTable::currency);
    PricedMonth {
        month: totals.month,
        currency,
        prices: table.map(|t| t.id().clone()),
        tasks: totals.tasks,
        models,
        sandbox,
        unmeasured: totals.unmeasured.clone(),
        total: PricedTotal {
            amount,
            currency,
            complete,
            unpriced,
        },
    }
}

/// `numerator / denominator`, rounded half up (`denominator` is positive).
fn divide_half_up(numerator: u128, denominator: u128) -> u128 {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    // `remainder * 2 >= denominator` without the doubling, which cannot overflow here anyway
    if remainder >= denominator - remainder {
        quotient + 1
    } else {
        quotient
    }
}

fn token_cost(tokens: &TokenCounts, price: &ModelPrice) -> LinePrice {
    let input = u128::from(tokens.input_tokens);
    let cached = u128::from(tokens.cached_input_tokens.unwrap_or(0)).min(input);
    let uncached = input - cached;
    let output = u128::from(tokens.output_tokens);
    let numerator = uncached
        .checked_mul(price.input.0)
        .zip(cached.checked_mul(price.cached_input.0))
        .zip(output.checked_mul(price.output.0))
        .and_then(|((a, b), c)| a.checked_add(b)?.checked_add(c));
    match numerator {
        Some(n) => LinePrice::Priced(Micros(divide_half_up(n, TOKENS_PER_PRICE))),
        None => LinePrice::Unpriced(Unpriced::Overflow),
    }
}

fn sandbox_cost(shape: &SandboxShape, price: SandboxPrice) -> LinePrice {
    let (Some(cpu), Some(memory)) = (shape.cpu_millis, shape.memory_bytes) else {
        return LinePrice::Unpriced(Unpriced::NoRequests);
    };
    // hours * (cores * cpu_price + gib * gib_price)
    //   = ms / 3.6e6 * (cpu / 1000 * pc + memory / 2^30 * pg)
    //   = ms * (cpu * pc * 2^30 + memory * pg * 1000) / (3.6e6 * 1000 * 2^30)
    let cpu_part = u128::from(cpu)
        .checked_mul(price.cpu_core_hour.0)
        .and_then(|n| n.checked_mul(BYTES_PER_GIB));
    let memory_part = u128::from(memory)
        .checked_mul(price.gib_hour.0)
        .and_then(|n| n.checked_mul(MILLICORES_PER_CORE));
    let numerator = cpu_part
        .zip(memory_part)
        .and_then(|(c, m)| c.checked_add(m))
        .and_then(|n| n.checked_mul(u128::from(shape.milliseconds)));
    let denominator = MS_PER_HOUR * MILLICORES_PER_CORE * BYTES_PER_GIB;
    match numerator {
        Some(n) => LinePrice::Priced(Micros(divide_half_up(n, denominator))),
        None => LinePrice::Unpriced(Unpriced::Overflow),
    }
}
