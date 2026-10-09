//! The desktop app of another-agentic-system (ADR 0047): the web's static export in a window, signing in as the public
//! client `another-agentic-desktop` through the system browser and a loopback listener (RFC 8252, [`loopback`]). The page
//! holds its DPoP key and tokens in the webview's IndexedDB as the browser web does (ADR 0054); this process holds nothing.

mod assets;
mod loopback;

/// Builds and runs the app.
///
/// # Panics
/// When the window cannot be created: there is nothing to show without it.
pub fn run() {
    let mut context = tauri::generate_context!();
    // a thread and a share link are one exported page each, whatever the id in the address (`assets.rs`)
    let embedded = context.set_assets(Box::new(assets::Empty));
    context.set_assets(Box::new(assets::Shells::new(embedded)));
    #[allow(clippy::expect_used)]
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(loopback::Loopback::default())
        .invoke_handler(tauri::generate_handler![
            loopback::loopback_listen,
            loopback::loopback_sign_in,
            loopback::loopback_cancel,
            loopback::open_in_browser,
        ])
        .run(context)
        .expect("the window could not be created");
}
