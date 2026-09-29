//! The vendored schema is the file the README says it is.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_agui_proto::SCHEMA_1_0;
use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn readme_records_the_sha256_of_the_vendored_schema() {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    let sha = hex(&Sha256::digest(SCHEMA_1_0.as_bytes()));
    assert!(
        readme.contains(&sha),
        "README.md must record the sha256 of schema/ag-ui-1.0.schema.json ({sha}); \
         update it together with the file, never one without the other"
    );
}

#[test]
fn the_vendored_schema_is_the_1_0_schema() {
    let schema: serde_json::Value = serde_json::from_str(SCHEMA_1_0).unwrap();
    assert_eq!(schema["$id"], "https://ag-ui.com/spec/1.0/schema.json");
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["$defs"].as_object().unwrap().len(), 98);
}
