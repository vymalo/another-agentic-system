//! The client's own settings, with no server: a CA bundle is read, a bad one is refused, and no
//! error holds the token or the bundle.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use orch_secrets_openbao::{OpenBaoConfig, OpenBaoSecretStore};
use secrecy::SecretString;

/// A self-signed certificate made for this test (its key was not kept).
const CA: &str = r"
-----BEGIN CERTIFICATE-----
MIIBfDCCASGgAwIBAgIUOu5su8hzTpxDw/r9EjaEBlpHShYwCgYIKoZIzj0EAwIw
EjEQMA4GA1UEAwwHdGVzdC1jYTAgFw0yNjExMjUwMTI5MzBaGA8yMTI2MTEwMTAx
MjkzMFowEjEQMA4GA1UEAwwHdGVzdC1jYTBZMBMGByqGSM49AgEGCCqGSM49AwEH
A0IABDsk0c3xJ89jLQiYvWRkYZ2Qpu4ZSWsY5BaYUSLGPGNSOdnfaHRtVO/M01S3
JiX9yF7g6ACtvVyo7QF8qXe2d/CjUzBRMB0GA1UdDgQWBBR3Pzh2ZR7wHJUbrax/
N+rljlqL3jAfBgNVHSMEGDAWgBR3Pzh2ZR7wHJUbrax/N+rljlqL3jAPBgNVHRMB
Af8EBTADAQH/MAoGCCqGSM49BAMCA0kAMEYCIQDPB9nv2grcEE7LwL06u+JlG7z3
t+JGCjV/azNKJGnrQQIhANpEF/j1t0zZnpLh2i7Li19zBInwg0djitpksoqRHgLW
-----END CERTIFICATE-----
";

const TOKEN: &str = "hvs.NEVER-IN-AN-ERROR-0123456789";

fn config(ca_pem: Option<&str>) -> OpenBaoConfig {
    OpenBaoConfig {
        address: "https://bao.invalid:8200".to_owned(),
        mount: "secret".to_owned(),
        prefix: "orchestrator".to_owned(),
        token: SecretString::from(TOKEN),
        timeout: Duration::from_secs(1),
        ca_pem: ca_pem.map(str::to_owned),
    }
}

#[test]
fn a_ca_bundle_is_accepted_and_none_is_too() {
    assert!(OpenBaoSecretStore::new(config(None)).is_ok());
    assert!(OpenBaoSecretStore::new(config(Some(CA))).is_ok());
}

#[test]
fn a_bundle_that_is_not_pem_is_refused_and_the_error_holds_neither_it_nor_the_token() {
    let junk = "-----BEGIN CERTIFICATE-----\nnot base64 at all %%%\n-----END CERTIFICATE-----\n";
    let error = OpenBaoSecretStore::new(config(Some(junk))).unwrap_err();
    let shown = format!("{error} {error:?}");
    assert!(
        !shown.contains(TOKEN) && !shown.contains("not base64") && !shown.contains("bao.invalid"),
        "{shown}"
    );
}

#[test]
fn the_store_never_prints_its_token() {
    let store = OpenBaoSecretStore::new(config(None)).unwrap();
    assert!(!format!("{store:?}").contains(TOKEN));
}
