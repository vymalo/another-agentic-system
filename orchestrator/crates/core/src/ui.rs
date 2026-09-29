//! A2UI in the core: the shape rules of the envelope an agent sends, and of the action a user
//! sends back (ADR 0013). Pure functions over `serde_json::Value`; no I/O.
//!
//! The orchestrator **relays** A2UI, it does not interpret it. What it does check, before
//! anything is stored or shown to a viewer, is the envelope:
//!
//! - the payload is a non-empty JSON array of at most [`MAX_OPERATIONS`] objects, at most
//!   [`MAX_OPERATIONS_BYTES`] serialised;
//! - every object has a `version` this build knows ([`UiVersion`]) and exactly one surface
//!   operation ([`SurfaceOp`]) whose body names a `surfaceId`.
//!
//! Components are not validated against a catalog here; the renderer does that (ADR 0013). Anything
//! that fails the envelope is refused whole, never passed on.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The most operations one payload may hold.
pub const MAX_OPERATIONS: usize = 256;
/// The most bytes (serialised JSON) one payload may hold: the renderer's per-surface limit
/// (ADR 0013), so a payload the renderer would refuse is never stored.
pub const MAX_OPERATIONS_BYTES: usize = 64 * 1024;
/// The most bytes of one surface's accumulated operations that the projection replays in a
/// snapshot: the whole surface is re-sent with every update, so it is bounded (and bigger than a
/// payload, because a surface is built from several).
pub const MAX_SURFACE_BYTES: usize = 256 * 1024;
/// The longest `surfaceId`, action `name` or `sourceComponentId`, in bytes.
pub const MAX_ID_BYTES: usize = 256;
/// The most bytes (serialised JSON) an action's `context` may hold.
pub const MAX_ACTION_CONTEXT_BYTES: usize = 16 * 1024;

/// An A2UI protocol version, as spelled in the `version` member of every message.
///
/// The set is closed on purpose: a version this build has not reviewed is refused (fail closed),
/// and the set moves with A2UI releases in a reviewed change (open question 22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum UiVersion {
    /// `v0.9`.
    #[serde(rename = "v0.9")]
    V0_9,
    /// `v0.9.1`, the current release.
    #[serde(rename = "v0.9.1")]
    V0_9_1,
    /// `v1.0`, a candidate.
    #[serde(rename = "v1.0")]
    V1_0,
}

impl UiVersion {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            UiVersion::V0_9 => "v0.9",
            UiVersion::V0_9_1 => "v0.9.1",
            UiVersion::V1_0 => "v1.0",
        }
    }

    /// The version spelled `s`, if this build knows it.
    pub fn parse(s: &str) -> Option<Self> {
        [UiVersion::V0_9, UiVersion::V0_9_1, UiVersion::V1_0]
            .into_iter()
            .find(|v| v.as_str() == s)
    }
}

impl UiVersion {
    /// The URI of the A2A extension for this version, when the extension is defined for it.
    /// `v0.9` predates the extension (ADR 0013); an agent that sends it is relayed all the same,
    /// but its card is never read for it.
    ///
    /// *Verified 2026-09-29*: <https://a2ui.org/specification/v0.9.1-a2ui-extension-specification/>
    /// and <https://a2ui.org/specification/v1.0-a2ui-extension-specification/>.
    pub fn extension_uri(self) -> Option<&'static str> {
        match self {
            UiVersion::V0_9 => None,
            UiVersion::V0_9_1 => Some(A2UI_EXTENSION_V0_9_1),
            UiVersion::V1_0 => Some(A2UI_EXTENSION_V1_0),
        }
    }

    /// The version whose extension URI is `uri`.
    pub fn from_extension_uri(uri: &str) -> Option<Self> {
        [UiVersion::V0_9_1, UiVersion::V1_0]
            .into_iter()
            .find(|v| v.extension_uri() == Some(uri))
    }

    /// The catalog ids the web renderer supports for this version: the basic catalog, which is
    /// what `@assistant-ui/react-generative-ui` maps onto our vocabulary (ADR 0013). This is what
    /// the orchestrator tells an agent it may generate. For `v0.9.1` both spellings of the basic
    /// catalog's id are listed, because the specification's own pages disagree
    /// (`v0_9_1` in the protocol page, `v0_9` in the extension page; *verified 2026-09-29*).
    pub fn basic_catalog_ids(self) -> &'static [&'static str] {
        match self {
            UiVersion::V0_9 => &["https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"],
            UiVersion::V0_9_1 => &[
                "https://a2ui.org/specification/v0_9_1/catalogs/basic/catalog.json",
                "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
            ],
            UiVersion::V1_0 => &["https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json"],
        }
    }
}

