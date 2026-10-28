//! `usage` (ADR 0065): the retention of the ledger and the price table. A price is a decimal string
//! read exactly into millionths; every refusal names the key path and quotes nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::{
    DEFAULT_USAGE_RETENTION_MONTHS, MAX_PRICE_MICROS, PriceError, parse_price, render,
};
use support::{Fake, lines, load};

const MINIMAL: &str = "\
version: 1
database:
  url: { env: DATABASE_URL }
agents:
  file: agents.yaml
";

fn env() -> Fake {
    Fake::default().env("DATABASE_URL", "postgres://u:pw@db/orch")
}

fn with(usage: &str) -> String {
    format!("{MINIMAL}usage:\n{usage}")
}

const FULL: &str = r#"  retentionMonths: 12
  prices:
    currency: EUR
    models:
      - provider: openai
        model: glm-5.3
        input: "0.40"
        cachedInput: "0.10"
        output: "1.60"
      - model: local-model
        input: "0"
        cachedInput: "0"
        output: "0.000001"
    sandbox:
      cpuCoreHour: "0.04"
      gibHour: "0.005"
"#;

#[test]
fn without_the_section_the_ledger_keeps_24_months_and_nothing_is_priced() {
    let valid = load(MINIMAL, &env()).unwrap();
    assert_eq!(valid.config.usage.retention_months, 24);
    assert_eq!(DEFAULT_USAGE_RETENTION_MONTHS, 24);
    assert!(valid.config.usage.prices.is_none());
    // an empty section is the same
    let valid = load(&format!("{MINIMAL}usage: {{}}\n"), &env()).unwrap();
    assert_eq!(valid.config.usage.retention_months, 24);
}

#[test]
fn a_full_section_is_read() {
    let valid = load(&with(FULL), &env()).unwrap();
    let usage = &valid.config.usage;
    assert_eq!(usage.retention_months, 12);
    let prices = usage.prices.as_ref().unwrap();
    assert_eq!(prices.currency, "EUR");
    assert_eq!(prices.models.len(), 2);
    assert_eq!(prices.models[0].provider.as_deref(), Some("openai"));
    assert_eq!(prices.models[0].model, "glm-5.3");
    assert_eq!(parse_price(&prices.models[0].input), Ok(400_000));
    assert_eq!(parse_price(&prices.models[0].cached_input), Ok(100_000));
    assert_eq!(parse_price(&prices.models[0].output), Ok(1_600_000));
    assert_eq!(prices.models[1].provider, None);
    assert_eq!(parse_price(&prices.models[1].output), Ok(1));
    let sandbox = prices.sandbox.as_ref().unwrap();
    assert_eq!(parse_price(&sandbox.cpu_core_hour), Ok(40_000));
    assert_eq!(parse_price(&sandbox.gib_hour), Ok(5_000));
}

#[test]
fn prices_may_name_no_models_and_no_sandbox() {
    let valid = load(&with("  prices: { currency: USD }\n"), &env()).unwrap();
    let prices = valid.config.usage.prices.unwrap();
    assert!(prices.models.is_empty() && prices.sandbox.is_none());
}

#[test]
fn the_effective_configuration_shows_the_section_without_a_float_in_it() {
    let valid = load(&with(FULL), &env()).unwrap();
    let json = serde_json::to_value(valid.config.effective()).unwrap();
    assert_eq!(json["usage"]["retentionMonths"], 12);
    assert_eq!(json["usage"]["prices"]["models"][0]["input"], "0.40");
    assert_eq!(json["usage"]["prices"]["sandbox"]["gibHour"], "0.005");
    let yaml = serde_norway::to_string(&valid.config.effective()).unwrap();
    assert!(
        yaml.contains("input: '0.40'") || yaml.contains("input: \"0.40\""),
        "{yaml}"
    );
}

// ---- the parser ------------------------------------------------------------------------------

