//! RFC 8785 for the values a prompt holds: members sorted by UTF-16 code units, strings escaped as §3.2.2.2 says, and
//! numbers as ECMAScript prints them (the samples of the RFC's appendix B).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_prompt_statement::{canonical, hash_of, sha256_b64u};
use serde_json::{Number, Value, json};

fn jcs(value: &Value) -> String {
    String::from_utf8(canonical(value)).unwrap()
}

#[test]
fn members_are_sorted_by_utf16_code_units_not_code_points() {
    // U+1F600 is the surrogate pair D83D DE00, which sorts before U+FF5E by UTF-16 and after it by code point
    let value = json!({ "\u{ff5e}": 1, "\u{1f600}": 2, "b": 3, "a": 4, "": 5 });
    assert_eq!(
        jcs(&value),
        "{\"\":5,\"a\":4,\"b\":3,\"\u{1f600}\":2,\"\u{ff5e}\":1}"
    );
}

#[test]
fn strings_escape_only_what_the_rfc_says() {
    let value = json!("\"\\/\u{8}\t\n\u{c}\r\u{1}\u{1f}\u{7f}é€😀");
    assert_eq!(
        jcs(&value),
        "\"\\\"\\\\/\\b\\t\\n\\f\\r\\u0001\\u001f\u{7f}é€😀\""
    );
}

#[test]
fn nesting_and_literals() {
    let value = json!({ "z": [true, false, null, [], {}], "y": { "b": [1, "two"], "a": null } });
    assert_eq!(
        jcs(&value),
        r#"{"y":{"a":null,"b":[1,"two"]},"z":[true,false,null,[],{}]}"#
    );
}

#[test]
fn numbers_print_as_ecmascript_prints_them() {
    // RFC 8785 appendix B, by IEEE 754 bit pattern
    let samples: [(u64, &str); 11] = [
        (0x0000_0000_0000_0000, "0"),
        (0x8000_0000_0000_0000, "0"),
        (0x0000_0000_0000_0001, "5e-324"),
        (0x7fef_ffff_ffff_ffff, "1.7976931348623157e+308"),
        (0x4340_0000_0000_0000, "9007199254740992"),
        (0xc340_0000_0000_0000, "-9007199254740992"),
        (0x4430_0000_0000_0000, "295147905179352830000"),
        (0x44b5_2d02_c7e1_4af5, "9.999999999999997e+22"),
        (0x44b5_2d02_c7e1_4af6, "1e+23"),
        (0x3eb0_c6f7_a0b5_ed8d, "0.000001"),
        (0x3eb0_c6f7_a0b5_ed8c, "9.999999999999997e-7"),
    ];
    for (bits, expected) in samples {
        let number = Number::from_f64(f64::from_bits(bits)).unwrap();
        assert_eq!(jcs(&Value::Number(number)), expected, "{bits:#x}");
    }
}

#[test]
fn numbers_as_parsed_from_a_request() {
    let value: Value = serde_json::from_str(
        r#"[12, -3, 0.25, 100.0, 1.5e3, -0.0, 1e21, 1e-7, 9007199254740993, 18446744073709551615]"#,
    )
    .unwrap();
    assert_eq!(
        jcs(&value),
        "[12,-3,0.25,100,1500,0,1e+21,1e-7,9007199254740992,18446744073709552000]"
    );
}

#[test]
fn a_hash_is_sha256_of_the_canonical_bytes_in_base64url_without_padding() {
    // SHA-256 of "{}"
    assert_eq!(
        sha256_b64u(b"{}"),
        "RBNvo1WzZ4oRRq0W9-hknpT7T8If536DEMBg9hyq_4o"
    );
    assert_eq!(
        hash_of(&json!({ "b": 1, "a": 2 })),
        sha256_b64u(br#"{"a":2,"b":1}"#)
    );
}
