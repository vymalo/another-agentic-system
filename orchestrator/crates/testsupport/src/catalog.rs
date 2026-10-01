//! The UI catalog as the web sends it, for tests (ADR 0023): `forwardedProps["vymalo.uiCatalog"]`.

use orch_core::catalog_digest;
use serde_json::{Value, json};

/// The id of the catalog the tests send; a UI keeps it across versions.
pub const UI_CATALOG_ID: &str = "https://agents.vymalo.com/a2ui/catalogs/chat";

/// Version `version` of the web's catalog, as it sends it: `{catalogId, version, digest,
/// catalog}`, with the real digest. Version 1 has `Text`, later versions add `Column`; every
/// version differs from the others, so each has a digest of its own.
pub fn ui_catalog(version: u32) -> Value {
    let mut components = json!({
        "Text": {
            "type": "object",
            "description": format!("Plain text (catalog version {version})."),
            "properties": {
                "id": {"type": "string", "minLength": 1, "maxLength": 256},
                "component": {"const": "Text"},
                "text": {"type": "string", "minLength": 1, "maxLength": 4000},
                "variant": {"enum": ["h1", "h2", "h3", "h4", "h5", "caption", "body"]},
            },
            "required": ["id", "component", "text"],
            "additionalProperties": false,
        },
    });
    if version >= 2 {
        components["Column"] = json!({
            "type": "object",
            "description": "Stacks its children vertically.",
            "properties": {
                "id": {"type": "string", "minLength": 1, "maxLength": 256},
                "component": {"const": "Column"},
                "children": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 50,
                    "items": {"type": "string", "minLength": 1, "maxLength": 256},
                },
                "align": {"enum": ["start", "center", "end", "stretch"]},
            },
            "required": ["id", "component", "children"],
            "additionalProperties": false,
        });
    }
    let catalog = json!({"catalogId": UI_CATALOG_ID, "components": components});
    json!({
        "catalogId": UI_CATALOG_ID,
        "version": version,
        "digest": catalog_digest(&catalog).expect("a catalog the web could send"),
        "catalog": catalog,
    })
}

/// The `extra` of [`Chat::agui_input`](crate::Chat::agui_input) that makes a run carry version
/// `version` of the catalog.
pub fn with_ui_catalog(version: u32) -> Value {
    json!({"forwardedProps": {"vymalo.uiCatalog": ui_catalog(version)}})
}

/// `value` with every number that is a whole number written as an integer.
///
/// An A2A server holds the numbers of a message's metadata as doubles (they are a protobuf
/// `Struct`), so a catalog that arrived inline reads `maxLength: 256.0`. RFC 8785 writes `256.0` as
/// `256`, and so must whoever recomputes the digest of such a catalog: after this, the digest of what
/// the agent received is the digest the orchestrator sent.
pub fn integral_numbers(value: &Value) -> Value {
    match value {
        Value::Number(n) => match n.as_f64() {
            Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 => {
                Value::from(f as i64)
            }
            Some(_) | None => value.clone(),
        },
        Value::Array(items) => Value::Array(items.iter().map(integral_numbers).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), integral_numbers(v)))
                .collect(),
        ),
        Value::Null | Value::Bool(_) | Value::String(_) => value.clone(),
    }
}
