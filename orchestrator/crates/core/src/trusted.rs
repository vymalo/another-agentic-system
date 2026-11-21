//! The trusted components an agent may ask a client to draw (`trusted-components/v1`, ADR 0083,
//! `docs/api/trusted-components-v1.md`): what a live card says, what a request may name, and the
//! hints it may carry.
//!
//! A trusted component is **not** a catalog component. The agent never draws it: it names one in
//! an A2A message's `metadata[<uri>]` as `{component, hints}`, and the client answers by drawing
//! its own interface, with its own frame. The hints are the only thing the agent controls, and
//! they are suggestions the person edits before anything is granted (ADR 0083 decisions 1, 3
//! and 4).
//!
//! Both halves fail closed (ADR 0008): a card that lists the extension with `params` that are not
//! `{components: [...]}` offers nothing, and a request that names a component this build does not
//! draw, or carries hints out of bounds, is not read at all. The set of components is closed
//! (ADR 0004): only [`TrustedComponent::McpCredential`], whose API exists (ADR 0046), is known
//! today.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The URI of `trusted-components/v1`: the components an agent may ask the client to draw
/// (ADR 0083).
pub const TRUSTED_COMPONENTS_EXTENSION: &str =
    "https://agents.vymalo.com/a2a/extensions/trusted-components/v1";

/// A trusted component a client may be asked to draw. Closed (ADR 0004, ADR 0083 decision 1): the
/// compiler lists every match a new component has to reach.
///
/// Only [`TrustedComponent::McpCredential`] is built (ADR 0046's `POST /api/user-tool-servers`):
/// the other four components the ADR names (`ApiKeyCreate`, `ModelServer`, `SecretGenerate`,
/// `SecretCollect`) have no route or store yet, so this build cannot draw them and a request for
/// one is ignored, never approximated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TrustedComponent {
    /// A person's own MCP server credential (ADR 0046): the client calls
    /// `POST /api/user-tool-servers` with the person's credentials, and the secret is write-only.
    #[serde(rename = "McpCredential")]
    McpCredential,
}

impl TrustedComponent {
    /// Every component this build can draw.
    pub const ALL: [TrustedComponent; 1] = [TrustedComponent::McpCredential];

    /// The wire name of the component, as a card lists it and a request names it.
    pub const fn as_str(self) -> &'static str {
        match self {
            TrustedComponent::McpCredential => "McpCredential",
        }
    }

    /// The component whose wire name is exactly `name`: matched as written, never folded.
    pub fn from_name(name: &str) -> Option<Self> {
        TrustedComponent::ALL
            .into_iter()
            .find(|component| component.as_str() == name)
    }
}

/// The most characters of a hint's `name` (the same bound as a person's own MCP server, ADR 0046).
pub const MAX_TRUSTED_NAME_CHARS: usize = 80;
/// The most scopes a hint may suggest (ADR 0046's bound for an `oauth2` server).
pub const MAX_TRUSTED_SCOPES: usize = 32;
/// The most bytes of one suggested scope (ADR 0046's bound).
pub const MAX_TRUSTED_SCOPE_BYTES: usize = 128;
/// The longest `expiresInDays` a hint may suggest: ten years. A hint is a suggestion the person
/// edits; a grant that never expires is not one this bound allows.
pub const MAX_TRUSTED_EXPIRES_IN_DAYS: u32 = 3650;

/// The suggestions an agent may send with a request (ADR 0083 decision 4). Every member is
/// optional and bounded: the agent's words are untrusted, and the client shows them as
/// "suggested by <agent>" for the person to edit and confirm.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedHints {
    /// A name to suggest for what is being created (a server's name, for `McpCredential`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Scopes to suggest, in the agent's order; a repeat is the same scope twice and the client
    /// treats them as a set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    /// How long the person may want the grant to last, in days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_days: Option<u32>,
}

