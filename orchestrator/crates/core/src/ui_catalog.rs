//! The UI catalog (ADR 0023): the components the person's screen can show, as the web defines
//! them, and what the orchestrator keeps of them for a thread.
//!
//! The catalog is an A2UI **inline catalog**, `{catalogId, components: {<Name>: <JSON Schema>}}`,
//! that the web sends with the first run of a thread and again when its own version is newer than
//! the thread's. The orchestrator **relays** it, it does not interpret it. What it checks, before
//! anything is stored, is the envelope ([`UiCatalogData::from_json`]): the shape, the limits, the
//! rules of the schema language we allow, and that the digest the sender claims is the digest of
//! what it sent. That the schemas are JSON Schema is checked by `orch-app`, which may use a
//! schema library; this crate stays pure.
//!
//! Three things follow:
//!
//! - [`UiCatalogData`] is the `ui_catalog` event: one per digest, appended first in the commit
//!   of the input that carried it;
//! - [`UiCatalogLedger`] is the thread's memory of them (`Job.catalog`): the digests seen, and
//!   which one is current, the highest version. [`UiCatalogLedger::accept`] is the one rule, and
//!   the AG-UI projection folds the same events with the same rule, so what the state snapshot
//!   says and what the core decided can not drift apart;
//! - [`UiDelivery`] is what an agent is sent: the catalog **inline** in the message that made it
//!   current, a **reference** (`catalogId`, `version`, `digest`) in every other. A delivery that
//!   was lost is healed by the agent asking for the catalog again (the thread-tools endpoint), so
//!   no "sent" bookkeeping exists.
//!
//! The digest is `sha256:` and the lowercase hex of SHA-256 over the UTF-8 of the **canonical**
//! JSON of the catalog: object keys sorted, no whitespace, strings escaped as `JSON.stringify` and
//! `serde_json` both do, integers in decimal (RFC 8785, restricted to what a catalog may hold:
//! keys are ASCII and numbers are integers, so that every language gets the same bytes without a
//! number-formatting rule).

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{Classify, ErrorClass};
use crate::ui::serialized_len;

/// The most bytes (serialised JSON) a catalog may hold.
pub const MAX_CATALOG_BYTES: usize = 64 * 1024;
/// The most components a catalog may hold.
pub const MAX_CATALOG_COMPONENTS: usize = 64;
/// How deep a catalog may nest (an object or an array inside another counts one).
pub const MAX_CATALOG_DEPTH: usize = 32;
/// The longest `catalogId`, in bytes.
pub const MAX_CATALOG_ID_BYTES: usize = 256;
/// The highest `version` a catalog may have.
pub const MAX_CATALOG_VERSION: u32 = 1_000_000;
/// How many digests a thread's ledger remembers; the oldest is forgotten first.
pub const MAX_SEEN_CATALOGS: usize = 32;
/// The largest integer a catalog may hold: the largest a JSON number holds exactly in JavaScript,
/// which is where catalogs are written.
const MAX_SAFE_INTEGER: i64 = (1 << 53) - 1;

/// Keywords that point outside the schema they are in, or name it. A catalog is self-contained
/// (ADR 0023): each component schema is read alone, with no resolution of references.
const FORBIDDEN_KEYWORDS: [&str; 5] = ["$ref", "$dynamicRef", "$id", "$anchor", "$schema"];

/// Why a catalog was refused. Every variant is the sender's mistake ([`ErrorClass::Invalid`]),
/// except [`CatalogError::TooLarge`], which surfaces name apart (HTTP 413).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum CatalogError {
    /// The value is not `{catalogId, version, digest, catalog}`.
    #[error("the UI catalog must be an object with catalogId, version, digest and catalog: {0}")]
    Shape(String),
    /// `catalogId` is not an absolute https URL that fits.
    #[error("catalogId must be an absolute https URL of at most {MAX_CATALOG_ID_BYTES} bytes")]
    CatalogId,
    /// `version` is outside `1..=`[`MAX_CATALOG_VERSION`].
    #[error("version must be an integer from 1 to {MAX_CATALOG_VERSION}")]
    Version,
    /// `digest` is not `sha256:` and 64 lowercase hex digits.
    #[error("digest must be \"sha256:\" followed by 64 lowercase hexadecimal digits")]
    DigestFormat,
    /// `digest` is well formed, but is not the digest of `catalog`.
    #[error("digest does not match the catalog: the digest of what was sent is {expected}")]
    DigestMismatch {
        /// The digest of the catalog as received.
        expected: String,
    },
    /// The catalog is larger than [`MAX_CATALOG_BYTES`].
    #[error("the catalog is larger than {MAX_CATALOG_BYTES} bytes")]
    TooLarge,
    /// `catalog.catalogId` is not `catalogId`.
    #[error("catalog.catalogId must be the catalogId")]
    IdMismatch,
    /// The catalog has a member other than `catalogId` and `components`.
    #[error("the catalog has a member '{0}'; only catalogId and components are accepted")]
    UnknownMember(String),
    /// `components` is missing, is not an object, or is empty.
    #[error("components must be an object with at least one component")]
    Components,
    /// More components than [`MAX_CATALOG_COMPONENTS`].
    #[error("the catalog has {count} components; at most {MAX_CATALOG_COMPONENTS} are accepted")]
    TooManyComponents {
        /// How many it has.
        count: usize,
    },
    /// A component name is not `[A-Z][A-Za-z0-9]{0,63}`.
    #[error(
        "the component name '{0}' is not valid: an uppercase letter, then up to 63 letters or digits"
    )]
    ComponentName(String),
    /// A component's schema is not an object.
    #[error("the schema of component '{0}' must be a JSON object")]
    ComponentSchema(String),
    /// The catalog nests deeper than [`MAX_CATALOG_DEPTH`].
    #[error("the catalog nests deeper than {MAX_CATALOG_DEPTH} levels")]
    TooDeep,
    /// A keyword that is not allowed in a catalog (a reference or an identity: [`FORBIDDEN_KEYWORDS`]).
    #[error("'{0}' is not allowed in a catalog: each component schema must stand alone")]
    Keyword(String),
    /// An object key that is not ASCII.
    #[error("object keys of a catalog must be ASCII; found '{0}'")]
    NonAsciiKey(String),
    /// A number that is not an integer within the safe range.
    #[error("numbers in a catalog must be integers within +-9007199254740991; found {0}")]
    Number(String),
}

