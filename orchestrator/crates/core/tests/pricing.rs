//! Pricing a month of usage (ADR 0065, decisions 5 to 7): integers all the way, one rounding per
//! line (half up), a line that cannot be priced says so and never counts as free, and a total that
//! says whether it is complete.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;
use proptest::prelude::*;

fn micros(n: u128) -> Micros {
    Micros::new(n)
}

fn eur() -> Currency {
    Currency::new("EUR").unwrap()
}

fn month() -> UsageMonth {
    "2026-10".parse().unwrap()
}

fn counts(input: u64, output: u64) -> TokenCounts {
    TokenCounts {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        ..TokenCounts::default()
    }
}

fn model(provider: Option<&str>, name: &str, tokens: TokenCounts) -> ModelTokens {
    ModelTokens {
        provider: provider.map(Into::into),
        model: name.into(),
        tokens,
    }
}

fn price(
    provider: Option<&str>,
    name: &str,
    input: u128,
    cached: u128,
    output: u128,
) -> ModelPrice {
    ModelPrice {
        provider: provider.map(Into::into),
        model: name.into(),
        input: micros(input),
        cached_input: micros(cached),
        output: micros(output),
    }
}

fn table(models: Vec<ModelPrice>, sandbox: Option<SandboxPrice>) -> PriceTable {
    PriceTable::new(eur(), models, sandbox).unwrap()
}

fn totals(models: Vec<ModelTokens>) -> UsageMonthTotals {
    UsageMonthTotals::new(month(), 1, models, Vec::new(), Vec::new())
}

fn shape(cpu: Option<u64>, memory: Option<u64>, commands: u64, milliseconds: u64) -> SandboxShape {
    SandboxShape {
        environment: SandboxEnvironment::Kubernetes,
        cpu_millis: cpu,
        memory_bytes: memory,
        commands,
        milliseconds,
    }
}

fn sandbox_price(cpu_core_hour: u128, gib_hour: u128) -> SandboxPrice {
    SandboxPrice {
        cpu_core_hour: micros(cpu_core_hour),
        gib_hour: micros(gib_hour),
    }
}

const GIB: u64 = 1 << 30;
const HOUR_MS: u64 = 3_600_000;

fn line_amount(priced: &PricedMonth, index: usize) -> Option<u128> {
    priced.models[index].price.amount().map(Micros::get)
}

// ---- money ---------------------------------------------------------------------------------

#[test]
fn an_amount_is_written_with_six_fraction_digits() {
    assert_eq!(micros(0).to_string(), "0.000000");
    assert_eq!(micros(1).to_string(), "0.000001");
    assert_eq!(micros(1_234).to_string(), "0.001234");
    assert_eq!(micros(1_460).to_string(), "0.001460");
    assert_eq!(micros(1_000_000).to_string(), "1.000000");
    assert_eq!(micros(123_456_789_012).to_string(), "123456.789012");
    assert_eq!(
        Micros::new(u128::MAX).to_string(),
        "340282366920938463463374607431768.211455",
        "the largest amount still has exactly six fraction digits"
    );
}

#[test]
fn a_currency_is_three_upper_case_ascii_letters() {
    assert_eq!(Currency::new("EUR").unwrap().as_str(), "EUR");
    assert_eq!(Currency::new("USD").unwrap().to_string(), "USD");
    for bad in [
        "",
        "EU",
        "EURO",
        "eur",
        "Eur",
        "E1R",
        "EU ",
        " EUR",
        "\u{20ac}\u{20ac}\u{20ac}",
    ] {
        assert!(Currency::new(bad).is_err(), "{bad:?}");
    }
}

// ---- the table -----------------------------------------------------------------------------