/// Why a request's hints are refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TrustedHintsInvalid {
    /// A name is blank, longer than [`MAX_TRUSTED_NAME_CHARS`], or has a control character.
    #[error("a name is 1 to {MAX_TRUSTED_NAME_CHARS} characters with no control character")]
    BadName,
    /// More than [`MAX_TRUSTED_SCOPES`] scopes.
    #[error("at most {MAX_TRUSTED_SCOPES} scopes")]
    TooManyScopes,
    /// A scope is not 1 to [`MAX_TRUSTED_SCOPE_BYTES`] bytes of visible ASCII without a space,
    /// quote or backslash (RFC 6749, as ADR 0046 checks one).
    #[error(
        "a scope is 1 to {MAX_TRUSTED_SCOPE_BYTES} characters with no space, quote or backslash"
    )]
    BadScope,
    /// `expiresInDays` is not a whole number in `1..=`[`MAX_TRUSTED_EXPIRES_IN_DAYS`].
    #[error("expiresInDays is a whole number from 1 to {MAX_TRUSTED_EXPIRES_IN_DAYS}")]
    BadExpiry,
}

impl TrustedHints {
    /// Reads a request's `hints` object strictly: an unknown member is ignored, a member of the
    /// wrong type or out of bounds is `None` (fail closed). `name` and `scopes` are optional;
    /// `null` reads as absent.
    pub fn from_value(value: &Value) -> Option<TrustedHints> {
        let object = value.as_object()?;
        let name = match object.get("name") {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) => Some(name.clone()),
            Some(_) => return None,
        };
        let scopes = match object.get("scopes") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => {
                let mut scopes = Vec::with_capacity(items.len());
                for item in items {
                    scopes.push(item.as_str()?.to_owned());
                }
                scopes
            }
            Some(_) => return None,
        };
        let expires_in_days = match object.get("expiresInDays") {
            None | Some(Value::Null) => None,
            Some(value) => Some(whole_days(value)?),
        };
        let hints = TrustedHints {
            name,
            scopes,
            expires_in_days,
        };
        hints.check().ok()?;
        Some(hints)
    }

    /// Checks the bounds of every member, as the core's door checks them again whatever adapter
    /// sent the request.
    ///
    /// # Errors
    /// [`TrustedHintsInvalid`] for the first member out of bounds.
    pub fn check(&self) -> Result<(), TrustedHintsInvalid> {
        if let Some(name) = &self.name
            && (name.trim().is_empty()
                || name.chars().count() > MAX_TRUSTED_NAME_CHARS
                || name.chars().any(char::is_control))
        {
            return Err(TrustedHintsInvalid::BadName);
        }
        if self.scopes.len() > MAX_TRUSTED_SCOPES {
            return Err(TrustedHintsInvalid::TooManyScopes);
        }
        if self.scopes.iter().any(|scope| !usable_scope(scope)) {
            return Err(TrustedHintsInvalid::BadScope);
        }
        if self
            .expires_in_days
            .is_some_and(|days| !(1..=MAX_TRUSTED_EXPIRES_IN_DAYS).contains(&days))
        {
            return Err(TrustedHintsInvalid::BadExpiry);
        }
        Ok(())
    }
}

/// A JSON number that is a whole number in `1..=MAX_TRUSTED_EXPIRES_IN_DAYS`; A2A's metadata is a
/// protobuf `Struct`, so `30` arrives as `30.0` through the SDK and a whole double counts.
fn whole_days(value: &Value) -> Option<u32> {
    let n = match value.as_u64() {
        Some(n) => n,
        None => {
            let f = value.as_f64()?;
            if f.fract() != 0.0 || f < 1.0 || f > f64::from(MAX_TRUSTED_EXPIRES_IN_DAYS) {
                return None;
            }
            // whole and in range, checked above
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let n = f as u64;
            n
        }
    };
    u32::try_from(n)
        .ok()
        .filter(|n| (1..=MAX_TRUSTED_EXPIRES_IN_DAYS).contains(n))
}

/// Whether a string is an RFC 6749 scope: 1 to 128 bytes of visible ASCII with no space, quote or
/// backslash (the rule of ADR 0046's `oauth2` scopes).
fn usable_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= MAX_TRUSTED_SCOPE_BYTES
        && scope
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b) && b != b'"' && b != b'\\')
}

