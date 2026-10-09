//! The deployment the app is built for (`../deployment.json`, or `AGENTIC_ISSUER_ORIGIN`): the issuer's origin is the one
//! address `loopback.rs` opens in the browser. `scripts/build-web.mjs` reads the same file for the API's origin, the web's
//! `config.json` and the content security policy, so the three cannot disagree.

fn main() {
    println!("cargo:rerun-if-changed=../deployment.json");
    println!("cargo:rerun-if-env-changed=AGENTIC_ISSUER_ORIGIN");
    let issuer = std::env::var("AGENTIC_ISSUER_ORIGIN")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| from_deployment("issuerOrigin"));
    println!("cargo:rustc-env=AGENTIC_ISSUER_ORIGIN={issuer}");
    tauri_build::build();
}

/// A string member of `../deployment.json`. A build without it cannot know where it signs in, so it stops.
#[allow(clippy::expect_used)]
fn from_deployment(key: &str) -> String {
    let text = std::fs::read_to_string("../deployment.json").expect("apps/tauri/deployment.json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("deployment.json is JSON");
    json.get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .expect("deployment.json names the issuer's origin (issuerOrigin)")
}
