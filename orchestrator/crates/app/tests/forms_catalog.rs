//! The forms catalog an asked agent is sent (ADR 0026, third note of 2026-11-14), derived from the
//! catalog the web really ships: it holds no `show`, in the schema or in the words that describe
//! it, and what is left still compiles and still takes a form of version 7.
//!
//! `UiCatalogData::forms_only` cuts a sentence of the description by its opening words, so these
//! tests read `web/src/features/chat/lib/a2ui/catalog/catalog.json` itself: a reworded sentence
//! fails here, in CI, and is not shipped as a schema that still invites `show`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::{UiCatalogData, catalog_digest};
use serde_json::{Value, json};

/// The one place the description may say the word: it is about the tool that shows a surface, not
/// about the question's `show` member.
const ALLOWED: &str = "a surface you show with show";

/// The catalog the web ships, as the `ui-catalog/v2` document a run carries (ADR 0075): the file
/// itself is still the v1 document — catalog 8, without `protocolVersion` — until the web's slice
/// moves it to 9, and these tests are about the `Choices` of its components, which are the same.
fn web_catalog() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../web/src/features/chat/lib/a2ui/catalog/catalog.json");
    let mut catalog: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    if catalog.get("protocolVersion").is_none() {
        catalog["protocolVersion"] = json!("1.0");
    }
    catalog
}

fn screen(catalog: Value, version: u64) -> UiCatalogData {
    let sent = json!({
        "catalogId": catalog["catalogId"],
        "version": version,
        "digest": catalog_digest(&catalog).unwrap(),
        "catalog": catalog,
    });
    UiCatalogData::from_json(&sent).unwrap()
}

/// Everything the forms catalog of `catalog` must be; the first rule it breaks, as text.
fn forms_catalog_is_honest(catalog: Value) -> Result<(), String> {
    let forms = screen(catalog, 9).forms_only().ok_or("no forms catalog")?;
    let choices = &forms.catalog["components"]["Choices"];
    let question = &choices["properties"]["questions"]["items"];
    if question["properties"].get("show").is_some() {
        return Err("the question's schema still has `show`".to_owned());
    }
    let description = choices["description"].as_str().ok_or("no description")?;
    let rest = description.replace(ALLOWED, "");
    if rest.contains("show") {
        return Err(format!("the description still says show: {description}"));
    }
    let schema = jsonschema::draft202012::new(choices).map_err(|e| e.to_string())?;
    let v7 = json!({
        "component": "Choices",
        "id": "ask",
        "title": "One thing",
        "questions": [{
            "id": "db",
            "question": "Which database?",
            "type": "single",
            "options": [{"value": "pg", "label": "Postgres"}, {"value": "my", "label": "MySQL"}],
        }, {
            "id": "go",
            "question": "Go ahead?",
            "type": "confirm",
        }],
    });
    if let Err(e) = schema.validate(&v7) {
        return Err(format!("a form of version 7 is refused: {e}"));
    }
    let mut shown = v7;
    shown["questions"][0]["show"] = json!(["chart"]);
    if schema.is_valid(&shown) {
        return Err("a question that names `show` is still valid".to_owned());
    }
    Ok(())
}

#[test]
fn the_forms_catalog_of_the_catalog_the_web_ships_does_not_offer_show() {
    forms_catalog_is_honest(web_catalog()).unwrap();
}

#[test]
fn a_reworded_sentence_is_caught_here_and_not_shipped() {
    let mut catalog = web_catalog();
    let description = catalog["components"]["Choices"]["description"]
        .as_str()
        .unwrap()
        .to_owned();
    let opening = " Only when the thread's catalog is version 8 or more: ";
    assert!(
        description.contains(opening),
        "the catalog no longer says what forms_only cuts by: change SHOW_SENTENCE_START with it"
    );
    catalog["components"]["Choices"]["description"] =
        json!(description.replace(opening, " From catalog version 8 on: "));
    let broken = forms_catalog_is_honest(catalog).unwrap_err();
    assert!(broken.contains("description still says show"), "{broken}");
}