#[test]
fn a_price_is_read_exactly_into_millionths() {
    for (text, micros) in [
        ("0", 0),
        ("0.0", 0),
        ("000", 0),
        ("1", 1_000_000),
        ("0.4", 400_000),
        ("0.40", 400_000),
        ("0.400000", 400_000),
        ("0.000001", 1),
        ("0.000010", 10),
        ("12.345678", 12_345_678),
        ("00012.5", 12_500_000),
        ("1000000", MAX_PRICE_MICROS),
        ("1000000.000000", MAX_PRICE_MICROS),
        ("999999.999999", MAX_PRICE_MICROS - 1),
    ] {
        assert_eq!(parse_price(text), Ok(micros), "{text}");
    }
    assert_eq!(MAX_PRICE_MICROS, 1_000_000_000_000);
}

#[test]
fn what_is_not_a_decimal_string_is_malformed() {
    for text in [
        "",
        " ",
        "-1",
        "+1",
        "-0",
        "1e3",
        "1E3",
        "1.",
        ".5",
        ".",
        "..",
        "1..2",
        "1.2.3",
        "1,5",
        "1 ",
        " 1",
        "1 0",
        "0x10",
        "1_000",
        "NaN",
        "inf",
        "abc",
        "\u{ff10}",
        "1\u{0660}",
        "\u{b7}5",
        "1.5\n",
        "\n1.5",
        "\u{2212}1",
        "1/2",
    ] {
        assert_eq!(parse_price(text), Err(PriceError::Malformed), "{text:?}");
    }
    // long before it is read
    assert_eq!(parse_price(&"1".repeat(33)), Err(PriceError::Malformed));
}

#[test]
fn more_than_six_decimals_and_more_than_the_bound_have_their_own_reasons() {
    assert_eq!(parse_price("0.0000001"), Err(PriceError::TooManyDecimals));
    assert_eq!(
        parse_price("0.1234567"),
        Err(PriceError::TooManyDecimals),
        "not rounded, not cut"
    );
    assert_eq!(parse_price("1000000.000001"), Err(PriceError::TooHigh));
    assert_eq!(parse_price("1000001"), Err(PriceError::TooHigh));
    assert_eq!(
        parse_price("99999999999999999999"),
        Err(PriceError::TooHigh)
    );
    assert_eq!(
        parse_price(&"9".repeat(32)),
        Err(PriceError::TooHigh),
        "a run of digits cannot overflow"
    );
}

// ---- the refusals ------------------------------------------------------------------------------

fn only(text: &str) -> String {
    let mut found = lines(load(text, &env()));
    assert_eq!(found.len(), 1, "{found:?}");
    found.remove(0)
}

#[test]
fn a_retention_out_of_its_bounds_is_refused_by_the_shape() {
    for bad in ["0", "1201", "-1", "100000"] {
        let line = only(&with(&format!("  retentionMonths: {bad}\n")));
        assert!(line.starts_with("usage.retentionMonths: "), "{line}");
    }
    let line = only(&with("  retentionMonths: \"24\"\n"));
    assert!(line.starts_with("usage.retentionMonths: "), "{line}");
    let line = only(&with("  retentionMonths: 1.5\n"));
    assert!(line.starts_with("usage.retentionMonths: "), "{line}");
    // the ends are valid
    for ok in [1, 1200] {
        load(&with(&format!("  retentionMonths: {ok}\n")), &env()).unwrap();
    }
}

#[test]
fn a_price_that_is_a_yaml_number_is_a_type_error_not_a_float() {
    for number in ["0.40", "1", "0.1", "1e3"] {
        let text = with(&format!(
            "  prices:\n    currency: EUR\n    models:\n      - model: m\n        input: {number}\n        cachedInput: \"1\"\n        output: \"1\"\n"
        ));
        let line = only(&text);
        assert!(
            line.starts_with("usage.prices.models[0].input: ") && line.contains("string"),
            "{line}"
        );
    }
}

