//! The release-channels extension (ADR 0008): reading it from a card, and its wire keys.
//!
//! Contract: `another-agentic-platform/docs/extensions/release-channels-v1.md`. Capability
//! detection is strict and fails closed: anything that does not look exactly like the v1
//! parameters yields `None` (no release picker), never a guess.

use std::collections::BTreeMap;

use a2a::AgentCard;
use orch_core::Releases;
use serde_json::{Map, Value};

/// URI of the release-channels v1 extension.
pub const RELEASE_CHANNELS_URI: &str =
    "https://agents.vymalo.com/a2a/extensions/release-channels/v1";

/// Reads the release channels a live card advertises. `None` when the card does not declare
/// the extension, or declares it with parameters that do not parse (a warning is logged).
pub fn releases_from_card(card: &AgentCard) -> Option<Releases> {
    let ext = card
        .capabilities
        .extensions
        .as_ref()?
        .iter()
        .find(|e| e.uri == RELEASE_CHANNELS_URI)?;
    let params = ext.params.as_ref()?;
    match parse_params(params) {
        Ok(r) => Some(r),
        Err(why) => {
            tracing::warn!(
                card = %card.name,
                reason = why,
                "release-channels extension parameters are malformed; offering no releases"
            );
            None
        }
    }
}

fn parse_params(
    params: &std::collections::HashMap<String, Value>,
) -> Result<Releases, &'static str> {
    let default_channel = params
        .get("defaultChannel")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("defaultChannel is missing or not a string")?
        .to_owned();
    let channels_obj = params
        .get("channels")
        .and_then(Value::as_object)
        .ok_or("channels is missing or not an object")?;
    let channels = channels(channels_obj)?;
    if !channels.contains_key(&default_channel) {
        return Err("channels does not contain the default channel");
    }
    let revisions = match params.get("revisions") {
        None | Some(Value::Null) => None,
        Some(Value::Array(items)) => Some(revisions(items)?),
        Some(_) => return Err("revisions is not an array"),
    };
    Ok(Releases {
        default_channel,
        channels,
        revisions,
    })
}

fn channels(obj: &Map<String, Value>) -> Result<BTreeMap<String, String>, &'static str> {
    obj.iter()
        .map(|(name, rev)| {
            rev.as_str()
                .filter(|r| !r.is_empty())
                .map(|r| (name.clone(), r.to_owned()))
                .ok_or("a channel does not map to a revision name")
        })
        .collect()
}

/// `revisions` is `[{name, createdAt}]` in v1; a bare string list is accepted too.
fn revisions(items: &[Value]) -> Result<Vec<String>, &'static str> {
    items
        .iter()
        .map(|item| {
            match item {
                Value::String(s) => Some(s.as_str()),
                Value::Object(o) => o.get("name").and_then(Value::as_str),
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => None,
            }
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .ok_or("a revision has no name")
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::HashMap;

    use a2a::{AgentCapabilities, AgentExtension};
    use serde_json::json;

    use super::*;

    fn card_with(ext: Option<AgentExtension>) -> AgentCard {
        AgentCard {
            name: "coder".into(),
            description: "d".into(),
            version: "1".into(),
            supported_interfaces: vec![],
            capabilities: AgentCapabilities {
                extensions: ext.map(|e| vec![e]),
                ..AgentCapabilities::default()
            },
            default_input_modes: vec![],
            default_output_modes: vec![],
            skills: vec![],
            provider: None,
            documentation_url: None,
            icon_url: None,
            security_schemes: None,
            security_requirements: None,
            signatures: None,
        }
    }

    fn ext(uri: &str, params: Value) -> AgentExtension {
        let params: HashMap<String, Value> = serde_json::from_value(params).unwrap_or_default();
        AgentExtension {
            uri: uri.to_owned(),
            description: None,
            required: None,
            params: Some(params),
        }
    }

    fn spec_params() -> Value {
        json!({
            "service": "coder",
            "defaultChannel": "production",
            "channels": {"production": "coder-r47", "staging": "coder-r51", "latest": "coder-r53"},
            "revisions": [
                {"name": "coder-r53", "createdAt": "2026-09-28T09:10:00Z"},
                {"name": "coder-r51", "createdAt": "2026-09-26T14:02:00Z"},
                {"name": "coder-r47", "createdAt": "2026-09-20T08:45:00Z"}
            ]
        })
    }

    #[test]
    fn parses_the_spec_example() {
        let card = card_with(Some(ext(RELEASE_CHANNELS_URI, spec_params())));
        let r = releases_from_card(&card).unwrap();
        assert_eq!(r.default_channel, "production");
        assert_eq!(r.channels["staging"], "coder-r51");
        assert_eq!(
            r.revisions.unwrap(),
            vec!["coder-r53", "coder-r51", "coder-r47"]
        );
    }

    #[test]
    fn accepts_plain_string_revisions_and_missing_revisions() {
        let mut p = spec_params();
        p["revisions"] = json!(["a", "b"]);
        let r = releases_from_card(&card_with(Some(ext(RELEASE_CHANNELS_URI, p.clone())))).unwrap();
        assert_eq!(r.revisions.unwrap(), vec!["a", "b"]);
        p.as_object_mut().unwrap().remove("revisions");
        let r = releases_from_card(&card_with(Some(ext(RELEASE_CHANNELS_URI, p)))).unwrap();
        assert!(r.revisions.is_none());
    }

    #[test]
    fn no_extension_or_another_version_means_no_releases() {
        assert!(releases_from_card(&card_with(None)).is_none());
        let v2 = "https://agents.vymalo.com/a2a/extensions/release-channels/v2";
        assert!(releases_from_card(&card_with(Some(ext(v2, spec_params())))).is_none());
    }

    #[test]
    fn malformed_parameters_fail_closed() {
        let cases = [
            json!({}),
            json!({"defaultChannel": "p", "channels": "nope"}),
            json!({"defaultChannel": "p", "channels": {"p": 1}}),
            json!({"defaultChannel": "p", "channels": {"q": "r"}}),
            json!({"defaultChannel": "", "channels": {"": "r"}}),
            json!({"defaultChannel": "p", "channels": {"p": "r"}, "revisions": 3}),
            json!({"defaultChannel": "p", "channels": {"p": "r"}, "revisions": [{"createdAt": "x"}]}),
        ];
        for params in cases {
            let card = card_with(Some(ext(RELEASE_CHANNELS_URI, params.clone())));
            assert!(releases_from_card(&card).is_none(), "{params}");
        }
        let mut no_params = ext(RELEASE_CHANNELS_URI, json!({}));
        no_params.params = None;
        assert!(releases_from_card(&card_with(Some(no_params))).is_none());
    }
}
