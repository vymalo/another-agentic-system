//! The JSON Schema of the file, and pass 2: the tree is checked against it.
//!
//! The schema is generated from the types by `schemars` and committed at
//! `docs/api/config.schema.json`; a test fails when they differ. `jsonschema`'s `iter_errors`
//! yields every violation, not the first: an unknown key (`additionalProperties: false`), a wrong
//! type, a missing required key, a plain string where a secret reference goes. Each is reported
//! by its instance path and its kind. **`jsonschema`'s own messages quote the instance and are
//! never used**: the instance may be a secret that was pasted by mistake.

use std::sync::OnceLock;

use jsonschema::Validator;
use jsonschema::error::{TypeKind, ValidationErrorKind};
use schemars::generate::SchemaSettings;
use serde_json::Value;

use crate::error::{ConfigError, ErrorKind};
use crate::reserved::reserved;
use crate::tree::{child, display_key};
use crate::types::Config;

/// The JSON Schema of [`Config`], draft 2020-12.
///
/// An optional key is simply optional (no `null`): a key with nothing after it is a type error.
#[must_use]
pub fn schema() -> Value {
    let generator = SchemaSettings::draft2020_12().into_generator();
    let mut schema =
        serde_json::to_value(generator.into_root_schema_for::<Config>()).unwrap_or(Value::Null);
    tidy(&mut schema);
    schema
}

/// `Option<T>` is `T` or `null` in the generated schema. A key that is optional is left out; one
/// that is written with nothing after it is a type error, so `null` goes. Descriptions are the
/// doc comments, put on one line.
fn tidy(node: &mut Value) {
    match node {
        Value::Object(object) => {
            let is_null =
                |branch: &Value| branch.get("type").and_then(Value::as_str) == Some("null");
            if let Some(Value::Array(branches)) = object.get("anyOf")
                && branches.len() == 2
                && let Some(kept) = branches.iter().find(|b| !is_null(b)).cloned()
                && branches.iter().any(is_null)
            {
                object.remove("anyOf");
                if let Value::Object(kept) = kept {
                    object.extend(kept);
                }
            }
            // `null` is a value of a few keys (`auth.defaultRole`), which say so.
            let null_is_a_value = object.contains_key("x-null-is-a-value");
            if !null_is_a_value
                && let Some(Value::Array(types)) = object.get("type")
                && types.len() == 2
                && types.iter().any(|t| t.as_str() == Some("null"))
                && let Some(kept) = types.iter().find(|t| t.as_str() != Some("null")).cloned()
            {
                object.insert("type".to_owned(), kept);
            }
            // A unit enum with documented variants is a `oneOf` of constants; an `enum` says the
            // same, and `jsonschema` reports a value outside it with the allowed ones.
            if let Some(Value::Array(branches)) = object.get("oneOf")
                && !branches.is_empty()
                && branches
                    .iter()
                    .all(|b| b.get("const").is_some_and(Value::is_string))
            {
                let allowed: Vec<Value> = branches
                    .iter()
                    .filter_map(|b| b.get("const").cloned())
                    .collect();
                object.remove("oneOf");
                object.insert("type".to_owned(), Value::String("string".to_owned()));
                object.insert("enum".to_owned(), Value::Array(allowed));
            }
            // A description is a doc comment, hard-wrapped in the source: one line here.
            if let Some(Value::String(description)) = object.get_mut("description") {
                *description = description.split_whitespace().collect::<Vec<_>>().join(" ");
            }
            object.values_mut().for_each(tidy);
        }
        Value::Array(items) => items.iter_mut().for_each(tidy),
        _ => {}
    }
}

/// [`schema`] as the text committed at `docs/api/config.schema.json`: pretty-printed, with a
/// final newline.
#[must_use]
pub fn schema_text() -> String {
    let mut text = serde_json::to_string_pretty(&schema()).unwrap_or_default();
    text.push('\n');
    text
}

fn validator() -> Option<&'static Validator> {
    static VALIDATOR: OnceLock<Option<Validator>> = OnceLock::new();
    VALIDATOR
        .get_or_init(|| jsonschema::validator_for(&schema()).ok())
        .as_ref()
}

