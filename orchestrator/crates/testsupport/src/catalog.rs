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
