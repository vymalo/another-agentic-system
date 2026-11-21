//! The vendored A2UI v1.0 schemas as the oracle of the A2UI goldens (ADR 0075, decision 9).
//!
//! The schemas under `tests/a2ui-v1_0/` are byte-for-byte copies of `a2ui-project/a2ui` at
//! `b4f91c2` (`tests/a2ui-v1_0/UPSTREAM` names the commit and every file's SHA-256, and the first
//! test checks them). They are read here, never at run time by the orchestrator: the orchestrator
//! relays A2UI and does not interpret it (ADR 0013), so the schemas are a test's oracle and nothing
//! more.
//!
//! What is checked:
//!
//! - every operation of every A2UI golden (`docs/api/examples/*.events.json`) validates against
//!   `agent_to_renderer.json`, with `catalog.json#/$defs/anyComponent` — a document the repository
//!   does not publish — standing for the catalog each surface's `createSurface` names;
//! - every `ui_action` of the goldens, as the message the agent receives, validates against
//!   `renderer_to_agent.json`;
//! - catalog 9 of [`docs/api/ui-catalog-v2.md`](../../../../docs/api/ui-catalog-v2.md) validates
//!   against `catalog_definition.json` and hashes to the digest the page pins;
//! - the renderer's capabilities, with catalog 9 inline, validate against
//!   `renderer_capabilities.json`, and a card's `params` against
//!   `agent_capabilities.json#/properties/1.0`.
//!
//! **Where the files live.** ADR 0075, decision 9 says `orchestrator/crates/core/tests/`. They are
//! here instead: what they validate — the goldens, the capabilities document, the card — is
//! `orch-e2e`'s, and `crates/core` was held by another slice when this landed. Moving the directory
//! is a `git mv` and this file's `dir()`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use jsonschema::{Registry, Resource};
use orch_core::{UiCatalogData, canonical_json, catalog_digest};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The directory the vendored schemas and their `UPSTREAM` record live in.
fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/a2ui-v1_0")
}

fn schema(name: &str) -> Value {
    let path = dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The id of the catalog the web renders, whose components the surfaces that name it use.
const OUR_CATALOG_ID: &str = "https://agents.vymalo.com/a2ui/catalogs/chat";
/// The id of A2UI's basic catalog.
const BASIC_CATALOG_ID: &str = "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json";
/// The URI `agent_to_renderer.json` reaches a catalog through: `catalog.json#/$defs/anyComponent`.
const CATALOG_SCHEMA_URI: &str = "https://a2ui.org/specification/v1_0/catalog.json";

/// The web's catalog, as the surfaces that name it are drawn by.
fn our_catalog() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../web/src/features/chat/lib/a2ui/catalog/catalog.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

/// A `catalog.json` document — the one upstream does not publish — as a renderer resolves the
/// references `agent_to_renderer.json` and `common_types.json` make for the catalog in use: its
/// `anyComponent` is the union of the catalog's components, and its `anyFunction` matches no call
/// (the web's catalog declares no function, ADR 0075 decision 8).
///
/// A2UI's own basic catalog is registered whole instead — its `$defs/anyFunction` points into its
/// own `functions`, so lifting a part of it out would leave a dangling reference.
fn our_catalog_document() -> Value {
    let catalog = our_catalog();
    let mut one_of: Vec<Value> = Vec::new();
    for (_, schema) in catalog["components"]
        .as_object()
        .expect("components")
        .iter()
    {
        one_of.push(schema.clone());
    }
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": CATALOG_SCHEMA_URI,
        "$defs": {
            "anyComponent": {"oneOf": one_of},
            "anyFunction": {"not": {}},
        },
    })
}

