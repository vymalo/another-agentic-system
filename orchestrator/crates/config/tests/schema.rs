//! The generated JSON Schema is the committed one (`docs/api/config.schema.json`), and the
//! secrets are exactly the fields the contract names.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::PathBuf;

use serde_json::Value;

fn committed() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/config.schema.json")
}

/// Compares the schema generated from the types with the committed file, or rewrites the file
/// when `UPDATE_SCHEMA=1` (the way `UPDATE_GOLDEN=1` rewrites the goldens).
#[test]
fn the_committed_schema_is_the_one_the_types_generate() {
    let generated = orch_config::schema_text();
    let path = committed();
    if std::env::var("UPDATE_SCHEMA").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        on_disk == generated,
        "docs/api/config.schema.json is out of date; run \
         `UPDATE_SCHEMA=1 cargo test -p orch-config --test schema` and review the diff"
    );
}

/// Every property of the schema that is a `SecretRef` (or a list of them): `Type.property`.
fn secret_fields(schema: &Value) -> Vec<String> {
    let is_secret = |v: &Value| {
        v.get("$ref").and_then(Value::as_str) == Some("#/$defs/SecretRef")
            || v.get("items")
                .and_then(|i| i.get("$ref"))
                .and_then(Value::as_str)
                == Some("#/$defs/SecretRef")
    };
    let mut found = Vec::new();
    for (type_name, def) in schema["$defs"].as_object().unwrap() {
        if let Some(properties) = def.get("properties").and_then(Value::as_object) {
            for (name, property) in properties {
                if is_secret(property) {
                    found.push(format!("{type_name}.{name}"));
                }
            }
        }
    }
    found.sort();
    found
}

/// ADR 0034: "the secrets are the eight values of today that are secrets" and, with ADR 0032, the
/// two credentials of the S3 store. A eleventh, or a string where one of these is, would be a change
/// of the contract, so it fails here first.
#[test]
fn the_secrets_are_the_ten_the_contract_names() {
    assert_eq!(
        secret_fields(&orch_config::schema()),
        [
            "ArtifactsS3.accessKeyId",
            "ArtifactsS3.secretAccessKey",
            "Database.url",
            "Endpoint.apiKey",
            "Registry.agentToken",
            "Registry.token",
            "ThreadTools.previousSecret",
            "ThreadTools.secret",
            "WebhookGeneric.secrets",
            "WebhookGithub.secrets",
        ]
    );
}

#[test]
fn the_schema_is_closed_and_has_no_null() {
    let text = orch_config::schema_text();
    assert!(text.ends_with("}\n"));
    assert!(
        !text.contains("\"null\""),
        "an optional key is optional, not nullable"
    );
    let schema = orch_config::schema();
    for (name, def) in schema["$defs"].as_object().unwrap() {
        if def.get("properties").is_some() {
            assert_eq!(
                def["additionalProperties"],
                Value::Bool(false),
                "{name} must refuse unknown keys"
            );
        }
    }
    assert_eq!(schema["additionalProperties"], Value::Bool(false));
}