/// `data` of a `trusted_request` event: the component an agent asked the client to draw, and the
/// hints it may show. The request is data; the client draws it, and the agent never sees a secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedRequestData {
    /// The component asked for.
    pub component: TrustedComponent,
    /// The agent's suggestions, bounded and validated.
    pub hints: TrustedHints,
}

impl TrustedRequestData {
    /// Reads a message's `metadata[trusted-components/v1]` strictly: `{component, hints}` with a
    /// component this build draws and hints within bounds. Unknown members are ignored; anything
    /// else is `None`, and the request is then not read at all (fail closed).
    pub fn from_metadata_value(value: &Value) -> Option<TrustedRequestData> {
        let object = value.as_object()?;
        let component = TrustedComponent::from_name(object.get("component")?.as_str()?)?;
        let hints = TrustedHints::from_value(object.get("hints")?)?;
        let request = TrustedRequestData { component, hints };
        request.check().ok()?;
        Some(request)
    }

    /// Checks the hints again at the core's door.
    ///
    /// # Errors
    /// [`TrustedHintsInvalid`] for a hint out of bounds.
    pub fn check(&self) -> Result<(), TrustedHintsInvalid> {
        self.hints.check()
    }
}

/// The `params` of a card's `trusted-components/v1` entry, well formed: the components the agent
/// may ask for that this build can draw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedComponentsOffer {
    components: Vec<TrustedComponent>,
}

impl TrustedComponentsOffer {
    /// Reads the `params` of the card's entry: `components`, a non-empty array of strings. A name
    /// this build does not draw is ignored (the ADR lists five; this build draws one), a name
    /// listed twice keeps its first entry, and unknown members are ignored. `None` for anything
    /// else, and for a list that names no component this build draws: a card that offers nothing
    /// is not an offer.
    pub fn from_params(params: &Value) -> Option<TrustedComponentsOffer> {
        let listed = params.get("components")?.as_array()?;
        let mut components: Vec<TrustedComponent> = Vec::new();
        for entry in listed {
            let name = entry.as_str().filter(|name| !name.is_empty())?;
            let Some(component) = TrustedComponent::from_name(name) else {
                continue;
            };
            if !components.contains(&component) {
                components.push(component);
            }
        }
        if components.is_empty() {
            return None;
        }
        Some(TrustedComponentsOffer { components })
    }

    /// The components the agent may ask for, in the card's order.
    pub fn components(&self) -> &[TrustedComponent] {
        &self.components
    }