/// A validator for `document`, offline (nothing is ever fetched) and with the A2UI documents in a
/// registry, so that a `$ref` to a sibling file resolves to the vendored copy.
fn validator_for(document: &Value, extra: &[(&str, Value)]) -> jsonschema::Validator {
    let mut resources: BTreeMap<String, Value> = [
        "agent_to_renderer.json",
        "renderer_to_agent.json",
        "common_types.json",
        "agent_capabilities.json",
        "renderer_capabilities.json",
        "catalog_definition.json",
    ]
    .into_iter()
    .map(|name| {
        let document = schema(name);
        let id = document["$id"]
            .as_str()
            .expect("every A2UI schema names itself")
            .to_owned();
        (id, document)
    })
    .collect();
    // `agent_to_renderer.json` reaches a catalog through `catalog.json`, which upstream does not
    // publish: a validator that does not say which catalog it draws gets A2UI's basic one, so that
    // the registry resolves every reference it holds. It is registered under both URIs it is
    // reached by: the alias the message schema uses, and its own `$id`, which its `$defs` use.
    if !extra.iter().any(|(uri, _)| *uri == CATALOG_SCHEMA_URI) {
        let basic = schema("basic-catalog.json");
        resources.insert(CATALOG_SCHEMA_URI.to_owned(), basic.clone());
        resources.insert(BASIC_CATALOG_ID.to_owned(), basic);
    }
    // A catalog refers to its siblings beside it (`common_types.json`), and A2UI's basic catalog
    // sits under `catalogs/basic/`: the document is registered at both addresses.
    resources.insert(
        "https://a2ui.org/specification/v1_0/catalogs/basic/common_types.json".to_owned(),
        schema("common_types.json"),
    );
    for (uri, document) in extra {
        resources.insert((*uri).to_owned(), document.clone());
    }
    let mut registry = Registry::new();
    for (uri, document) in resources {
        registry = registry
            .add(uri.as_str(), Resource::from_contents(document))
            .unwrap_or_else(|e| panic!("{uri}: {e}"));
    }
    let registry = registry
        .prepare()
        .unwrap_or_else(|e| panic!("the registry: {e}"));
    jsonschema::options()
        .with_registry(&registry)
        .offline()
        .build(document)
        .unwrap_or_else(|e| panic!("the schema compiles: {e}"))
}

/// The errors of `instance` against `validator`, as text a failure can show.
fn errors(validator: &jsonschema::Validator, instance: &Value) -> Vec<String> {
    validator
        .iter_errors(instance)
        .map(|e| format!("{}: {e}", e.instance_path()))
        .collect()
}

// ------------------------------------------------------------------ the vendored bytes

/// `tests/a2ui-v1_0/UPSTREAM` names a commit and each file's SHA-256; the bytes here are those
/// bytes. A file that drifts from the record fails before anything is validated against it.
#[test]
fn the_vendored_schemas_are_the_ones_the_record_pins() {
    let text = std::fs::read_to_string(dir().join("UPSTREAM")).expect("UPSTREAM");
    let mut pinned = BTreeMap::new();
    let mut commit = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("commit=") {
            commit = Some(value.to_owned());
        }
        let Some((name, hash)) = line.split_once(' ') else {
            continue;
        };
        if name.ends_with(".json") {
            pinned.insert(name.to_owned(), hash.trim().to_owned());
        }
    }
    assert_eq!(
        commit.as_deref(),
        Some("b4f91c26fcf62abc0720c21a65840220941d0a3f"),
        "ADR 0075, decision 9 pins this commit"
    );
    assert_eq!(pinned.len(), 9, "nine files are vendored: {pinned:?}");
    for (name, want) in pinned {
        let bytes = std::fs::read(dir().join(&name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let got = format!("{:x}", Sha256::digest(&bytes));
        assert_eq!(got, want, "{name} is not the file UPSTREAM pins");
        // and it is a JSON document of the draft the contract names
        let document: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            document["$schema"],
            json!("https://json-schema.org/draft/2020-12/schema"),
            "{name}"
        );
    }
}

// ------------------------------------------------------------------ the goldens

/// Every `*.events.json` of `docs/api/examples`, oldest first as the log holds it.
fn golden_logs() -> Vec<(PathBuf, Vec<Value>)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/examples");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("the examples") {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if !path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".events.json"))
        {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let events: Vec<Value> =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        out.push((path, events));
    }
    assert!(!out.is_empty(), "no golden log was found");
    out
}

