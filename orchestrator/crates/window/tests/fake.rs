//! The testkit on the fake adapter.
#![allow(clippy::unwrap_used)]
use orch_core::ThreadId;
use orch_window::testkit::FakeWindow;
use orch_window::{WindowId, window_conformance};

fn make() -> FakeWindow {
    FakeWindow::new()
}

fn expire(w: &FakeWindow, t: ThreadId, id: &WindowId) {
    w.expire(t, id);
}

window_conformance!(make, expire = expire);

#[test]
fn a_name_resolving_to_a_private_address_is_refused_and_a_public_one_is_not() {
    use orch_window::check_url;
    let private = ["10.0.0.5".parse().unwrap()];
    let public = ["93.184.216.34".parse().unwrap()];
    let mixed = [
        "93.184.216.34".parse().unwrap(),
        "127.0.0.1".parse().unwrap(),
    ];
    assert!(check_url("http://app.example/", &private).is_err());
    assert!(check_url("http://app.example/", &mixed).is_err());
    assert!(check_url("http://app.example/", &[]).is_err());
    assert!(check_url("http://app.example/", &public).is_ok());
}

/// The guard's cases are seen red: with the guard removed from the fake, they fail.
#[tokio::test]
#[should_panic(expected = "must be refused")]
async fn the_private_address_cases_fail_without_the_guard() {
    orch_window::testkit::cases::private_loopback_link_local_and_metadata_addresses_are_refused(
        FakeWindow::unguarded(),
    )
    .await;
}