#[test]
fn every_price_is_checked_and_named_by_its_key_path() {
    let text = with(
        "  prices:
    currency: EUR
    models:
      - { provider: openai, model: a, input: \"0.40\", cachedInput: \"-1\", output: \"1.60\" }
      - { model: b, input: \"0.4000001\", cachedInput: \"1e2\", output: \"2000000\" }
      - { model: c, input: \"\", cachedInput: \" 1\", output: \"1,5\" }
    sandbox: { cpuCoreHour: \"x\", gibHour: \"0.0000001\" }
",
    );
    let found = lines(load(&text, &env()));
    let paths: Vec<&str> = found.iter().map(|l| l.split(':').next().unwrap()).collect();
    assert_eq!(
        paths,
        [
            "usage.prices.models[0].cachedInput",
            "usage.prices.models[1].cachedInput",
            "usage.prices.models[1].input",
            "usage.prices.models[1].output",
            "usage.prices.models[2].cachedInput",
            "usage.prices.models[2].input",
            "usage.prices.models[2].output",
            "usage.prices.sandbox.cpuCoreHour",
            "usage.prices.sandbox.gibHour",
        ],
        "{found:#?}"
    );
    assert!(found[2].contains("six decimals"), "{}", found[2]);
    assert!(found[3].contains("1000000"), "{}", found[3]);
    assert!(found[8].contains("six decimals"), "{}", found[8]);
}

#[test]
fn two_entries_for_one_provider_and_model_are_refused_and_the_first_is_named() {
    let entry = |provider: &str, model: &str| {
        format!(
            "      - {{ {provider}model: {model}, input: \"1\", cachedInput: \"1\", output: \"1\" }}\n"
        )
    };
    let dup = with(&format!(
        "  prices:\n    currency: EUR\n    models:\n{}{}{}",
        entry("provider: p, ", "m"),
        entry("", "m"),
        entry("provider: p, ", "m"),
    ));
    let line = only(&dup);
    assert!(line.starts_with("usage.prices.models[2]: "), "{line}");
    assert!(line.contains("usage.prices.models[0]"), "{line}");
    // the same model under another provider, or under none, is another entry
    let fine = with(&format!(
        "  prices:\n    currency: EUR\n    models:\n{}{}{}",
        entry("provider: p, ", "m"),
        entry("provider: q, ", "m"),
        entry("", "m"),
    ));
    load(&fine, &env()).unwrap();
}

#[test]
fn a_currency_is_three_upper_case_ascii_letters() {
    for bad in [
        "eur",
        "EU",
        "EURO",
        "E1R",
        "\"\"",
        "\"EU \"",
        "\"€€€\"",
        "1",
        "true",
    ] {
        let line = only(&with(&format!("  prices: {{ currency: {bad} }}\n")));
        assert!(line.starts_with("usage.prices.currency: "), "{bad}: {line}");
    }
    let line = only(&with("  prices: {}\n"));
    assert_eq!(line, "usage.prices.currency: required key is missing");
    for good in ["EUR", "USD", "XAF", "ZZZ"] {
        load(
            &with(&format!("  prices: {{ currency: {good} }}\n")),
            &env(),
        )
        .unwrap();
    }
}

#[test]
fn a_provider_and_a_model_are_names_the_ledger_can_hold() {
    let long = "m".repeat(129);
    for (what, bad) in [
        ("provider", "\"\""),
        ("provider", "\"  \""),
        ("provider", "\"a\\nb\""),
        ("model", "\"\""),
        ("model", "\"a\\tb\""),
        ("model", long.as_str()),
    ] {
        let (provider, model) = if what == "provider" {
            (bad, "ok")
        } else {
            ("ok", bad)
        };
        let line = only(&with(&format!(
            "  prices:\n    currency: EUR\n    models:\n      - {{ provider: {provider}, model: {model}, input: \"1\", cachedInput: \"1\", output: \"1\" }}\n"
        )));
        assert!(
            line.starts_with(&format!("usage.prices.models[0].{what}: ")),
            "{what} {bad}: {line}"
        );
    }
    let longest = "m".repeat(128);
    load(
        &with(&format!(
            "  prices:\n    currency: EUR\n    models:\n      - {{ model: {longest}, input: \"1\", cachedInput: \"1\", output: \"1\" }}\n"
        )),
        &env(),
    )
    .unwrap();
}

#[test]
fn a_price_entry_needs_all_three_prices_and_a_model() {
    for missing in ["model", "input", "cachedInput", "output"] {
        let all = [
            ("model", "m"),
            ("input", "\"1\""),
            ("cachedInput", "\"1\""),
            ("output", "\"1\""),
        ];
        let body: Vec<String> = all
            .iter()
            .filter(|(k, _)| *k != missing)
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();
        let line = only(&with(&format!(
            "  prices:\n    currency: EUR\n    models:\n      - {{ {} }}\n",
            body.join(", ")
        )));
        assert_eq!(
            line,
            format!("usage.prices.models[0].{missing}: required key is missing")
        );
    }
    for missing in ["cpuCoreHour", "gibHour"] {
        let other = if missing == "gibHour" {
            "cpuCoreHour"
        } else {
            "gibHour"
        };
        let line = only(&with(&format!(
            "  prices:\n    currency: EUR\n    sandbox: {{ {other}: \"1\" }}\n"
        )));
        assert_eq!(
            line,
            format!("usage.prices.sandbox.{missing}: required key is missing")
        );
    }
}

#[test]
fn unknown_keys_are_refused_at_every_level() {
    for text in [
        "  retention: 12\n",
        "  prices: { currency: EUR, extra: 1 }\n",
        "  prices: { currency: EUR, models: [{ model: m, input: \"1\", cachedInput: \"1\", output: \"1\", cache: \"1\" }] }\n",
        "  prices: { currency: EUR, sandbox: { cpuCoreHour: \"1\", gibHour: \"1\", gpuHour: \"1\" } }\n",
    ] {
        let line = only(&with(text));
        assert!(line.contains("unknown"), "{line}");
    }
}

#[test]
fn at_most_256_models() {
    let entry = |i: usize| {
        format!("      - {{ model: m{i}, input: \"1\", cachedInput: \"1\", output: \"1\" }}\n")
    };
    let many = |n: usize| {
        with(&format!(
            "  prices:\n    currency: EUR\n    models:\n{}",
            (0..n).map(entry).collect::<String>()
        ))
    };
    load(&many(256), &env()).unwrap();
    let line = only(&many(257));
    assert!(line.starts_with("usage.prices.models: "), "{line}");
}

#[test]
fn no_refusal_quotes_what_it_refuses() {
    const SECRET: &str = "S3CR3T-VALUE-THAT-MUST-NOT-LEAK";
    for text in [
        // a secret where a price goes, as text, as a number and as a list
        format!(
            "usage:\n  prices:\n    currency: {SECRET}\n    models:\n      - {{ provider: '{SECRET}\\n', model: '{SECRET}\\u0007', input: '{SECRET}', cachedInput: [{SECRET}], output: {SECRET} }}\n    sandbox: {{ cpuCoreHour: '{SECRET}', gibHour: '1.{SECRET}' }}\n"
        ),
        format!(
            "usage:\n  retentionMonths: {SECRET}\n  prices: {{ currency: EUR, sandbox: {{ cpuCoreHour: [{SECRET}], gibHour: {{ a: {SECRET} }} }} }}\n"
        ),
        format!(
            "usage:\n  prices:\n    currency: EUR\n    models:\n      - {{ model: '{SECRET}', input: '1', cachedInput: '1', output: '1' }}\n      - {{ model: '{SECRET}', input: '1', cachedInput: '1', output: '1' }}\n"
        ),
    ] {
        let text = format!("{MINIMAL}{text}");
        let errors = load(&text, &env()).unwrap_err();
        assert!(!errors.is_empty());
        let shown = format!("{} {errors:?}", render(&errors));
        assert!(
            !shown.contains(SECRET),
            "a value reached an error:\n{shown}"
        );
    }
}