/// The operations of the goldens, each with the catalog its surface names.
fn operations() -> Vec<(String, Value, String)> {
    let mut out = Vec::new();
    for (path, events) in golden_logs() {
        // the catalog of a surface, from its `createSurface`
        let mut catalogs: BTreeMap<String, String> = BTreeMap::new();
        for event in &events {
            if event["kind"] != "ui_surface" {
                continue;
            }
            for op in event["data"]["operations"].as_array().into_iter().flatten() {
                if let Some(create) = op.get("createSurface")
                    && let Some(id) = create["surfaceId"].as_str()
                {
                    let catalog = create["catalogId"].as_str().unwrap_or(BASIC_CATALOG_ID);
                    catalogs.insert(id.to_owned(), catalog.to_owned());
                }
                let surface = op
                    .as_object()
                    .into_iter()
                    .flat_map(|o| o.values())
                    .find_map(|body| body.get("surfaceId"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let catalog = catalogs
                    .get(&surface)
                    .cloned()
                    .unwrap_or_else(|| BASIC_CATALOG_ID.to_owned());
                out.push((path.display().to_string(), op.clone(), catalog));
            }
        }
    }
    assert!(!out.is_empty(), "the goldens hold no A2UI operation");
    out
}

/// ADR 0075, decision 9: every golden's operations validate against `agent_to_renderer.json`.
/// `catalog.json` stands for the catalog each surface names — the basic catalog, or the web's —
/// so a component of another catalog is refused, as a renderer refuses it.
#[test]
fn every_operation_of_the_goldens_validates_against_agent_to_renderer() {
    let document = schema("agent_to_renderer.json");
    let basic = validator_for(
        &document,
        &[(CATALOG_SCHEMA_URI, schema("basic-catalog.json"))],
    );
    let ours = validator_for(&document, &[(CATALOG_SCHEMA_URI, our_catalog_document())]);
    let mut seen = 0;
    for (file, op, catalog) in operations() {
        let validator = match catalog.as_str() {
            BASIC_CATALOG_ID => &basic,
            OUR_CATALOG_ID => &ours,
            other => panic!("{file}: a surface names a catalog this test does not know: {other}"),
        };
        let found = errors(validator, &op);
        assert!(found.is_empty(), "{file}: {op}\n{found:#?}");
        seen += 1;
    }
    assert!(seen >= 10, "the goldens' operations were validated: {seen}");
}

/// ADR 0075, decision 4: the action the agent receives is the five-member v1.0 action. Every
/// `ui_action` of the goldens is checked in that shape — the same members the A2A adapter writes —
/// against `renderer_to_agent.json`.
#[test]
fn every_action_of_the_goldens_validates_against_renderer_to_agent() {
    let validator = validator_for(&schema("renderer_to_agent.json"), &[]);
    let mut seen = 0;
    for (path, events) in golden_logs() {
        let file = path.display();
        for event in events {
            if event["kind"] != "ui_action" {
                continue;
            }
            let data = &event["data"];
            let message = json!({
                "version": data["version"],
                "action": {
                    "name": data["name"],
                    "surfaceId": data["surfaceId"],
                    "sourceComponentId": data["sourceComponentId"],
                    // the log's own time is the action's time (docs/api/agui.md, "Actions")
                    "timestamp": event["at"],
                    "context": data["context"],
                }
            });
            assert_eq!(
                message["action"].as_object().map(|o| o.len()),
                Some(5),
                "{file}: exactly the five members"
            );
            let errors = errors(&validator, &message);
            assert!(errors.is_empty(), "{file}: {message}\n{errors:#?}");
            seen += 1;
        }
    }
    // `a2ui.events.json` holds the person's click; a golden that gains one is checked here too.
    assert!(seen >= 1, "no action of the goldens was validated");
}

// ------------------------------------------------------------------ catalog 9 of the contract

/// Catalog 9 as `docs/api/ui-catalog-v2.md` writes it: the fenced block tagged `json catalog-9`.
fn catalog_nine() -> UiCatalogData {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/ui-catalog-v2.md");
    let page = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let start = page
        .find("```json catalog-9\n")
        .expect("the page holds catalog 9")
        + "```json catalog-9\n".len();
    let end = start + page[start..].find("\n```").expect("the block ends");
    let catalog: Value = serde_json::from_str(&page[start..end]).expect("catalog 9 is JSON");
    UiCatalogData {
        catalog_id: catalog["catalogId"].as_str().expect("an id").to_owned(),
        version: 9,
        digest: catalog_digest(&catalog).expect("a catalog the orchestrator could hash"),
        catalog,
    }
}

/// ADR 0075, decision 9: catalog 9 validates against `catalog_definition.json`, and its digest is
/// the one the contract pins — the orchestrator's own canonicalisation, over the page's own bytes.
#[test]
fn catalog_nine_validates_against_catalog_definition_and_has_the_pinned_digest() {
    let catalog = catalog_nine();
    let validator = validator_for(&schema("catalog_definition.json"), &[]);
    let found = errors(&validator, &catalog.catalog);
    assert!(found.is_empty(), "{found:#?}");
    assert_eq!(catalog.catalog["protocolVersion"], json!("1.0"));
    assert_eq!(
        catalog.digest, "sha256:5f174ee91c50759d59a0d7e91e3a1dc2e7c110b82ee91d858f4a23a944ed72b2",
        "the digest of catalog 9 (ui-catalog-v2.md, section 2)"
    );
    // and the orchestrator would take it as the web sends it
    let sent = json!({
        "catalogId": catalog.catalog_id,
        "version": catalog.version,
        "digest": catalog.digest,
        "catalog": catalog.catalog,
    });
    assert_eq!(UiCatalogData::from_json(&sent).unwrap(), catalog);
    // taking `protocolVersion` out gives back catalog 8's digest (the page pins both)
    let mut without = catalog.catalog.clone();
    without.as_object_mut().unwrap().remove("protocolVersion");
    assert_eq!(
        catalog_digest(&without).unwrap(),
        "sha256:22c333102bed676791dc9d47f21fc0a39be6e5062ef2c52f5a599ecca1b8341d"
    );
    assert!(
        canonical_json(&catalog.catalog)
            .unwrap()
            .contains("\"protocolVersion\":\"1.0\"")
    );
}

// ------------------------------------------------------------------ the capabilities

/// The renderer's capabilities the orchestrator sends, with catalog 9 inline, validate against
/// `renderer_capabilities.json` — and the card's `params` against `agent_capabilities.json`.
#[test]
fn the_renderer_capabilities_and_a_cards_params_validate_against_their_schemas() {
    let catalog = catalog_nine();
    let delivery = orch_core::UiDelivery::Inline(catalog.clone());
    let (key, value) = orch_agent_a2a::renderer_capabilities(Some(&delivery), true);
    assert_eq!(key, "a2uiRendererCapabilities");
    let validator = validator_for(&schema("renderer_capabilities.json"), &[]);
    let found = errors(&validator, &value);
    assert!(found.is_empty(), "{value}\n{found:#?}");
    assert_eq!(
        value["1.0"]["supportedCatalogIds"][0],
        json!(OUR_CATALOG_ID),
        "ours first, then the basic catalog"
    );
    assert_eq!(
        value["1.0"]["inlineCatalogs"][0], catalog.catalog,
        "the whole document, inline"
    );

    // A card's A2UI entry: the inner `"1.0"` object (never the wrapper), validated on its own.
    let capabilities = schema("agent_capabilities.json");
    let params = json!({
        "supportedCatalogIds": [BASIC_CATALOG_ID],
        "acceptsInlineCatalogs": true,
    });
    let validator = validator_for(&capabilities["properties"]["1.0"], &[]);
    let found = errors(&validator, &params);
    assert!(found.is_empty(), "{params}\n{found:#?}");
}