/// The A2A extension URI of A2UI v0.9.1, the current release (ADR 0013).
pub const A2UI_EXTENSION_V0_9_1: &str = "https://a2ui.org/a2a-extension/a2ui/v0.9.1";
/// The A2A extension URI of A2UI v1.0, a candidate (open question 22).
pub const A2UI_EXTENSION_V1_0: &str = "https://a2ui.org/a2a-extension/a2ui/v1.0";
/// The media type of an A2A part that carries A2UI messages.
pub const A2UI_MEDIA_TYPE: &str = "application/a2ui+json";

/// The surface operations the orchestrator relays: the four every A2UI version has.
///
/// `v1.0` adds `callRendererFunction` and `agentFunctionResponse`, which need a reply from the
/// renderer; they are not relayed (refused as unknown operations).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceOp {
    /// `createSurface`.
    Create,
    /// `updateComponents`.
    UpdateComponents,
    /// `updateDataModel`.
    UpdateDataModel,
    /// `deleteSurface`.
    Delete,
}

impl SurfaceOp {
    fn from_key(key: &str) -> Option<Self> {
        match key {
            "createSurface" => Some(SurfaceOp::Create),
            "updateComponents" => Some(SurfaceOp::UpdateComponents),
            "updateDataModel" => Some(SurfaceOp::UpdateDataModel),
            "deleteSurface" => Some(SurfaceOp::Delete),
            _ => None,
        }
    }
}

/// What the envelope check learns about one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationInfo<'a> {
    /// Which operation.
    pub op: SurfaceOp,
    /// The surface it addresses.
    pub surface_id: &'a str,
    /// The protocol version it declares.
    pub version: UiVersion,
}

/// Why one operation fails the envelope check.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OperationError {
    /// It is not a JSON object.
    #[error("not an object")]
    NotAnObject,
    /// It has no string `version`.
    #[error("no version")]
    MissingVersion,
    /// Its `version` is not one this build knows.
    #[error("unsupported version {0:?}")]
    UnsupportedVersion(String),
    /// It names no operation.
    #[error("no operation")]
    NoOperation,
    /// It names more than one operation, or a member that is neither `version` nor an operation.
    #[error("more than one operation, or an unknown member")]
    SeveralOperations,
    /// Its operation is not one this build relays.
    #[error("unsupported operation {0:?}")]
    UnknownOperation(String),
    /// Its operation body is not an object with a `surfaceId` of 1 to [`MAX_ID_BYTES`] bytes.
    #[error("no valid surfaceId")]
    BadSurfaceId,
}

/// Why a payload is refused. The text ends up in an `error` event that viewers see, so it names
/// the rule and never repeats more than a short, escaped excerpt of what the agent sent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UiRejection {
    /// The payload is not a JSON array.
    #[error("the payload is not an array of A2UI messages")]
    NotAnArray,
    /// The array is empty.
    #[error("the payload holds no A2UI messages")]
    Empty,
    /// More than [`MAX_OPERATIONS`] messages.
    #[error("the payload holds {count} messages, more than the limit of {max}")]
    TooManyOperations {
        /// How many.
        count: usize,
        /// The limit.
        max: usize,
    },
    /// More than [`MAX_OPERATIONS_BYTES`] bytes.
    #[error("the payload is {bytes} bytes, more than the limit of {max}")]
    TooLarge {
        /// How large.
        bytes: usize,
        /// The limit.
        max: usize,
    },
    /// One message fails the check.
    #[error("message {index}: {why}")]
    Operation {
        /// Its position in the array.
        index: usize,
        /// The rule it breaks.
        why: OperationError,
    },
}