impl CatalogError {
    /// Whether the catalog was refused for its size (a surface answers 413, not 400).
    pub fn is_too_large(&self) -> bool {
        matches!(self, CatalogError::TooLarge)
    }
}

impl Classify for CatalogError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Invalid
    }
}

/// `data` of a `ui_catalog` event, and the object the web sends as
/// `forwardedProps["vymalo.uiCatalog"]`: one version of the UI's catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiCatalogData {
    /// The catalog's stable id, an https URL. A UI keeps it across versions.
    pub catalog_id: String,
    /// The version, from 1, raised by hand with every change of the catalog. It orders catalogs:
    /// the newest is the highest.
    pub version: u32,
    /// `sha256:` and the lowercase hex digest of the canonical JSON of `catalog`.
    pub digest: String,
    /// The catalog, `{catalogId, components}`: an A2UI inline catalog.
    pub catalog: Value,
}

impl UiCatalogData {
    /// Reads and checks what a sender gave as a catalog: the shape, the limits, the allowed
    /// keywords, and that `digest` is the digest of `catalog`. Nothing is interpreted: whether a
    /// component schema is valid JSON Schema is for the caller to check.
    ///
    /// # Errors
    ///
    /// A [`CatalogError`] naming the first rule the value breaks.
    pub fn from_json(value: &Value) -> Result<Self, CatalogError> {
        let wire = Wire::deserialize(value).map_err(|e| CatalogError::Shape(e.to_string()))?;
        if !is_https_url(&wire.catalog_id) {
            return Err(CatalogError::CatalogId);
        }
        let version = u32::try_from(wire.version)
            .ok()
            .filter(|v| (1..=MAX_CATALOG_VERSION).contains(v))
            .ok_or(CatalogError::Version)?;
        if !is_digest(&wire.digest) {
            return Err(CatalogError::DigestFormat);
        }
        // Every check below walks the catalog; the size comes first so that none of them runs on
        // more than the limit.
        if serialized_len(&wire.catalog) > MAX_CATALOG_BYTES {
            return Err(CatalogError::TooLarge);
        }
        check_envelope(&wire.catalog, &wire.catalog_id)?;
        // Canonicalising refuses what no language could write the same way (non-ASCII keys,
        // fractional or huge numbers) and what nests too deep.
        let canonical = canonical_json(&wire.catalog)?;
        if canonical.len() > MAX_CATALOG_BYTES {
            return Err(CatalogError::TooLarge);
        }
        check_keywords(&wire.catalog)?;
        let expected = digest_of_canonical(&canonical);
        if expected != wire.digest {
            return Err(CatalogError::DigestMismatch { expected });
        }
        Ok(UiCatalogData {
            catalog_id: wire.catalog_id,
            version,
            digest: wire.digest,
            catalog: wire.catalog,
        })
    }

    /// Checks a catalog that is already a [`UiCatalogData`] by the rules of
    /// [`from_json`](Self::from_json): for an input built in code, where nothing has read it from
    /// JSON yet.
    ///
    /// # Errors
    ///
    /// A [`CatalogError`] naming the first rule it breaks.
    pub fn check(&self) -> Result<(), CatalogError> {
        let value = serde_json::to_value(self).map_err(|e| CatalogError::Shape(e.to_string()))?;
        UiCatalogData::from_json(&value).map(|_| ())
    }

    /// The reference to this catalog.
    pub fn reference(&self) -> UiCatalogRef {
        UiCatalogRef {
            catalog_id: self.catalog_id.clone(),
            version: self.version,
            digest: self.digest.clone(),
        }
    }
}

/// The members of the object the web sends; anything else in it is refused.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    catalog_id: String,
    version: u64,
    digest: String,
    catalog: Value,
}

