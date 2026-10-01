//! The UI catalog's schema check (ADR 0023).
//!
//! `orch-core` checks the envelope of a catalog the web sends: the shape, the limits, the
//! keywords that point outside a schema, and the digest. It has no schema library, so it cannot
//! say whether a component's schema **is** JSON Schema; that is this check, which the surfaces
//! (and [`App::submit`](crate::App::submit)) make before anything is stored: every component schema
//! must be a valid JSON Schema (draft 2020-12), and must describe the component it is the entry
//! of, with `properties.component.const` equal to its name. The orchestrator does not interpret
//! the schemas beyond that; it relays them.
//!
//! Nothing is fetched: a catalog has no `$ref` or `$id` (the core refuses them), and this crate
//! builds `jsonschema` without its resolvers, so a schema cannot make the orchestrator open a URL.

use orch_core::{Classify, ErrorClass, UiCatalogData};
use serde_json::Value;

/// The key of the run's `forwardedProps` that carries the screen's catalog (AG-UI, ADR 0023).
pub const THREAD_UI_CATALOG_KEY: &str = "vymalo.uiCatalog";

/// The longest reason of a schema error that is reported back: the error of a large schema can
/// quote much of it.
const MAX_REASON_CHARS: usize = 300;

/// Why a component schema of a catalog was refused. The sender's mistake, a 400 at a surface.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CatalogSchemaError {
    /// The catalog has no `components` object (the core's envelope check refuses this first).
    #[error("components must be an object")]
    Components,
    /// A component's schema is not valid JSON Schema (draft 2020-12).
    #[error(
        "the schema of component '{component}' is not valid JSON Schema (draft 2020-12): {reason}"
    )]
    NotJsonSchema {
        /// The component.
        component: String,
        /// What `jsonschema` said, cut short.
        reason: String,
    },
    /// A component's schema does not say `properties.component.const` is the component's name.
    #[error(
        "the schema of component '{component}' must say properties.component.const = \"{component}\""
    )]
    NamesAnother {
        /// The component.
        component: String,
    },
}

impl Classify for CatalogSchemaError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Invalid
    }
}

/// Checks that every component schema of `catalog` is JSON Schema 2020-12 and says
/// `properties.component.const: "<its name>"`.
///
/// # Errors
///
/// A [`CatalogSchemaError`] naming the first component that fails and why.
pub fn check_catalog_schemas(catalog: &UiCatalogData) -> Result<(), CatalogSchemaError> {
    let Some(components) = catalog.catalog.get("components").and_then(Value::as_object) else {
        return Err(CatalogSchemaError::Components);
    };
    for (name, schema) in components {
        if let Err(e) = jsonschema::draft202012::new(schema) {
            return Err(CatalogSchemaError::NotJsonSchema {
                component: name.clone(),
                reason: cut(&e.to_string()),
            });
        }
        let names_itself = schema
            .pointer("/properties/component/const")
            .and_then(Value::as_str)
            == Some(name.as_str());
        if !names_itself {
            return Err(CatalogSchemaError::NamesAnother {
                component: name.clone(),
            });
        }
    }
    Ok(())
}

fn cut(reason: &str) -> String {
    if reason.chars().count() <= MAX_REASON_CHARS {
        return reason.to_owned();
    }
    let kept: String = reason.chars().take(MAX_REASON_CHARS).collect();
    format!("{kept}…")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    const ID: &str = "https://agents.vymalo.com/a2ui/catalogs/chat";

    fn catalog(components: &Value) -> UiCatalogData {
        let catalog = json!({"catalogId": ID, "components": components});
        UiCatalogData {
            catalog_id: ID.to_owned(),
            version: 1,
            digest: orch_core::catalog_digest(&catalog).unwrap(),
            catalog,
        }
    }

    fn text() -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "minLength": 1},
                "component": {"const": "Text"},
                "text": {"type": "string", "maxLength": 4000},
                "variant": {"enum": ["h1", "body"]},
            },
            "required": ["id", "component", "text"],
            "additionalProperties": false,
        })
    }

    fn refused(components: &Value) -> CatalogSchemaError {
        check_catalog_schemas(&catalog(components)).unwrap_err()
    }

    #[test]
    fn a_catalog_of_valid_schemas_that_name_their_components_passes() {
        let ok = catalog(&json!({
            "Text": text(),
            "Column": {
                "type": "object",
                "properties": {
                    "component": {"const": "Column"},
                    "children": {"type": "array", "items": {"type": "string"}, "minItems": 1},
                },
                "required": ["component", "children"],
            },
        }));
        check_catalog_schemas(&ok).unwrap();
    }

    #[test]
    fn a_schema_that_is_not_json_schema_is_refused_with_the_component_named() {
        for bad in [
            json!({"type": "nonsense"}),
            json!({"type": "object", "properties": {"component": {"const": "Text"}}, "required": "id"}),
            json!({"type": "object", "properties": {"component": {"const": "Text"}, "n": {"minLength": -1}}}),
        ] {
            let error = refused(&json!({ "Text": bad }));
            assert!(
                matches!(&error, CatalogSchemaError::NotJsonSchema { component, .. } if component == "Text"),
                "{error:?}"
            );
            assert!(
                error.to_string().contains("not valid JSON Schema"),
                "{error}"
            );
        }
    }

    #[test]
    fn a_schema_must_name_the_component_it_belongs_to() {
        for schema in [
            json!({"type": "object"}),
            json!({"type": "object", "properties": {}}),
            json!({"type": "object", "properties": {"component": {}}}),
            json!({"type": "object", "properties": {"component": {"const": "Column"}}}),
            json!({"type": "object", "properties": {"component": {"enum": ["Text"]}}}),
            json!({"type": "object", "properties": {"component": {"const": 7}}}),
        ] {
            assert_eq!(
                refused(&json!({ "Text": schema })),
                CatalogSchemaError::NamesAnother {
                    component: "Text".to_owned()
                }
            );
        }
    }

    #[test]
    fn a_catalog_without_components_is_refused() {
        let mut broken = catalog(&json!({"Text": text()}));
        broken.catalog = json!({"catalogId": ID});
        assert_eq!(
            check_catalog_schemas(&broken),
            Err(CatalogSchemaError::Components)
        );
    }

    #[test]
    fn the_reason_that_comes_back_is_short() {
        let mut schema = text();
        schema["properties"]["text"]["pattern"] = json!(format!("({}", "a".repeat(5_000)));
        let error = refused(&json!({ "Text": schema }));
        assert!(error.to_string().chars().count() < 700, "{error}");
        assert_eq!(error.class(), ErrorClass::Invalid);
    }
}