/// Checks one operation and says what it is. Nothing is copied.
pub fn inspect(op: &Value) -> Result<OperationInfo<'_>, OperationError> {
    let Some(object) = op.as_object() else {
        return Err(OperationError::NotAnObject);
    };
    let version = match object.get("version") {
        Some(Value::String(v)) => {
            UiVersion::parse(v).ok_or_else(|| OperationError::UnsupportedVersion(excerpt(v)))?
        }
        Some(_) | None => return Err(OperationError::MissingVersion),
    };
    let mut keys = object.keys().filter(|k| k.as_str() != "version");
    let (Some(key), None) = (keys.next(), keys.next()) else {
        return Err(if object.len() <= 1 {
            OperationError::NoOperation
        } else {
            OperationError::SeveralOperations
        });
    };
    let Some(kind) = SurfaceOp::from_key(key) else {
        return Err(OperationError::UnknownOperation(excerpt(key)));
    };
    let surface_id = object
        .get(key)
        .and_then(Value::as_object)
        .and_then(|body| body.get("surfaceId"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= MAX_ID_BYTES)
        .ok_or(OperationError::BadSurfaceId)?;
    Ok(OperationInfo {
        op: kind,
        surface_id,
        version,
    })
}

/// A short, escaped excerpt of untrusted text, for a message a viewer will read.
fn excerpt(s: &str) -> String {
    s.chars().take(32).collect()
}

/// Checks a payload (the `data` of an A2UI part) and returns its operations, unchanged.
///
/// This is the only door: nothing an agent sent reaches the log, a viewer or an agent's
/// successor without passing it. The A2A adapter passes each part through it, and `transition`
/// passes every [`AgentUpdate::Ui`](crate::AgentUpdate::Ui) through it again, so an adapter that
/// forgot cannot get an unchecked payload into the log.
pub fn check_operations(payload: &Value) -> Result<Vec<Value>, UiRejection> {
    let Some(items) = payload.as_array() else {
        return Err(UiRejection::NotAnArray);
    };
    check_operation_list(items)?;
    Ok(items.clone())
}

/// [`check_operations`] for operations already split out of their array.
pub fn check_operation_list(items: &[Value]) -> Result<(), UiRejection> {
    if items.is_empty() {
        return Err(UiRejection::Empty);
    }
    if items.len() > MAX_OPERATIONS {
        return Err(UiRejection::TooManyOperations {
            count: items.len(),
            max: MAX_OPERATIONS,
        });
    }
    let bytes = serde_json::to_vec(items).map_or(usize::MAX, |v| v.len());
    if bytes > MAX_OPERATIONS_BYTES {
        return Err(UiRejection::TooLarge {
            bytes,
            max: MAX_OPERATIONS_BYTES,
        });
    }
    for (index, op) in items.iter().enumerate() {
        inspect(op).map_err(|why| UiRejection::Operation { index, why })?;
    }
    Ok(())
}

/// The serialised size of a JSON value; a value always serialises, so a failure counts as too
/// large.
pub fn serialized_len(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |v| v.len())
}

/// `data` of a `ui_surface` event: the operations of one payload, as the agent sent them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiSurfaceData {
    /// A2UI messages, each already through [`check_operations`].
    pub operations: Vec<Value>,
}

/// `data` of a `ui_action` event, and the action the agent receives: what the user did on a
/// surface of the thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiActionData {
    /// The surface the action came from (one the thread has).
    pub surface_id: String,
    /// The action's name.
    pub name: String,
    /// The component that triggered it. A2UI requires it.
    pub source_component_id: String,
    /// The context the component's action declared, already resolved by the renderer.
    pub context: Map<String, Value>,
    /// The A2UI version of the surface, so the agent gets the action in the dialect it spoke.
    pub version: UiVersion,
    /// The id of the run the action started or continued, when the surface named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// Why an action is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UiActionError {
    /// A required text member is empty.
    #[error("{field} must not be empty")]
    Empty {
        /// The member.
        field: &'static str,
    },
    /// A text member is longer than [`MAX_ID_BYTES`].
    #[error("{field} must be at most {max} bytes")]
    TooLong {
        /// The member.
        field: &'static str,
        /// The limit.
        max: usize,
    },
    /// The context is larger than [`MAX_ACTION_CONTEXT_BYTES`].
    #[error("context is {bytes} bytes, more than the limit of {max}")]
    ContextTooLarge {
        /// How large.
        bytes: usize,
        /// The limit.
        max: usize,
    },
}