/// Which catalog, without its contents: what the state snapshot says, and what every message
/// that does not carry the catalog itself tells the agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiCatalogRef {
    /// The catalog's id.
    pub catalog_id: String,
    /// Its version.
    pub version: u32,
    /// Its digest.
    pub digest: String,
}

/// What a thread knows of the catalogs its UIs sent (`Job.catalog`). Kept across jobs, like the
/// gate: the catalog belongs to the conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiCatalogLedger {
    /// The catalog an agent is to use: the highest version recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<UiCatalogRef>,
    /// The digests recorded, oldest first, at most [`MAX_SEEN_CATALOGS`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    seen: Vec<String>,
}

/// What [`UiCatalogLedger::observe`] did with a catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observed {
    /// The digest was new to the thread: it is recorded, and the event is to be appended.
    pub recorded: bool,
    /// It is now the current catalog. Only a catalog that was recorded can become current.
    pub became_current: bool,
}

impl UiCatalogLedger {
    /// A ledger that has recorded nothing, which is what a thread without a catalog has (and
    /// what the log leaves out).
    pub fn is_empty(&self) -> bool {
        self.current.is_none() && self.seen.is_empty()
    }

    /// The current catalog, if the thread has one.
    pub fn current(&self) -> Option<&UiCatalogRef> {
        self.current.as_ref()
    }

    /// The digests recorded, oldest first.
    pub fn seen(&self) -> &[String] {
        &self.seen
    }

    /// Whether the thread has recorded this digest (the current one counts, even if the window
    /// of [`MAX_SEEN_CATALOGS`] has moved past it).
    pub fn knows(&self, digest: &str) -> bool {
        self.current.as_ref().is_some_and(|c| c.digest == digest)
            || self.seen.iter().any(|d| d == digest)
    }

    /// Records a catalog the thread was shown.
    ///
    /// It is recorded when its digest is new. It becomes current when it is recorded and the
    /// thread had no current catalog, or its version is higher, or it is the same version with
    /// another digest (the later one wins; a UI whose lock file is kept does not do that). An
    /// older version is recorded, so that it is not appended twice, but never becomes current:
    /// the thread keeps the newest. A digest the thread knows changes nothing.
    pub fn observe(&mut self, catalog: &UiCatalogRef) -> Observed {
        if self.knows(&catalog.digest) {
            return Observed {
                recorded: false,
                became_current: false,
            };
        }
        self.seen.push(catalog.digest.clone());
        if self.seen.len() > MAX_SEEN_CATALOGS {
            self.seen.remove(0);
        }
        let became_current = self
            .current
            .as_ref()
            .is_none_or(|current| catalog.version >= current.version);
        if became_current {
            self.current = Some(catalog.clone());
        }
        Observed {
            recorded: true,
            became_current,
        }
    }

    /// Folds the catalog an input carried (if it carried one) into the ledger, and says what the
    /// input's commit is to do: append the `ui_catalog` event (`record`), and what to send the
    /// agent (`delivery`).
    ///
    /// The delivery is the catalog **inline** exactly when this input made it current: that is
    /// the first message of a thread, and the first one after the UI's catalog changed. Otherwise
    /// it is a reference to the current catalog, or nothing when the thread has none.
    pub fn accept(&mut self, carried: Option<&UiCatalogData>) -> Accepted {
        let Some(catalog) = carried else {
            return Accepted {
                record: false,
                delivery: self.redelivery(),
            };
        };
        let observed = self.observe(&catalog.reference());
        let delivery = if observed.became_current {
            Some(UiDelivery::Inline(catalog.clone()))
        } else {
            self.redelivery()
        };
        Accepted {
            record: observed.recorded,
            delivery,
        }
    }

    /// What to send with a message that carries no catalog: a reference to the current one.
    pub fn redelivery(&self) -> Option<UiDelivery> {
        self.current.clone().map(UiDelivery::Ref)
    }
}

/// What [`UiCatalogLedger::accept`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    /// Append the catalog's `ui_catalog` event, first in the commit.
    pub record: bool,
    /// What the delegation to the agent carries.
    pub delivery: Option<UiDelivery>,
}

/// What a delegation tells the agent about the screen's catalog. The thread's current catalog is
/// always one or the other; the agent chooses what to do with it (the adapter sends it only to an
/// agent whose live card lists `ui-catalog/v1`, ADR 0008).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiDelivery {
    /// The catalog itself: this message made it current.
    Inline(UiCatalogData),
    /// Which catalog is current; the agent has it already, or asks for it again.
    Ref(UiCatalogRef),
}

impl UiDelivery {
    /// The catalog the delivery is about.
    pub fn reference(&self) -> UiCatalogRef {
        match self {
            UiDelivery::Inline(data) => data.reference(),
            UiDelivery::Ref(r) => r.clone(),
        }
    }
}

/// `sha256:` and 64 lowercase hex digits.
fn is_digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// An absolute https URL, as far as an id needs to be one: the scheme, a host, printable ASCII
/// only, at most [`MAX_CATALOG_ID_BYTES`] bytes.
fn is_https_url(s: &str) -> bool {
    s.len() <= MAX_CATALOG_ID_BYTES
        && s.bytes().all(|b| b.is_ascii_graphic())
        && s.strip_prefix("https://")
            .is_some_and(|rest| !rest.is_empty() && !rest.starts_with(['/', '?', '#']))
}

