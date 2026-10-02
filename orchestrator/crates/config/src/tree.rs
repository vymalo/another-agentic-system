//! Pass 1: the text becomes a tree, or a syntax error, alone.
//!
//! `serde_norway` reads the YAML (anchors and aliases are resolved by the parser), merge keys
//! (`<<:`) are applied, a tag is refused, a key that is repeated in one mapping is refused (the
//! parser says so; "the last one wins" is never the reading), and the tree is written as a JSON
//! value so that the shape pass can check it against the JSON Schema and the overlay of the
//! binary can set keys in it.
//!
//! A syntax error is reported by its line and column only: the parser's own message may quote
//! the text, and the text may hold a secret that was pasted by mistake.

use serde_json::{Map, Number, Value};
use serde_norway::Value as Yaml;

use crate::error::{ConfigError, ErrorKind, where_detail};

/// Reads `text` as YAML into a tree: a JSON value whose objects are the file's mappings.
///
/// # Errors
///
/// A syntax error is returned alone. Otherwise every tag, every key that is not a string and
/// every number that is not finite is listed, each with its key path.
pub fn parse_yaml(text: &str) -> Result<Value, Vec<ConfigError>> {
    let mut yaml: Yaml = serde_norway::from_str(text).map_err(|e| vec![syntax_error(&e)])?;
    yaml.apply_merge().map_err(|_| {
        vec![ConfigError::invalid(
            "",
            "a merge key (`<<`) needs a mapping, or a list of mappings, to merge",
        )]
    })?;
    let mut errors = Vec::new();
    let tree = convert(&yaml, "", &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(tree)
}

/// A parse error as a kind that carries a place and never the text.
fn syntax_error(e: &serde_norway::Error) -> ConfigError {
    let (line, column) = e
        .location()
        .map_or((0, 0), |location| (location.line(), location.column()));
    // The parser says `<path>: duplicate entry with key "<key>" at line L column C` and nothing
    // else says "duplicate entry with key", so the one message that is read is read by those
    // words, and only a plain name is repeated back, never anything else of the text.
    let message = e.to_string();
    if let Some((before, after)) = message.split_once("duplicate entry with key ") {
        let name = after
            .strip_prefix('"')
            .and_then(|rest| rest.split('"').next())
            .filter(|key| is_plain_name(key));
        let path = before
            .strip_suffix(": ")
            .filter(|path| {
                path.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '[' | ']'))
            })
            .unwrap_or_default();
        return ConfigError::new(
            path,
            ErrorKind::DuplicateKey {
                detail: where_detail(name, line, column),
            },
        );
    }
    ConfigError::new("", ErrorKind::Syntax { line, column })
}

/// A key written as an identifier: safe to print.
fn is_plain_name(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// How a key is written in a path: as it is when it is a plain name, else quoted and cut.
pub(crate) fn display_key(key: &str) -> String {
    if is_plain_name(key) {
        key.to_owned()
    } else {
        let cut: String = key.chars().take(32).collect();
        format!("{cut:?}")
    }
}

/// The path of `key` under `parent`.
pub(crate) fn child(parent: &str, key: &str) -> String {
    if parent.is_empty() {
        display_key(key)
    } else {
        format!("{parent}.{}", display_key(key))
    }
}

fn convert(yaml: &Yaml, path: &str, errors: &mut Vec<ConfigError>) -> Value {
    match yaml {
        Yaml::Null => Value::Null,
        Yaml::Bool(b) => Value::Bool(*b),
        Yaml::Number(n) => {
            if let Some(u) = n.as_u64() {
                Value::Number(u.into())
            } else if let Some(i) = n.as_i64() {
                Value::Number(i.into())
            } else if let Some(f) = n.as_f64().and_then(Number::from_f64) {
                Value::Number(f)
            } else {
                errors.push(ConfigError::new(path, ErrorKind::BadNumber));
                Value::Null
            }
        }
        Yaml::String(s) => Value::String(s.clone()),
        Yaml::Sequence(items) => Value::Array(
            items
                .iter()
                .enumerate()
                .map(|(i, item)| convert(item, &format!("{path}[{i}]"), errors))
                .collect(),
        ),
        Yaml::Mapping(map) => {
            let mut object = Map::new();
            for (key, value) in map {
                match key {
                    Yaml::String(key) => {
                        let at = child(path, key);
                        object.insert(key.clone(), convert(value, &at, errors));
                    }
                    _ => errors.push(ConfigError::new(path, ErrorKind::NonStringKey)),
                }
            }
            Value::Object(object)
        }
        Yaml::Tagged(tagged) => {
            errors.push(ConfigError::new(path, ErrorKind::Tag));
            convert(&tagged.value, path, errors)
        }
    }
}