#[test]
fn a_table_refuses_two_entries_for_the_same_provider_and_model() {
    let err = PriceTable::new(
        eur(),
        vec![
            price(Some("openai"), "glm", 1, 1, 1),
            price(Some("openai"), "glm", 2, 2, 2),
        ],
        None,
    )
    .unwrap_err();
    assert_eq!(
        err,
        PriceTableError::DuplicateModel {
            provider: Some("openai".into()),
            model: "glm".into()
        }
    );
    // an absent provider is a provider of its own
    assert!(
        PriceTable::new(
            eur(),
            vec![
                price(Some("openai"), "glm", 1, 1, 1),
                price(None, "glm", 1, 1, 1)
            ],
            None
        )
        .is_ok()
    );
    // and so is the same name under another provider
    assert!(
        PriceTable::new(
            eur(),
            vec![
                price(Some("a"), "glm", 1, 1, 1),
                price(Some("b"), "glm", 1, 1, 1)
            ],
            None
        )
        .is_ok()
    );
}

#[test]
fn a_table_refuses_a_price_above_the_bound() {
    assert_eq!(MAX_PRICE_MICROS, 1_000_000_000_000, "1 000 000 units");
    let ok = PriceTable::new(
        eur(),
        vec![price(None, "m", MAX_PRICE_MICROS.into(), 0, 0)],
        Some(sandbox_price(MAX_PRICE_MICROS.into(), 0)),
    );
    assert!(ok.is_ok());
    for (what, models, sandbox) in [
        (
            "input",
            vec![price(None, "m", u128::from(MAX_PRICE_MICROS) + 1, 0, 0)],
            None,
        ),
        (
            "cached",
            vec![price(None, "m", 0, u128::from(MAX_PRICE_MICROS) + 1, 0)],
            None,
        ),
        (
            "output",
            vec![price(None, "m", 0, 0, u128::from(MAX_PRICE_MICROS) + 1)],
            None,
        ),
        (
            "cpu",
            Vec::new(),
            Some(sandbox_price(u128::from(MAX_PRICE_MICROS) + 1, 0)),
        ),
        (
            "gib",
            Vec::new(),
            Some(sandbox_price(0, u128::from(MAX_PRICE_MICROS) + 1)),
        ),
    ] {
        assert_eq!(
            PriceTable::new(eur(), models, sandbox).unwrap_err(),
            PriceTableError::PriceTooHigh,
            "{what}"
        );
    }
}