/// The members of the catalog object and the names of its components.
fn check_envelope(catalog: &Value, catalog_id: &str) -> Result<(), CatalogError> {
    let Some(object) = catalog.as_object() else {
        return Err(CatalogError::Shape("catalog must be an object".to_owned()));
    };
    if let Some(extra) = object
        .keys()
        .find(|k| !matches!(k.as_str(), "catalogId" | "components"))
    {
        return Err(CatalogError::UnknownMember(extra.clone()));
    }
    if object.get("catalogId").and_then(Value::as_str) != Some(catalog_id) {
        return Err(CatalogError::IdMismatch);
    }
    let components = object
        .get("components")
        .and_then(Value::as_object)
        .filter(|c| !c.is_empty())
        .ok_or(CatalogError::Components)?;
    if components.len() > MAX_CATALOG_COMPONENTS {
        return Err(CatalogError::TooManyComponents {
            count: components.len(),
        });
    }
    for (name, schema) in components {
        if !is_component_name(name) {
            return Err(CatalogError::ComponentName(name.clone()));
        }
        if !schema.is_object() {
            return Err(CatalogError::ComponentSchema(name.clone()));
        }
    }
    Ok(())
}

/// `[A-Z][A-Za-z0-9]{0,63}`.
fn is_component_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_uppercase())
        && name.len() <= 64
        && bytes.all(|b| b.is_ascii_alphanumeric())
}

/// No component schema uses a keyword of [`FORBIDDEN_KEYWORDS`], at any depth. A schema that
/// names a property `$ref` is refused too: a check that told them apart would have to understand
/// JSON Schema, and a component can use another name.
fn check_keywords(value: &Value) -> Result<(), CatalogError> {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                if FORBIDDEN_KEYWORDS.contains(&key.as_str()) {
                    return Err(CatalogError::Keyword(key.clone()));
                }
                check_keywords(inner)?;
            }
            Ok(())
        }
        Value::Array(items) => items.iter().try_for_each(check_keywords),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
    }
}

/// The canonical JSON of `value`: the bytes the digest is taken over.
///
/// Object keys are sorted (by code point, which for the ASCII keys that are allowed is the byte
/// order, and also the UTF-16 order JavaScript sorts by), there is no whitespace, strings are
/// escaped as `serde_json` and `JSON.stringify` do, and integers are in decimal. This is written
/// out, not left to `serde_json`'s map order, which a dependency can change for the whole build
/// (`preserve_order`).
///
/// # Errors
///
/// A key that is not ASCII, a number that is not an integer within +-(2^53 - 1), or a value that
/// nests deeper than [`MAX_CATALOG_DEPTH`].
pub fn canonical_json(value: &Value) -> Result<String, CatalogError> {
    let mut out = String::new();
    write_canonical(value, 0, &mut out)?;
    Ok(out)
}

fn write_canonical(value: &Value, depth: usize, out: &mut String) -> Result<(), CatalogError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            let integer = n
                .as_i64()
                .filter(|i| i.unsigned_abs() <= MAX_SAFE_INTEGER.unsigned_abs())
                .ok_or_else(|| CatalogError::Number(n.to_string()))?;
            // Writing to a String does not fail.
            let _ = write!(out, "{integer}");
        }
        Value::String(s) => write_string(s, out)?,
        Value::Array(items) => {
            nested(depth)?;
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, depth + 1, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            nested(depth)?;
            write_object(map, depth, out)?;
        }
    }
    Ok(())
}

fn write_object(
    map: &Map<String, Value>,
    depth: usize,
    out: &mut String,
) -> Result<(), CatalogError> {
    let mut keys: Vec<&String> = map.keys().collect();
    if let Some(key) = keys.iter().find(|k| !k.is_ascii()) {
        return Err(CatalogError::NonAsciiKey((*key).clone()));
    }
    keys.sort_unstable();
    out.push('{');
    for (i, key) in keys.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_string(key, out)?;
        out.push(':');
        if let Some(inner) = map.get(key) {
            write_canonical(inner, depth + 1, out)?;
        }
    }
    out.push('}');
    Ok(())
}

/// A container at `depth` is one level deeper than its parent: the top-level object is level 1.
fn nested(depth: usize) -> Result<(), CatalogError> {
    if depth >= MAX_CATALOG_DEPTH {
        Err(CatalogError::TooDeep)
    } else {
        Ok(())
    }
}

fn write_string(s: &str, out: &mut String) -> Result<(), CatalogError> {
    let escaped = serde_json::to_string(s).map_err(|e| CatalogError::Shape(e.to_string()))?;
    out.push_str(&escaped);
    Ok(())
}

/// The digest of a catalog: `sha256:` and the lowercase hex of SHA-256 over its canonical JSON.
///
/// # Errors
///
/// What [`canonical_json`] refuses.
pub fn catalog_digest(catalog: &Value) -> Result<String, CatalogError> {
    canonical_json(catalog).map(|canonical| digest_of_canonical(&canonical))
}