    /// Whether the card lets the agent ask for `component`.
    pub fn offers(&self, component: TrustedComponent) -> bool {
        self.components.contains(&component)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_the_exact_component_name_is_known() {
        assert_eq!(
            TrustedComponent::from_name("McpCredential"),
            Some(TrustedComponent::McpCredential)
        );
        for near in [
            "mcpCredential",
            "mcpcredential",
            "Mcp-Credential",
            " McpCredential",
            "ApiKeyCreate",
            "ModelServer",
            "SecretGenerate",
            "SecretCollect",
            "",
        ] {
            assert_eq!(TrustedComponent::from_name(near), None, "{near}");
        }
        assert_eq!(
            serde_json::to_value(TrustedComponent::McpCredential).unwrap(),
            json!("McpCredential")
        );
    }

    #[test]
    fn a_card_lists_the_components_it_may_ask_for() {
        let offer = TrustedComponentsOffer::from_params(&json!({
            "components": ["McpCredential", "ApiKeyCreate", "McpCredential"],
            "later": "ignored"
        }))
        .unwrap();
        assert_eq!(offer.components(), [TrustedComponent::McpCredential]);
        assert!(offer.offers(TrustedComponent::McpCredential));
        assert_eq!(
            serde_json::to_value(&offer).unwrap(),
            json!({"components": ["McpCredential"]})
        );
    }

    #[test]
    fn a_card_that_lists_only_components_this_build_cannot_draw_offers_nothing() {
        for params in [
            json!({"components": ["ApiKeyCreate"]}),
            json!({"components": ["ApiKeyCreate", "ModelServer"]}),
            json!({"components": []}),
            json!({}),
            json!({"components": "McpCredential"}),
            json!({"components": [""]}),
            json!({"components": ["McpCredential", 3]}),
            json!([]),
            json!(null),
        ] {
            assert_eq!(
                TrustedComponentsOffer::from_params(&params),
                None,
                "{params}"
            );
        }
    }

    #[test]
    fn well_formed_hints_are_read_and_serialised_as_the_contract_has_them() {
        let hints = TrustedHints::from_value(&json!({
            "name": "My search",
            "scopes": ["search.read", "search.write"],
            "expiresInDays": 30,
            "later": "ignored"
        }))
        .unwrap();
        assert_eq!(hints.name.as_deref(), Some("My search"));
        assert_eq!(hints.scopes, ["search.read", "search.write"]);
        assert_eq!(hints.expires_in_days, Some(30));
        assert_eq!(
            serde_json::to_value(&hints).unwrap(),
            json!({"name": "My search", "scopes": ["search.read", "search.write"], "expiresInDays": 30})
        );
    }

    #[test]
    fn empty_hints_are_an_empty_object() {
        let hints = TrustedHints::from_value(&json!({})).unwrap();
        assert_eq!(hints, TrustedHints::default());
        assert_eq!(serde_json::to_value(&hints).unwrap(), json!({}));
        // null members read as absent
        let hints = TrustedHints::from_value(&json!({
            "name": null, "scopes": null, "expiresInDays": null
        }))
        .unwrap();
        assert_eq!(hints, TrustedHints::default());
    }

    #[test]
    fn a_whole_double_is_a_whole_number_of_days() {
        let hints = TrustedHints::from_value(&json!({"expiresInDays": 30.0})).unwrap();
        assert_eq!(hints.expires_in_days, Some(30));
    }

    #[test]
    fn hints_out_of_bounds_are_refused() {
        for bad in [
            json!({"name": ""}),
            json!({"name": "   "}),
            json!({"name": "two\nlines"}),
            json!({"name": "x".repeat(MAX_TRUSTED_NAME_CHARS + 1)}),
            json!({"name": 3}),
            json!({"scopes": "search"}),
            json!({"scopes": [""]}),
            json!({"scopes": ["has space"]}),
            json!({"scopes": ["has\"quote"]}),
            json!({"scopes": ["has\\backslash"]}),
            json!({"scopes": [3]}),
            json!({"scopes": (0..=MAX_TRUSTED_SCOPES).map(|i| format!("s{i}")).collect::<Vec<_>>()}),
            json!({"expiresInDays": 0}),
            json!({"expiresInDays": 3651}),
            json!({"expiresInDays": 1.5}),
            json!({"expiresInDays": "30"}),
            json!([]),
            json!("hints"),
        ] {
            assert_eq!(TrustedHints::from_value(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_request_needs_a_known_component_and_hints() {
        let request = TrustedRequestData::from_metadata_value(&json!({
            "component": "McpCredential",
            "hints": {"name": "My search"},
            "later": "ignored"
        }))
        .unwrap();
        assert_eq!(request.component, TrustedComponent::McpCredential);
        assert_eq!(request.hints.name.as_deref(), Some("My search"));
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            json!({"component": "McpCredential", "hints": {"name": "My search"}})
        );
        for bad in [
            json!({}),
            json!({"hints": {}}),
            json!({"component": "ApiKeyCreate", "hints": {}}),
            json!({"component": "McpCredential"}),
            json!({"component": "McpCredential", "hints": []}),
            json!({"component": "McpCredential", "hints": {"scopes": ["bad scope"]}}),
            json!({"component": 3, "hints": {}}),
            json!([]),
            json!("request"),
        ] {
            assert_eq!(TrustedRequestData::from_metadata_value(&bad), None, "{bad}");
        }
    }
}