/// Pass 2: checks `tree` against the schema, listing every shape error, and reads it into the
/// types when there is none.
///
/// # Errors
///
/// Every shape error, sorted by key path.
pub fn check(tree: &Value) -> Result<Config, Vec<ConfigError>> {
    if !tree.is_object() {
        return Err(vec![ConfigError::new("", ErrorKind::NotAMapping)]);
    }
    let Some(validator) = validator() else {
        return Err(vec![bug("the schema of this build cannot be compiled")]);
    };
    let mut errors: Vec<ConfigError> = validator
        .iter_errors(tree)
        .flat_map(|e| {
            map_error(
                &e.instance_path().to_string(),
                &e.schema_path().to_string(),
                e.kind(),
            )
        })
        .collect();
    errors.sort();
    errors.dedup();
    if !errors.is_empty() {
        return Err(errors);
    }
    // After a clean schema pass this cannot fail, short of a drift the schema test catches. The
    // serde message is never shown: it quotes the value.
    serde_json::from_value(tree.clone())
        .map_err(|_| vec![bug("the schema and the types of this build disagree")])
}

fn bug(what: &str) -> ConfigError {
    ConfigError::invalid("", format!("{what} (a bug in this build)"))
}

/// The segments of a JSON pointer (`/a/b/0`), unescaped.
fn segments(pointer: &str) -> Vec<String> {
    pointer
        .split('/')
        .skip(1)
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect()
}

/// The key path of a JSON pointer: `server.surfaces[1]`.
fn path_of(pointer: &str) -> String {
    let mut path = String::new();
    for segment in segments(pointer) {
        if !segment.is_empty() && segment.bytes().all(|b| b.is_ascii_digit()) {
            path.push_str(&format!("[{segment}]"));
        } else {
            path = child(&path, &segment);
        }
    }
    path
}

/// One violation as the errors it stands for. The schema path says which rule failed; nothing
/// of the instance is read.
fn map_error(instance: &str, schema_path: &str, kind: &ValidationErrorKind) -> Vec<ConfigError> {
    let path = path_of(instance);
    // `version` has one message whatever is wrong with it, and when it is missing.
    let at_version = path == "version";
    if let ValidationErrorKind::Required { property } = kind
        && path.is_empty()
        && property.as_str() == Some("version")
    {
        return vec![ConfigError::new("version", ErrorKind::Version)];
    }
    if at_version {
        return vec![ConfigError::new("version", ErrorKind::Version)];
    }
    // Likewise a prompt that is not `{ inline }` or `{ file }`.
    if schema_path.starts_with("/$defs/Prompt") {
        return vec![ConfigError::new(path, ErrorKind::NotAPrompt)];
    }
    // Anything the `SecretRef` definition refuses (a string, a number, a mapping with another
    // key) is "a secret is a reference".
    if schema_path.starts_with("/$defs/SecretRef") {
        return vec![ConfigError::new(path, ErrorKind::NotASecretRef)];
    }
    let one = |kind: ErrorKind| vec![ConfigError::new(path.clone(), kind)];
    match kind {
        ValidationErrorKind::AdditionalProperties { unexpected } => unexpected
            .iter()
            .map(|name| {
                let at = child(&path, name);
                match reserved(&at) {
                    Some(r) => ConfigError::new(at, ErrorKind::Reserved { by: r.by }),
                    None => ConfigError::new(at, ErrorKind::Unknown),
                }
            })
            .collect(),
        ValidationErrorKind::Required { property } => {
            let name = property.as_str().map(display_key).unwrap_or_default();
            vec![ConfigError::new(
                if path.is_empty() {
                    name
                } else {
                    format!("{path}.{name}")
                },
                ErrorKind::Missing,
            )]
        }
        ValidationErrorKind::Type { kind } => one(ErrorKind::WrongType {
            expected: match kind {
                TypeKind::Single(t) => format!("{t:?}").to_lowercase(),
                _ => "another type".to_owned(),
            },
        }),
        ValidationErrorKind::Enum { options } => one(ErrorKind::NotAllowed {
            allowed: options
                .as_array()
                .map(|all| {
                    all.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default(),
        }),
        ValidationErrorKind::Minimum { limit } => one(ErrorKind::TooSmall {
            min: limit.to_string(),
        }),
        ValidationErrorKind::Maximum { limit } => one(ErrorKind::TooLarge {
            max: limit.to_string(),
        }),
        ValidationErrorKind::MinItems { limit } => one(ErrorKind::TooFewItems { min: *limit }),
        ValidationErrorKind::MaxItems { limit } => one(ErrorKind::TooManyItems { max: *limit }),
        ValidationErrorKind::MinLength { limit: 1 } => one(ErrorKind::Empty),
        _ => one(ErrorKind::Schema {
            keyword: schema_path
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned(),
        }),
    }
}