fn digest_of_canonical(canonical: &str) -> String {
    let hash = Sha256::digest(canonical.as_bytes());
    let mut out = String::with_capacity("sha256:".len() + 2 * hash.len());
    out.push_str("sha256:");
    for byte in hash {
        // Writing to a String does not fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    const ID: &str = "https://agents.vymalo.com/a2ui/catalogs/test";
    /// The known-answer vector of ADR 0023's contract: the same catalog and digest are pinned in
    /// the web's tests and in adam's.
    const KAT_CATALOG: &str = r#"{"catalogId":"https://agents.vymalo.com/a2ui/catalogs/test","components":{"Note":{"type":"object","properties":{"component":{"const":"Note"},"text":{"type":"string","maxLength":10}},"required":["component","text"]}}}"#;
    const KAT_CANONICAL: &str = r#"{"catalogId":"https://agents.vymalo.com/a2ui/catalogs/test","components":{"Note":{"properties":{"component":{"const":"Note"},"text":{"maxLength":10,"type":"string"}},"required":["component","text"],"type":"object"}}}"#;
    const KAT_DIGEST: &str =
        "sha256:a237e931c3a02fc72b214561e3a52d33eaf0e29c9306bbe0ef7b3da238507293";

    fn kat() -> Value {
        serde_json::from_str(KAT_CATALOG).unwrap()
    }

    /// A catalog with the given components, and its envelope with the right digest.
    fn envelope(version: u64, catalog: &Value) -> Value {
        json!({
            "catalogId": catalog["catalogId"],
            "version": version,
            "digest": catalog_digest(catalog).unwrap(),
            "catalog": catalog,
        })
    }

    fn catalog_with(components: Value) -> Value {
        json!({"catalogId": ID, "components": components})
    }

    fn note() -> Value {
        json!({"type": "object", "properties": {"component": {"const": "Note"}}})
    }

    fn data(version: u32, tag: &str) -> UiCatalogData {
        let catalog = catalog_with(json!({"Note": {"type": "object", "title": tag}}));
        UiCatalogData {
            catalog_id: ID.to_owned(),
            version,
            digest: catalog_digest(&catalog).unwrap(),
            catalog,
        }
    }

    // ---- the digest ----------------------------------------------------------------------

    #[test]
    fn the_known_answer_vector_holds() {
        assert_eq!(canonical_json(&kat()).unwrap(), KAT_CANONICAL);
        assert_eq!(catalog_digest(&kat()).unwrap(), KAT_DIGEST);
    }

    #[test]
    fn the_order_the_keys_arrive_in_does_not_change_the_digest() {
        let shuffled = r#"{"components":{"Note":{"required":["component","text"],"properties":{"text":{"maxLength":10,"type":"string"},"component":{"const":"Note"}},"type":"object"}},"catalogId":"https://agents.vymalo.com/a2ui/catalogs/test"}"#;
        let value: Value = serde_json::from_str(shuffled).unwrap();
        assert_eq!(catalog_digest(&value).unwrap(), KAT_DIGEST);
    }

    #[test]
    fn strings_are_escaped_as_json_stringify_does() {
        let value = json!({"a": "tab\t quote\" slash\\ nl\n bell\u{7} é 日本"});
        assert_eq!(
            canonical_json(&value).unwrap(),
            "{\"a\":\"tab\\t quote\\\" slash\\\\ nl\\n bell\\u0007 é 日本\"}"
        );
    }

    #[test]
    fn integers_are_written_in_decimal_and_other_numbers_are_refused() {
        let ok: Value =
            serde_json::from_str(r#"{"a":[0,-7,9007199254740991,-9007199254740991]}"#).unwrap();
        assert_eq!(
            canonical_json(&ok).unwrap(),
            r#"{"a":[0,-7,9007199254740991,-9007199254740991]}"#
        );
        for bad in [
            "1.5",
            "1.0",
            "1e3",
            "9007199254740992",
            "-9007199254740992",
            "18446744073709551615",
        ] {
            let value: Value = serde_json::from_str(&format!("{{\"a\":{bad}}}")).unwrap();
            assert!(
                matches!(canonical_json(&value), Err(CatalogError::Number(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_key_that_is_not_ascii_is_refused() {
        let value = json!({"outer": {"clé": 1}});
        assert_eq!(
            canonical_json(&value),
            Err(CatalogError::NonAsciiKey("clé".to_owned()))
        );
    }

    #[test]
    fn nesting_is_limited() {
        let mut value = json!(1);
        for _ in 0..MAX_CATALOG_DEPTH {
            value = json!([value]);
        }
        assert!(canonical_json(&value).is_ok(), "the limit itself is fine");
        assert_eq!(canonical_json(&json!([value])), Err(CatalogError::TooDeep));
    }

    // ---- the envelope --------------------------------------------------------------------

    #[test]
    fn a_catalog_with_the_right_digest_is_read() {
        let value = envelope(3, &kat());
        let read = UiCatalogData::from_json(&value).unwrap();
        assert_eq!(
            read.catalog_id,
            "https://agents.vymalo.com/a2ui/catalogs/test"
        );
        assert_eq!(read.version, 3);
        assert_eq!(read.digest, KAT_DIGEST);
        assert_eq!(read.catalog, kat());
        // and it is what the event's data holds
        assert_eq!(serde_json::to_value(&read).unwrap(), value);
    }

    fn refused(value: &Value) -> CatalogError {
        UiCatalogData::from_json(value).unwrap_err()
    }

    #[test]
    fn what_is_not_an_object_of_four_members_is_refused() {
        for bad in [json!(null), json!("x"), json!([]), json!({})] {
            assert!(matches!(refused(&bad), CatalogError::Shape(_)), "{bad}");
        }
        let mut missing = envelope(1, &kat());
        missing.as_object_mut().unwrap().remove("digest");
        assert!(matches!(refused(&missing), CatalogError::Shape(_)));
        let mut extra = envelope(1, &kat());
        extra["extra"] = json!(1);
        assert!(matches!(refused(&extra), CatalogError::Shape(_)));
        let mut wrong_type = envelope(1, &kat());
        wrong_type["version"] = json!("1");
        assert!(matches!(refused(&wrong_type), CatalogError::Shape(_)));
        let mut fractional = envelope(1, &kat());
        fractional["version"] = json!(1.5);
        assert!(matches!(refused(&fractional), CatalogError::Shape(_)));
    }

    #[test]
    fn the_catalog_id_must_be_an_absolute_https_url_that_fits() {
        for bad in [
            "",
            "http://agents.vymalo.com/c",
            "agents.vymalo.com/c",
            "https://",
            "https:///c",
            "https://a b",
            "https://agents.vymalo.com/é",
            &format!(
                "https://agents.vymalo.com/{}",
                "a".repeat(MAX_CATALOG_ID_BYTES)
            ),
        ] {
            let mut value = envelope(1, &kat());
            value["catalogId"] = json!(bad);
            assert_eq!(refused(&value), CatalogError::CatalogId, "{bad}");
        }
        let longest = format!("https://x.test/{}", "a".repeat(MAX_CATALOG_ID_BYTES - 15));
        let catalog = json!({"catalogId": longest, "components": {"Note": note()}});
        assert!(UiCatalogData::from_json(&envelope(1, &catalog)).is_ok());
    }

    #[test]
    fn the_version_is_from_one_to_a_million() {
        for bad in [
            json!(0),
            json!(-1),
            json!(1_000_001),
            json!(4_294_967_296_u64),
        ] {
            let mut value = envelope(1, &kat());
            value["version"] = bad.clone();
            let error = refused(&value);
            assert!(
                matches!(error, CatalogError::Version | CatalogError::Shape(_)),
                "{bad}: {error}"
            );
        }
        for ok in [1, 1_000_000] {
            assert!(UiCatalogData::from_json(&envelope(ok, &kat())).is_ok());
        }
    }

    #[test]
    fn the_digest_must_be_well_formed_and_true() {
        for bad in [
            "",
            "sha256:",
            "sha1:a237e931c3a02fc72b214561e3a52d33eaf0e29c9306bbe0ef7b3da238507293",
            "sha256:A237E931C3A02FC72B214561E3A52D33EAF0E29C9306BBE0EF7B3DA238507293",
            "sha256:a237e931c3a02fc72b214561e3a52d33eaf0e29c9306bbe0ef7b3da23850729",
            "a237e931c3a02fc72b214561e3a52d33eaf0e29c9306bbe0ef7b3da238507293",
        ] {
            let mut value = envelope(1, &kat());
            value["digest"] = json!(bad);
            assert_eq!(refused(&value), CatalogError::DigestFormat, "{bad}");
        }
        let mut value = envelope(1, &kat());
        value["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
        assert_eq!(
            refused(&value),
            CatalogError::DigestMismatch {
                expected: KAT_DIGEST.to_owned()
            }
        );
        // a change to the catalog under the old digest
        let mut changed = envelope(1, &kat());
        changed["catalog"]["components"]["Note"]["properties"]["text"]["maxLength"] = json!(11);
        assert!(matches!(
            refused(&changed),
            CatalogError::DigestMismatch { .. }
        ));
    }

    #[test]
    fn the_catalog_inside_must_say_the_same_id_and_nothing_else() {
        let mut value = envelope(1, &kat());
        value["catalog"]["catalogId"] = json!("https://agents.vymalo.com/a2ui/catalogs/other");
        assert_eq!(refused(&value), CatalogError::IdMismatch);

        let mut not_object = envelope(1, &kat());
        not_object["catalog"] = json!([]);
        assert!(matches!(refused(&not_object), CatalogError::Shape(_)));

        for member in ["functions", "theme", "$schema", "anything"] {
            let mut catalog = kat();
            catalog[member] = json!([]);
            assert_eq!(
                refused(&envelope(1, &catalog)),
                CatalogError::UnknownMember(member.to_owned()),
                "{member}"
            );
        }
    }

    #[test]
    fn components_are_an_object_of_one_to_sixty_four_named_object_schemas() {
        for bad in [json!(null), json!([]), json!({}), json!("Note")] {
            let catalog = catalog_with(bad.clone());
            assert_eq!(
                refused(&envelope(1, &catalog)),
                CatalogError::Components,
                "{bad}"
            );
        }
        let mut missing = kat();
        missing.as_object_mut().unwrap().remove("components");
        assert_eq!(refused(&envelope(1, &missing)), CatalogError::Components);

        let many: Map<String, Value> = (0..=MAX_CATALOG_COMPONENTS)
            .map(|i| (format!("C{i}"), note()))
            .collect();
        assert_eq!(
            refused(&envelope(1, &catalog_with(Value::Object(many)))),
            CatalogError::TooManyComponents {
                count: MAX_CATALOG_COMPONENTS + 1
            }
        );
        let exactly: Map<String, Value> = (0..MAX_CATALOG_COMPONENTS)
            .map(|i| (format!("C{i}"), note()))
            .collect();
        assert!(
            UiCatalogData::from_json(&envelope(1, &catalog_with(Value::Object(exactly)))).is_ok()
        );

        for bad in [
            "note",
            "1Note",
            "Not e",
            "Not-e",
            "N_e",
            &format!("N{}", "a".repeat(64)),
        ] {
            let catalog = catalog_with(json!({ bad: note() }));
            assert_eq!(
                refused(&envelope(1, &catalog)),
                CatalogError::ComponentName(bad.to_owned()),
                "{bad}"
            );
        }
        assert!(
            UiCatalogData::from_json(&envelope(
                1,
                &catalog_with(json!({ format!("N{}", "a".repeat(63)): note() }))
            ))
            .is_ok()
        );
        let catalog = catalog_with(json!({"Note": true}));
        assert_eq!(
            refused(&envelope(1, &catalog)),
            CatalogError::ComponentSchema("Note".to_owned())
        );
    }

    #[test]
    fn a_reference_or_an_identity_keyword_is_refused_at_any_depth() {
        for keyword in FORBIDDEN_KEYWORDS {
            let schema =
                json!({"type": "object", "properties": {"a": {"items": [{ keyword: "x" }]}}});
            let catalog = catalog_with(json!({"Note": schema}));
            assert_eq!(
                refused(&envelope(1, &catalog)),
                CatalogError::Keyword(keyword.to_owned()),
                "{keyword}"
            );
        }
        // as a value, not a keyword, it is only text
        let catalog = catalog_with(json!({"Note": {"description": "see $ref in the docs"}}));
        assert!(UiCatalogData::from_json(&envelope(1, &catalog)).is_ok());
    }

    #[test]
    fn numbers_and_keys_are_refused_through_the_envelope_too() {
        let fractional = catalog_with(json!({"Note": {"type": "number", "multipleOf": 0.5}}));
        let mut value =
            json!({"catalogId": ID, "version": 1, "digest": KAT_DIGEST, "catalog": fractional});
        assert!(matches!(refused(&value), CatalogError::Number(_)));
        value["catalog"] = catalog_with(json!({"Note": {"clé": 1}}));
        assert_eq!(refused(&value), CatalogError::NonAsciiKey("clé".to_owned()));
    }

    #[test]
    fn a_catalog_larger_than_the_limit_is_too_large_and_one_at_the_limit_is_not() {
        let filler = |n: usize| catalog_with(json!({"Note": {"description": "x".repeat(n)}}));
        let overhead = serialized_len(&filler(0));
        let at_limit = filler(MAX_CATALOG_BYTES - overhead);
        assert_eq!(serialized_len(&at_limit), MAX_CATALOG_BYTES);
        assert!(UiCatalogData::from_json(&envelope(1, &at_limit)).is_ok());
        let over = filler(MAX_CATALOG_BYTES - overhead + 1);
        let error = refused(&envelope(1, &over));
        assert_eq!(error, CatalogError::TooLarge);
        assert!(error.is_too_large());
        assert!(!CatalogError::DigestFormat.is_too_large());
        assert_eq!(error.class(), ErrorClass::Invalid);
    }

    // ---- the ledger ----------------------------------------------------------------------

    #[test]
    fn the_first_catalog_is_recorded_and_current_and_comes_inline() {
        let mut ledger = UiCatalogLedger::default();
        assert!(ledger.is_empty());
        let v1 = data(1, "a");
        let accepted = ledger.accept(Some(&v1));
        assert_eq!(
            accepted,
            Accepted {
                record: true,
                delivery: Some(UiDelivery::Inline(v1.clone()))
            }
        );
        assert_eq!(ledger.current(), Some(&v1.reference()));
        assert_eq!(ledger.seen(), std::slice::from_ref(&v1.digest));
        assert!(!ledger.is_empty());
    }

    #[test]
    fn the_same_digest_again_changes_nothing_and_comes_as_a_reference() {
        let mut ledger = UiCatalogLedger::default();
        let v1 = data(1, "a");
        ledger.accept(Some(&v1));
        let before = ledger.clone();
        let again = ledger.accept(Some(&v1));
        assert_eq!(
            again,
            Accepted {
                record: false,
                delivery: Some(UiDelivery::Ref(v1.reference()))
            }
        );
        assert_eq!(ledger, before);
    }

    #[test]
    fn a_newer_version_becomes_current_and_comes_inline() {
        let mut ledger = UiCatalogLedger::default();
        let (v1, v2) = (data(1, "a"), data(2, "b"));
        ledger.accept(Some(&v1));
        let accepted = ledger.accept(Some(&v2));
        assert!(accepted.record);
        assert_eq!(accepted.delivery, Some(UiDelivery::Inline(v2.clone())));
        assert_eq!(ledger.current(), Some(&v2.reference()));
        assert_eq!(ledger.seen(), [v1.digest, v2.digest]);
    }

    #[test]
    fn an_older_version_is_recorded_once_and_never_becomes_current() {
        let mut ledger = UiCatalogLedger::default();
        let (v1, v2) = (data(1, "a"), data(2, "b"));
        ledger.accept(Some(&v2));
        let accepted = ledger.accept(Some(&v1));
        assert!(accepted.record, "an unseen digest is recorded");
        assert_eq!(
            accepted.delivery,
            Some(UiDelivery::Ref(v2.reference())),
            "the agent is told the newest"
        );
        assert_eq!(ledger.current(), Some(&v2.reference()));
        let again = ledger.accept(Some(&v1));
        assert!(!again.record, "and not a second time");
    }

    #[test]
    fn the_same_version_with_another_digest_replaces_the_current_one() {
        let mut ledger = UiCatalogLedger::default();
        let (first, second) = (data(2, "a"), data(2, "b"));
        ledger.accept(Some(&first));
        let accepted = ledger.accept(Some(&second));
        assert!(accepted.record);
        assert_eq!(accepted.delivery, Some(UiDelivery::Inline(second.clone())));
        assert_eq!(ledger.current(), Some(&second.reference()));
        // the first is known: sending it again is neither recorded nor current again
        let back = ledger.accept(Some(&first));
        assert!(!back.record);
        assert_eq!(back.delivery, Some(UiDelivery::Ref(second.reference())));
    }

    #[test]
    fn a_message_without_a_catalog_carries_a_reference_to_the_current_one() {
        let mut ledger = UiCatalogLedger::default();
        assert_eq!(
            ledger.accept(None),
            Accepted {
                record: false,
                delivery: None
            }
        );
        assert_eq!(ledger.redelivery(), None);
        let v1 = data(1, "a");
        ledger.accept(Some(&v1));
        assert_eq!(
            ledger.accept(None).delivery,
            Some(UiDelivery::Ref(v1.reference()))
        );
        assert_eq!(ledger.redelivery(), Some(UiDelivery::Ref(v1.reference())));
    }

    #[test]
    fn the_oldest_digest_is_forgotten_past_the_window_but_the_current_one_stays_known() {
        let mut ledger = UiCatalogLedger::default();
        let current = data(100, "current");
        ledger.accept(Some(&current));
        let olders: Vec<_> = (1..=MAX_SEEN_CATALOGS as u32 + 2)
            .map(|i| data(i, &format!("older-{i}")))
            .collect();
        for older in &olders {
            ledger.accept(Some(older));
        }
        assert_eq!(ledger.seen().len(), MAX_SEEN_CATALOGS);
        assert_eq!(ledger.current(), Some(&current.reference()));
        // the current catalog's digest fell out of the window, and is still known
        assert!(!ledger.seen().contains(&current.digest));
        assert!(ledger.knows(&current.digest));
        assert!(!ledger.accept(Some(&current)).record);
        // the newest older ones are remembered; the very first is forgotten
        assert!(ledger.knows(&olders[olders.len() - 1].digest));
        assert!(!ledger.knows(&olders[0].digest));
    }

    #[test]
    fn the_ledger_and_the_delivery_have_the_wire_shapes_the_store_and_the_outbox_keep() {
        let mut ledger = UiCatalogLedger::default();
        assert_eq!(serde_json::to_value(&ledger).unwrap(), json!({}));
        let v1 = data(1, "a");
        ledger.accept(Some(&v1));
        let value = serde_json::to_value(&ledger).unwrap();
        assert_eq!(
            value,
            json!({
                "current": {"catalogId": ID, "version": 1, "digest": v1.digest},
                "seen": [v1.digest],
            })
        );
        assert_eq!(
            serde_json::from_value::<UiCatalogLedger>(value).unwrap(),
            ledger
        );
        assert_eq!(
            serde_json::from_value::<UiCatalogLedger>(json!({})).unwrap(),
            UiCatalogLedger::default()
        );

        let inline = serde_json::to_value(UiDelivery::Inline(v1.clone())).unwrap();
        assert_eq!(
            inline,
            json!({"inline": serde_json::to_value(&v1).unwrap()})
        );
        let by_ref = serde_json::to_value(UiDelivery::Ref(v1.reference())).unwrap();
        assert_eq!(
            by_ref,
            json!({"ref": {"catalogId": ID, "version": 1, "digest": v1.digest}})
        );
        for delivery in [
            UiDelivery::Inline(v1.clone()),
            UiDelivery::Ref(v1.reference()),
        ] {
            let back: UiDelivery =
                serde_json::from_value(serde_json::to_value(&delivery).unwrap()).unwrap();
            assert_eq!(back, delivery);
            assert_eq!(back.reference(), v1.reference());
        }
    }
}
