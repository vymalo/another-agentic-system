//! Loads `docs/api/chat-api.yaml` and validates real responses against its schemas.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeSet;

use serde_json::{Value, json};

pub struct Contract {
    doc: Value,
}

/// Rewrites OpenAPI 3.0 style `{type: T, nullable: true}` into `{type: [T, "null"]}` so a
/// future 3.0-flavoured edit of the contract cannot silently weaken validation.
pub fn normalize_nullable(v: &mut Value) {
    match v {
        Value::Object(map) => {
            if map.get("nullable") == Some(&Value::Bool(true))
                && let Some(t) = map.get("type").cloned()
            {
                map.remove("nullable");
                map.insert("type".to_owned(), json!([t, "null"]));
            }
            for child in map.values_mut() {
                normalize_nullable(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_nullable),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

impl Contract {
    pub fn load() -> Self {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/api/chat-api.yaml"
        );
        let text = std::fs::read_to_string(path).expect("contract file");
        let mut doc: Value = serde_norway::from_str(&text).expect("contract is valid YAML");
        normalize_nullable(&mut doc);
        Contract { doc }
    }

    /// Every `(operationId, path, method)` of the contract.
    pub fn operations(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for (path, item) in self.doc["paths"].as_object().unwrap() {
            for (method, op) in item.as_object().unwrap() {
                if let Some(id) = op.get("operationId").and_then(Value::as_str) {
                    out.push((id.to_owned(), path.clone(), method.clone()));
                }
            }
        }
        out
    }

    pub fn operation_ids(&self) -> BTreeSet<String> {
        self.operations().into_iter().map(|(id, _, _)| id).collect()
    }

    fn resolve<'a>(&'a self, node: &'a Value) -> &'a Value {
        match node.get("$ref").and_then(Value::as_str) {
            Some(r) => {
                let pointer = r.strip_prefix('#').expect("local refs only");
                self.doc.pointer(pointer).expect("dangling $ref")
            }
            None => node,
        }
    }

    /// The declared response of `operation` for `status`, as `(content type, schema)`.
    /// `Some(None)` means the status is documented without a body schema.
    pub fn response(&self, operation: &str, status: u16) -> Option<Option<(String, Value)>> {
        let (_, path, method) = self
            .operations()
            .into_iter()
            .find(|(id, _, _)| id == operation)
            .unwrap_or_else(|| panic!("unknown operation {operation}"));
        let responses = &self.doc["paths"][&path][&method]["responses"];
        let declared = responses.get(status.to_string())?;
        let declared = self.resolve(declared);
        let content = declared.get("content").and_then(Value::as_object);
        Some(content.and_then(|c| {
            c.iter().next().map(|(ct, media)| {
                (
                    ct.clone(),
                    media.get("schema").cloned().unwrap_or_else(|| json!({})),
                )
            })
        }))
    }

    pub fn problem_schema(&self) -> Value {
        let (_, schema) = {
            let r = &self.doc["components"]["responses"]["Problem"]["content"]["application/problem+json"];
            ("application/problem+json", r["schema"].clone())
        };
        schema
    }

    /// Validates `instance` against `schema`, resolving `#/components/...` in the contract.
    pub fn validate(&self, schema: &Value, instance: &Value) {
        let mut root = schema.clone();
        let obj = root.as_object_mut().expect("schema object");
        obj.insert(
            "$schema".to_owned(),
            json!("https://json-schema.org/draft/2020-12/schema"),
        );
        obj.insert("components".to_owned(), self.doc["components"].clone());
        let validator = jsonschema::draft202012::options()
            .should_validate_formats(true)
            .build(&root)
            .expect("schema compiles");
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|e| format!("{e} at {}", e.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "instance does not match the contract schema:\n{}\ninstance: {instance}",
            errors.join("\n")
        );
    }

    /// Validates against a named component schema, e.g. `Event`.
    pub fn validate_component(&self, name: &str, instance: &Value) {
        self.validate(
            &json!({"$ref": format!("#/components/schemas/{name}")}),
            instance,
        );
    }
}