impl UiActionError {
    /// The action is too big rather than malformed.
    pub fn is_oversized(&self) -> bool {
        match self {
            UiActionError::TooLong { .. } | UiActionError::ContextTooLarge { .. } => true,
            UiActionError::Empty { .. } => false,
        }
    }
}

impl UiActionData {
    /// Checks the sizes and required members. Whether the surface exists is the caller's
    /// business (it needs the log).
    pub fn check(&self) -> Result<(), UiActionError> {
        for (field, value) in [
            ("surfaceId", &self.surface_id),
            ("name", &self.name),
            ("sourceComponentId", &self.source_component_id),
        ] {
            if value.is_empty() {
                return Err(UiActionError::Empty { field });
            }
            if value.len() > MAX_ID_BYTES {
                return Err(UiActionError::TooLong {
                    field,
                    max: MAX_ID_BYTES,
                });
            }
        }
        let bytes = self
            .context
            .iter()
            .map(|(k, v)| k.len() + serialized_len(v))
            .fold(0usize, usize::saturating_add);
        if bytes > MAX_ACTION_CONTEXT_BYTES {
            return Err(UiActionError::ContextTooLarge {
                bytes,
                max: MAX_ACTION_CONTEXT_BYTES,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn create() -> Value {
        json!({"version": "v0.9.1", "createSurface": {"surfaceId": "s1", "catalogId": "c"}})
    }

    #[test]
    fn a_valid_payload_is_returned_unchanged() {
        let payload = json!([
            create(),
            {"version": "v0.9.1", "updateComponents": {"surfaceId": "s1", "components": []}},
            {"version": "v0.9.1", "updateDataModel": {"surfaceId": "s1", "value": 1}},
            {"version": "v0.9.1", "deleteSurface": {"surfaceId": "s1"}},
        ]);
        let ops = check_operations(&payload).unwrap();
        assert_eq!(Value::Array(ops), payload);
    }

    #[test]
    fn every_known_version_is_accepted_and_round_trips() {
        for v in ["v0.9", "v0.9.1", "v1.0"] {
            let op = json!({"version": v, "deleteSurface": {"surfaceId": "s"}});
            let info = inspect(&op).unwrap();
            assert_eq!(info.version.as_str(), v);
            assert_eq!(info.op, SurfaceOp::Delete);
            assert_eq!(UiVersion::parse(v), Some(info.version));
            let json = serde_json::to_value(info.version).unwrap();
            assert_eq!(json, json!(v));
        }
    }

    #[test]
    fn the_envelope_rules() {
        let cases: Vec<(Value, UiRejection)> = vec![
            (json!({"a": 1}), UiRejection::NotAnArray),
            (json!("x"), UiRejection::NotAnArray),
            (json!([]), UiRejection::Empty),
            (
                json!([1]),
                UiRejection::Operation {
                    index: 0,
                    why: OperationError::NotAnObject,
                },
            ),
            (
                json!([create(), {"createSurface": {"surfaceId": "s"}}]),
                UiRejection::Operation {
                    index: 1,
                    why: OperationError::MissingVersion,
                },
            ),
            (
                json!([{"version": 9, "createSurface": {"surfaceId": "s"}}]),
                UiRejection::Operation {
                    index: 0,
                    why: OperationError::MissingVersion,
                },
            ),
            (
                json!([{"version": "v0.8", "createSurface": {"surfaceId": "s"}}]),
                UiRejection::Operation {
                    index: 0,
                    why: OperationError::UnsupportedVersion("v0.8".into()),
                },
            ),
            (
                json!([{"version": "v0.9.1"}]),
                UiRejection::Operation {
                    index: 0,
                    why: OperationError::NoOperation,
                },
            ),
            (
                json!([{"version": "v0.9.1", "createSurface": {"surfaceId": "s"}, "deleteSurface": {"surfaceId": "s"}}]),
                UiRejection::Operation {
                    index: 0,
                    why: OperationError::SeveralOperations,
                },
            ),
            (
                json!([{"version": "v1.0", "callRendererFunction": {"surfaceId": "s"}}]),
                UiRejection::Operation {
                    index: 0,
                    why: OperationError::UnknownOperation("callRendererFunction".into()),
                },
            ),
            (
                json!([{"version": "v0.9.1", "createSurface": "s"}]),
                UiRejection::Operation {
                    index: 0,
                    why: OperationError::BadSurfaceId,
                },
            ),
            (
                json!([{"version": "v0.9.1", "createSurface": {"surfaceId": ""}}]),
                UiRejection::Operation {
                    index: 0,
                    why: OperationError::BadSurfaceId,
                },
            ),
            (
                json!([{"version": "v0.9.1", "createSurface": {"surfaceId": 7}}]),
                UiRejection::Operation {
                    index: 0,
                    why: OperationError::BadSurfaceId,
                },
            ),
        ];
        for (payload, want) in cases {
            assert_eq!(check_operations(&payload), Err(want), "{payload}");
        }
    }

    #[test]
    fn a_surface_id_longer_than_the_limit_is_refused() {
        let id = "s".repeat(MAX_ID_BYTES + 1);
        let op = json!({"version": "v0.9.1", "createSurface": {"surfaceId": id}});
        assert_eq!(inspect(&op), Err(OperationError::BadSurfaceId));
        let ok = "s".repeat(MAX_ID_BYTES);
        let op = json!({"version": "v0.9.1", "createSurface": {"surfaceId": ok}});
        assert!(inspect(&op).is_ok());
    }

    #[test]
    fn size_and_count_are_capped() {
        let big = json!([{"version": "v0.9.1", "updateDataModel": {
            "surfaceId": "s", "value": "x".repeat(MAX_OPERATIONS_BYTES)}}]);
        assert!(matches!(
            check_operations(&big),
            Err(UiRejection::TooLarge { .. })
        ));
        let many = Value::Array(vec![create(); MAX_OPERATIONS + 1]);
        assert!(matches!(
            check_operations(&many),
            Err(UiRejection::TooManyOperations { .. })
        ));
        let at_limit = Value::Array(vec![create(); MAX_OPERATIONS]);
        assert!(check_operations(&at_limit).is_ok());
    }

    #[test]
    fn a_rejection_never_repeats_more_than_an_excerpt() {
        let long = "x".repeat(10_000);
        let op = json!({"version": long, "createSurface": {"surfaceId": "s"}});
        let text = check_operations(&json!([op])).unwrap_err().to_string();
        assert!(text.len() < 200, "{}", text.len());
    }

    fn action() -> UiActionData {
        UiActionData {
            surface_id: "s1".into(),
            name: "submit".into(),
            source_component_id: "btn".into(),
            context: Map::new(),
            version: UiVersion::V0_9_1,
            run_id: None,
        }
    }

    #[test]
    fn an_action_is_checked() {
        assert_eq!(action().check(), Ok(()));
        let mut a = action();
        a.name.clear();
        assert_eq!(a.check(), Err(UiActionError::Empty { field: "name" }));
        assert!(!a.check().unwrap_err().is_oversized());
        let mut a = action();
        a.source_component_id = "c".repeat(MAX_ID_BYTES + 1);
        let err = a.check().unwrap_err();
        assert!(err.is_oversized(), "{err}");
        let mut a = action();
        a.context
            .insert("k".into(), json!("v".repeat(MAX_ACTION_CONTEXT_BYTES)));
        let err = a.check().unwrap_err();
        assert!(matches!(err, UiActionError::ContextTooLarge { .. }));
        assert!(err.is_oversized());
    }

    #[test]
    fn an_action_serialises_as_the_contract_says() {
        let mut a = action();
        a.context.insert("email".into(), json!("a@b.c"));
        a.run_id = Some("run-9".into());
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(
            v,
            json!({"surfaceId": "s1", "name": "submit", "sourceComponentId": "btn",
                   "context": {"email": "a@b.c"}, "version": "v0.9.1", "runId": "run-9"})
        );
        assert_eq!(serde_json::from_value::<UiActionData>(v).unwrap(), a);
    }
}