#[test]
fn the_id_of_a_table_is_short_stable_and_does_not_depend_on_the_order_of_the_entries() {
    let a = table(
        vec![
            price(Some("openai"), "glm-5.3", 400_000, 100_000, 1_600_000),
            price(None, "local", 1, 1, 1),
        ],
        Some(sandbox_price(40_000, 5_000)),
    );
    let b = table(
        vec![
            price(None, "local", 1, 1, 1),
            price(Some("openai"), "glm-5.3", 400_000, 100_000, 1_600_000),
        ],
        Some(sandbox_price(40_000, 5_000)),
    );
    assert_eq!(a.id(), b.id());
    let text = a.id().to_string();
    assert!(text.starts_with("sha256:"), "{text}");
    assert_eq!(text.len(), "sha256:".len() + 16, "{text}");
    assert!(
        text["sha256:".len()..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "{text}"
    );
    // it is a function of the parsed table, so every number, the currency and the sandbox change it
    let changed = [
        PriceTable::new(
            Currency::new("USD").unwrap(),
            vec![
                price(Some("openai"), "glm-5.3", 400_000, 100_000, 1_600_000),
                price(None, "local", 1, 1, 1),
            ],
            Some(sandbox_price(40_000, 5_000)),
        )
        .unwrap(),
        table(
            vec![
                price(Some("openai"), "glm-5.3", 400_001, 100_000, 1_600_000),
                price(None, "local", 1, 1, 1),
            ],
            Some(sandbox_price(40_000, 5_000)),
        ),
        table(
            vec![
                price(Some("openai"), "glm-5.3", 400_000, 100_000, 1_600_000),
                price(None, "local", 1, 1, 1),
            ],
            Some(sandbox_price(40_000, 5_001)),
        ),
        table(
            vec![
                price(Some("openai"), "glm-5.3", 400_000, 100_000, 1_600_000),
                price(None, "local", 1, 1, 1),
            ],
            None,
        ),
        table(
            vec![
                price(Some("openai"), "glm-5.3", 400_000, 100_000, 1_600_000),
                price(Some(""), "local", 1, 1, 1),
            ],
            Some(sandbox_price(40_000, 5_000)),
        ),
    ];
    for other in &changed {
        assert_ne!(a.id(), other.id());
    }
}

// ---- tokens --------------------------------------------------------------------------------

#[test]
fn a_model_is_priced_per_million_tokens_with_the_cached_part_at_its_own_price() {
    // glm: 0.40 in, 0.10 cached, 1.60 out per million
    let t = table(
        vec![price(Some("openai"), "glm", 400_000, 100_000, 1_600_000)],
        None,
    );
    let tokens = TokenCounts {
        input_tokens: 1_000_000,
        output_tokens: 500_000,
        total_tokens: 1_500_000,
        cached_input_tokens: Some(400_000),
        reasoning_tokens: Some(100_000),
        cache_write_input_tokens: Some(50_000),
    };
    let priced = price_month(
        Some(&t),
        &totals(vec![model(Some("openai"), "glm", tokens)]),
    );
    // 600 000 uncached * 0.40 + 400 000 cached * 0.10 + 500 000 out * 1.60, per million:
    // 0.24 + 0.04 + 0.80 = 1.08 units; the cache writes are input, at the input price, and the
    // reasoning is output, at the output price: neither is a charge of its own
    assert_eq!(line_amount(&priced, 0), Some(1_080_000));
    assert_eq!(priced.total.amount, micros(1_080_000));
    assert!(priced.total.complete);
}

#[test]
fn cached_tokens_are_a_part_of_the_input_and_never_more_than_it() {
    let t = table(vec![price(None, "m", 1_000_000, 100_000, 0)], None);
    // an agent that says more cached than input: the cached part is capped at the input, the rest
    // (none) at the input price; never a negative uncached count
    let tokens = TokenCounts {
        input_tokens: 1_000,
        output_tokens: 0,
        total_tokens: 1_000,
        cached_input_tokens: Some(5_000),
        ..TokenCounts::default()
    };
    let priced = price_month(Some(&t), &totals(vec![model(None, "m", tokens)]));
    assert_eq!(
        line_amount(&priced, 0),
        Some(100),
        "1 000 tokens at 0.10 per million"
    );
    // no cached count at all: every input token is at the input price
    let priced = price_month(Some(&t), &totals(vec![model(None, "m", counts(1_000, 0))]));
    assert_eq!(line_amount(&priced, 0), Some(1_000));
}

#[test]
fn a_line_is_rounded_half_up_once() {
    // 1 token at 0.5 micro per token: 0.5 micro -> up to 1; at 0.4999 -> down to 0
    // (prices are per million tokens, so the numerator is tokens * price and the divisor 1e6)
    let t = table(vec![price(None, "m", 500_000, 0, 0)], None);
    let at = |input: u64| {
        let priced = price_month(Some(&t), &totals(vec![model(None, "m", counts(input, 0))]));
        line_amount(&priced, 0).unwrap()
    };
    assert_eq!(at(0), 0);
    assert_eq!(at(1), 1, "0.5 micro is half, rounded up");
    assert_eq!(at(2), 1, "1.0 micro");
    assert_eq!(at(3), 2, "1.5 micro is half, rounded up");
    assert_eq!(at(4), 2);
    let t = table(vec![price(None, "m", 499_999, 0, 0)], None);
    let priced = price_month(Some(&t), &totals(vec![model(None, "m", counts(1, 0))]));
    assert_eq!(
        line_amount(&priced, 0),
        Some(0),
        "0.499999 micro rounds down"
    );
}

#[test]
fn the_division_happens_once_per_line_not_per_part() {
    // three parts of 0.4 micro each: rounded one by one they would be 0 + 0 + 0; as one line they
    // are 1.2 micro, which is 1
    let t = table(vec![price(None, "m", 400_000, 400_000, 400_000)], None);
    let tokens = TokenCounts {
        input_tokens: 2,
        output_tokens: 1,
        total_tokens: 3,
        cached_input_tokens: Some(1),
        ..TokenCounts::default()
    };
    let priced = price_month(Some(&t), &totals(vec![model(None, "m", tokens)]));
    assert_eq!(line_amount(&priced, 0), Some(1));
}

#[test]
fn a_model_with_no_price_is_unpriced_never_free() {
    let t = table(
        vec![price(Some("openai"), "glm", 400_000, 100_000, 1_600_000)],
        None,
    );
    let priced = price_month(
        Some(&t),
        &totals(vec![
            model(Some("openai"), "glm", counts(1_000_000, 0)),
            model(Some("openai"), "glm-mini", counts(5, 5)),
        ]),
    );
    assert_eq!(priced.models.len(), 2);
    assert_eq!(priced.models[0].price, LinePrice::Priced(micros(400_000)));
    assert_eq!(
        priced.models[1].price,
        LinePrice::Unpriced(Unpriced::NoModelPrice)
    );
    assert_eq!(
        priced.total.amount,
        micros(400_000),
        "the sum of the priced lines"
    );
    assert!(!priced.total.complete);
    assert_eq!(
        priced.total.unpriced.models,
        vec![ModelKey {
            provider: Some("openai".into()),
            model: "glm-mini".into()
        }]
    );
    assert_eq!(
        (
            priced.total.unpriced.sandbox,
            priced.total.unpriced.unmeasured
        ),
        (0, 0)
    );
}

#[test]
fn a_line_matches_the_entry_of_its_provider_and_model_exactly() {
    let t = table(
        vec![
            price(Some("openai"), "glm", 1_000_000, 0, 0),
            price(None, "bare", 2_000_000, 0, 0),
        ],
        None,
    );
    let priced = price_month(
        Some(&t),
        &totals(vec![
            // no provider matches the entry that has none, and only that one
            model(None, "bare", counts(1, 0)),
            model(Some("other"), "bare", counts(1, 0)),
            // a provider does not match an entry without one, and the model is compared exactly
            model(None, "glm", counts(1, 0)),
            model(Some("openai"), "GLM", counts(1, 0)),
            model(Some("openai"), "glm", counts(1, 0)),
            model(Some("OpenAI"), "glm", counts(1, 0)),
        ]),
    );
    let by_key = |provider: Option<&str>, name: &str| {
        priced
            .models
            .iter()
            .find(|l| l.usage.provider.as_deref() == provider && l.usage.model == name)
            .unwrap()
            .price
    };
    assert_eq!(by_key(None, "bare"), LinePrice::Priced(micros(2)));
    assert_eq!(
        by_key(Some("other"), "bare"),
        LinePrice::Unpriced(Unpriced::NoModelPrice)
    );
    assert_eq!(
        by_key(None, "glm"),
        LinePrice::Unpriced(Unpriced::NoModelPrice)
    );
    assert_eq!(
        by_key(Some("openai"), "GLM"),
        LinePrice::Unpriced(Unpriced::NoModelPrice)
    );
    assert_eq!(by_key(Some("openai"), "glm"), LinePrice::Priced(micros(1)));
    assert_eq!(
        by_key(Some("OpenAI"), "glm"),
        LinePrice::Unpriced(Unpriced::NoModelPrice)
    );
}

#[test]
fn an_overflow_makes_the_line_unpriced_and_nothing_else() {
    // the largest price times the largest count is below u128::MAX; a sandbox with absurd times is
    // what overflows, and a token line cannot
    let t = table(
        vec![price(
            None,
            "m",
            MAX_PRICE_MICROS.into(),
            MAX_PRICE_MICROS.into(),
            MAX_PRICE_MICROS.into(),
        )],
        Some(sandbox_price(
            MAX_PRICE_MICROS.into(),
            MAX_PRICE_MICROS.into(),
        )),
    );
    let huge = TokenCounts {
        input_tokens: u64::MAX,
        output_tokens: u64::MAX,
        total_tokens: u64::MAX,
        cached_input_tokens: Some(u64::MAX / 2),
        ..TokenCounts::default()
    };
    let mut month_totals = totals(vec![model(None, "m", huge), model(None, "n", counts(1, 1))]);
    month_totals.sandbox = vec![
        shape(Some(u64::MAX), Some(u64::MAX), 1, u64::MAX),
        shape(Some(1000), Some(GIB), 1, HOUR_MS),
    ];
    let priced = price_month(Some(&t), &month_totals);
    assert!(
        matches!(priced.models[0].price, LinePrice::Priced(_)),
        "{:?}",
        priced.models[0]
    );
    assert_eq!(
        priced.models[1].price,
        LinePrice::Unpriced(Unpriced::NoModelPrice)
    );
    assert_eq!(priced.sandbox.len(), 2);
    let absurd = priced
        .sandbox
        .iter()
        .find(|s| s.shape.milliseconds == u64::MAX)
        .unwrap();
    assert_eq!(absurd.price, LinePrice::Unpriced(Unpriced::Overflow));
    let sane = priced
        .sandbox
        .iter()
        .find(|s| s.shape.milliseconds == HOUR_MS)
        .unwrap();
    assert_eq!(
        sane.price,
        LinePrice::Priced(micros(2 * u128::from(MAX_PRICE_MICROS))),
        "one core and one GiB for one hour"
    );
    assert!(!priced.total.complete);
    assert_eq!(priced.total.unpriced.sandbox, 1);
}

// ---- the sandbox ---------------------------------------------------------------------------

#[test]
fn a_sandbox_shape_is_priced_by_core_hours_and_gib_hours() {
    // 0.04 per core-hour, 0.005 per GiB-hour; a quarter core and half a GiB for 81 234 ms
    let t = table(Vec::new(), Some(sandbox_price(40_000, 5_000)));
    let mut totals = UsageMonthTotals::empty(month());
    totals.tasks = 1;
    totals.sandbox = vec![shape(Some(250), Some(GIB / 2), 12, 81_234)];
    let priced = price_month(Some(&t), &totals);
    // hours = 81234 / 3600000 = 0.0225650; cost = hours * (0.25 * 40000 + 0.5 * 5000) micros
    //      = 0.022565 * 12500 = 282.0625 micros -> 282
    assert_eq!(priced.sandbox[0].price, LinePrice::Priced(micros(282)));
    assert_eq!(priced.sandbox[0].shape.commands, 12);
    assert_eq!(priced.total.amount, micros(282));
    assert!(priced.total.complete);
}

#[test]
fn a_sandbox_line_is_rounded_half_up_once() {
    // one core for exactly one hour at 1 micro per core-hour is 1 micro; for half an hour it is
    // 0.5 micro, which is half, up
    let t = table(Vec::new(), Some(sandbox_price(1, 0)));
    let cost = |ms: u64| {
        let mut totals = UsageMonthTotals::empty(month());
        totals.sandbox = vec![shape(Some(1000), Some(0), 1, ms)];
        match price_month(Some(&t), &totals).sandbox[0].price {
            LinePrice::Priced(m) => m.get(),
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(cost(HOUR_MS), 1);
    assert_eq!(cost(HOUR_MS / 2), 1, "0.5 micro, half up");
    assert_eq!(cost(HOUR_MS / 2 - 1), 0, "just under half");
    assert_eq!(cost(HOUR_MS * 3 / 2), 2, "1.5 micro, half up");
    assert_eq!(cost(0), 0);
}

#[test]
fn a_shape_with_no_request_is_unpriced() {
    let t = table(Vec::new(), Some(sandbox_price(40_000, 5_000)));
    let mut totals = UsageMonthTotals::empty(month());
    totals.sandbox = vec![
        shape(None, Some(GIB), 1, HOUR_MS),
        shape(Some(1000), None, 1, HOUR_MS),
        shape(None, None, 1, HOUR_MS),
        shape(Some(1000), Some(GIB), 1, HOUR_MS),
    ];
    let priced = price_month(Some(&t), &totals);
    let unpriced: Vec<_> = priced
        .sandbox
        .iter()
        .filter(|s| s.price == LinePrice::Unpriced(Unpriced::NoRequests))
        .collect();
    assert_eq!(
        unpriced.len(),
        3,
        "a missing CPU or memory request is not zero"
    );
    assert_eq!(
        priced.total.amount,
        micros(45_000),
        "only the shape that has both"
    );
    assert_eq!(priced.total.unpriced.sandbox, 3);
    assert!(!priced.total.complete);
}

#[test]
fn without_sandbox_prices_every_measured_shape_is_unpriced() {
    let t = table(vec![price(None, "m", 1, 1, 1)], None);
    let mut totals = UsageMonthTotals::empty(month());
    totals.sandbox = vec![shape(Some(1000), Some(GIB), 1, HOUR_MS)];
    let priced = price_month(Some(&t), &totals);
    assert_eq!(
        priced.sandbox[0].price,
        LinePrice::Unpriced(Unpriced::NoSandboxPrice)
    );
    assert_eq!(priced.total.amount, micros(0));
    assert!(!priced.total.complete);
    assert_eq!(priced.total.unpriced.sandbox, 1);
}

#[test]
fn an_unmeasured_sandbox_is_never_priced_and_makes_the_total_incomplete() {
    let t = table(
        vec![price(None, "m", 1_000_000, 0, 0)],
        Some(sandbox_price(1, 1)),
    );
    let mut totals = totals(vec![model(None, "m", counts(1, 0))]);
    totals.unmeasured = vec![
        UnmeasuredSandbox {
            environment: SandboxEnvironment::Devcontainer,
            tasks: 2,
        },
        UnmeasuredSandbox {
            environment: SandboxEnvironment::Local,
            tasks: 1,
        },
    ];
    let priced = price_month(Some(&t), &totals);
    assert_eq!(priced.unmeasured, totals.unmeasured, "reported as they are");
    assert_eq!(priced.total.amount, micros(1));
    assert!(!priced.total.complete);
    assert_eq!(
        priced.total.unpriced.unmeasured, 3,
        "tasks, not environments"
    );
    assert!(priced.total.unpriced.models.is_empty());
}

// ---- no table, no usage ----------------------------------------------------------------------

#[test]
fn no_usage_at_all_is_complete_and_zero_with_a_table_or_without() {
    let t = table(vec![price(None, "m", 1, 1, 1)], None);
    for table in [Some(&t), None] {
        let priced = price_month(table, &UsageMonthTotals::empty(month()));
        assert_eq!(priced.month, month());
        assert_eq!(priced.tasks, 0);
        assert!(
            priced.models.is_empty() && priced.sandbox.is_empty() && priced.unmeasured.is_empty()
        );
        assert_eq!(priced.total.amount, micros(0));
        assert!(priced.total.complete);
        assert_eq!(priced.currency.is_some(), table.is_some());
        assert_eq!(priced.prices.is_some(), table.is_some());
    }
}

#[test]
fn with_no_table_nothing_is_priced_and_no_currency_or_table_is_named() {
    let mut totals = totals(vec![model(Some("openai"), "glm", counts(10, 10))]);
    totals.sandbox = vec![shape(Some(1000), Some(GIB), 1, HOUR_MS)];
    let priced = price_month(None, &totals);
    assert_eq!(priced.currency, None);
    assert_eq!(priced.prices, None);
    assert_eq!(priced.total.currency, None);
    assert_eq!(
        priced.models[0].price,
        LinePrice::Unpriced(Unpriced::NoTable)
    );
    assert_eq!(
        priced.sandbox[0].price,
        LinePrice::Unpriced(Unpriced::NoTable)
    );
    assert_eq!(priced.total.amount, micros(0));
    assert!(!priced.total.complete);
    assert_eq!(priced.total.unpriced.models.len(), 1);
    assert_eq!(priced.total.unpriced.sandbox, 1);
}

#[test]
fn a_priced_answer_names_the_table_that_priced_it() {
    let t = table(vec![price(None, "m", 1, 1, 1)], None);
    let priced = price_month(Some(&t), &UsageMonthTotals::empty(month()));
    assert_eq!(priced.currency, Some(eur()));
    assert_eq!(priced.total.currency, Some(eur()));
    assert_eq!(priced.prices.as_ref(), Some(t.id()));
}

#[test]
fn the_total_is_the_sum_of_the_priced_lines() {
    let t = table(
        vec![
            price(Some("openai"), "a", 1_000_000, 0, 0),
            price(Some("openai"), "b", 2_000_000, 0, 0),
        ],
        Some(sandbox_price(1_000_000, 0)),
    );
    let mut totals = totals(vec![
        model(Some("openai"), "a", counts(3, 0)),
        model(Some("openai"), "b", counts(4, 0)),
        model(Some("openai"), "c", counts(100, 0)),
    ]);
    totals.sandbox = vec![shape(Some(1000), Some(0), 1, HOUR_MS)];
    let priced = price_month(Some(&t), &totals);
    assert_eq!(priced.total.amount, micros(3 + 8 + 1_000_000));
}

// ---- properties ----------------------------------------------------------------------------

fn arbitrary_counts() -> impl Strategy<Value = TokenCounts> {
    (
        0..=u64::MAX / 4,
        0..=u64::MAX / 4,
        proptest::option::of(0..=u64::MAX / 2),
    )
        .prop_map(|(input, output, cached)| TokenCounts {
            input_tokens: input,
            output_tokens: output,
            total_tokens: input + output,
            cached_input_tokens: cached,
            ..TokenCounts::default()
        })
}

fn one_model_table(input: u64, cached: u64, output: u64) -> PriceTable {
    table(
        vec![price(None, "m", input.into(), cached.into(), output.into())],
        None,
    )
}

fn cost_of(t: &PriceTable, tokens: TokenCounts) -> u128 {
    let priced = price_month(Some(t), &totals(vec![model(None, "m", tokens)]));
    line_amount(&priced, 0).unwrap()
}

proptest! {
    /// The rounded line is the exact rational within half a micro, whatever the counts and prices.
    #[test]
    fn a_line_is_within_half_a_micro_of_the_exact_cost(
        tokens in arbitrary_counts(),
        input in 0..=MAX_PRICE_MICROS,
        cached in 0..=MAX_PRICE_MICROS,
        output in 0..=MAX_PRICE_MICROS,
    ) {
        let t = one_model_table(input, cached, output);
        let cost = cost_of(&t, tokens);
        let cached_part = u128::from(tokens.cached_input_tokens.unwrap_or(0)).min(u128::from(tokens.input_tokens));
        let exact = (u128::from(tokens.input_tokens) - cached_part) * u128::from(input)
            + cached_part * u128::from(cached)
            + u128::from(tokens.output_tokens) * u128::from(output);
        // |cost * 1e6 - exact| <= 500 000, and half is up
        let scaled = cost * 1_000_000;
        prop_assert!(scaled <= exact + 500_000 && exact < scaled + 500_000, "{cost} {exact}");
    }

    /// Splitting a count in two never moves the sum by more than the two roundings.
    #[test]
    fn pricing_is_additive_up_to_one_rounding(
        a in 0..=u64::MAX / 8,
        b in 0..=u64::MAX / 8,
        input in 0..=MAX_PRICE_MICROS,
        output in 0..=MAX_PRICE_MICROS,
    ) {
        let t = one_model_table(input, 0, output);
        let whole = cost_of(&t, counts(a + b, a + b));
        let parts = cost_of(&t, counts(a, a)) + cost_of(&t, counts(b, b));
        prop_assert!(whole.abs_diff(parts) <= 1, "{whole} {parts}");
    }

    /// More tokens never cost less, and a dearer price never costs less.
    #[test]
    fn pricing_is_monotone(
        a in 0..=u64::MAX / 8,
        extra in 0..=u64::MAX / 8,
        input in 0..=MAX_PRICE_MICROS,
        dearer in 0..=1_000_000_u64,
    ) {
        let t = one_model_table(input, input, input);
        let more = cost_of(&t, counts(a + extra, 0));
        prop_assert!(more >= cost_of(&t, counts(a, 0)));
        let t2 = one_model_table((input + dearer).min(MAX_PRICE_MICROS), input, input);
        prop_assert!(cost_of(&t2, counts(a, 0)) >= cost_of(&t, counts(a, 0)));
    }

    /// The total is exactly the sum of the priced lines, however many are unpriced.
    #[test]
    fn the_total_is_the_sum_of_the_priced_lines_always(
        lines in proptest::collection::vec((0_u8..4, 0..=u64::MAX / 8, 0..=u64::MAX / 8), 0..8),
    ) {
        let t = table(
            vec![
                price(None, "m0", 1_000_000, 0, 0),
                price(None, "m1", 333_333, 7, 999_999),
            ],
            None,
        );
        // one line per model name, at most
        let mut seen = std::collections::BTreeSet::new();
        let models: Vec<ModelTokens> = lines
            .into_iter()
            .filter(|(m, _, _)| seen.insert(*m))
            .map(|(m, i, o)| model(None, &format!("m{m}"), counts(i, o)))
            .collect();
        let priced = price_month(Some(&t), &totals(models));
        let sum: u128 = priced.models.iter().filter_map(|l| l.price.amount()).map(Micros::get).sum();
        prop_assert_eq!(priced.total.amount.get(), sum);
        let unpriced = priced.models.iter().filter(|l| l.price.amount().is_none()).count();
        prop_assert_eq!(priced.total.complete, unpriced == 0);
        prop_assert_eq!(priced.total.unpriced.models.len(), unpriced);
    }

    /// A sandbox line is within half a micro of the exact rational.
    #[test]
    fn a_sandbox_line_is_within_half_a_micro_of_the_exact_cost(
        cpu in 0..=64_000_u64,
        memory in 0..=(256 * GIB),
        ms in 0..=(30 * 24 * HOUR_MS),
        cpu_price in 0..=MAX_PRICE_MICROS,
        gib_price in 0..=MAX_PRICE_MICROS,
    ) {
        let t = table(Vec::new(), Some(sandbox_price(cpu_price.into(), gib_price.into())));
        let mut totals = UsageMonthTotals::empty(month());
        totals.sandbox = vec![shape(Some(cpu), Some(memory), 1, ms)];
        let cost = match price_month(Some(&t), &totals).sandbox[0].price {
            LinePrice::Priced(m) => m.get(),
            other => return Err(TestCaseError::fail(format!("{other:?}"))),
        };
        // cost = ms/3.6e6 * (cpu/1000 * pc + memory/2^30 * pg), scaled by the denominator
        let den: u128 = 3_600_000 * 1000 * u128::from(GIB);
        let num = u128::from(ms)
            * (u128::from(cpu) * u128::from(cpu_price) * u128::from(GIB)
                + u128::from(memory) * u128::from(gib_price) * 1000);
        let scaled = cost * den;
        prop_assert!(scaled <= num + den / 2 && num < scaled + den / 2, "{cost}");
    }
}
