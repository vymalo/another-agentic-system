//! `PlatformClientCertificates` conformance cases: what the provisioner relies on when it presents
//! the orchestrator's workload certificate on a TLS connection (ANR-809, ADR 0110).
//!
//! The port's contract is two sentences: an answer is a certificate that is **valid now**, with its
//! chain and its key, and **an error means no identity** (the caller sends nothing). An
//! implementation supplies a [`PlatformClientCertificatesFixture`]: the source under test, the clock
//! it judges "now" by (and a way to move it), how long a certificate it issues lives, and a way to
//! make the issuer behind it fail.

use std::time::Duration;

use jiff::Timestamp;
use orch_core::{Classify, ErrorClass};

use crate::{PlatformClientCertificate, PlatformClientCertificates, PlatformCredentialsError};

/// A source under test, with the knobs the cases move it by.
pub trait PlatformClientCertificatesFixture: Send + Sync + 'static {
    /// The source under test.
    type Source: PlatformClientCertificates;

    /// The source.
    fn source(&self) -> &Self::Source;

    /// "Now", on the clock the source judges validity by.
    fn now(&self) -> Timestamp;

    /// Moves that clock forward.
    fn advance(&self, by: Duration);

    /// How long a certificate lives from the moment it is issued.
    fn lifetime(&self) -> Duration;

    /// Makes the issuer behind the source fail with `error` (`Some`), or work again (`None`). A
    /// source that holds a certificate that is still valid may keep answering it while the issuer is
    /// down; it never answers one that is over.
    fn set_issuer_failing(&self, error: Option<PlatformCredentialsError>);
}

fn assert_valid_now<F: PlatformClientCertificatesFixture>(f: &F, cert: &PlatformClientCertificate) {
    assert!(
        cert.not_after() > f.now(),
        "the certificate answered is already over: not_after {} is not after now {}",
        cert.not_after(),
        f.now()
    );
    assert!(!cert.chain_der().is_empty(), "the certificate has no chain");
    assert!(
        cert.chain_der().iter().all(|der| !der.is_empty()),
        "the chain holds an empty certificate"
    );
    assert!(
        !cert.expose_key_pkcs8().is_empty(),
        "the certificate has no key"
    );
    assert!(!cert.serial().is_empty(), "the certificate has no serial");
}

/// A source that answers a certificate answers one that is valid now, with a chain, a key and a
/// serial, every time it is asked.
pub async fn a_certificate_is_valid_now_and_complete<F: PlatformClientCertificatesFixture>(f: F) {
    let source = f.source();
    let first = source.current().await.unwrap();
    assert_valid_now(&f, &first);
    let second = source.current().await.unwrap();
    assert_valid_now(&f, &second);
}

/// An error means no identity: with the issuer failing and nothing held, the answer is that error
/// and never a certificate, whatever kind of failure it is, and a failure does not poison the source
/// (when the issuer works again it answers a certificate). The errors are classified:
/// `Unavailable` is transient, `Refused` is a failure of authentication.
pub async fn an_error_means_no_identity_and_is_classified<F: PlatformClientCertificatesFixture>(
    f: F,
) {
    let source = f.source();
    f.set_issuer_failing(Some(PlatformCredentialsError::Unavailable));
    let error = source
        .current()
        .await
        .expect_err("an unavailable issuer answered a certificate");
    assert_eq!(error, PlatformCredentialsError::Unavailable);
    assert_eq!(error.class(), ErrorClass::Transient);

    f.set_issuer_failing(Some(PlatformCredentialsError::Refused));
    let error = source
        .current()
        .await
        .expect_err("a refusing issuer answered a certificate");
    assert_eq!(error, PlatformCredentialsError::Refused);
    assert_eq!(error.class(), ErrorClass::Unauthenticated);

    f.set_issuer_failing(None);
    let cert = source.current().await.unwrap();
    assert_valid_now(&f, &cert);
}

/// A certificate that has lapsed is never answered: with the issuer failing and the clock past the
/// end of the certificate's validity, the answer is an error; when the issuer is back, a
/// certificate that is valid now.
pub async fn a_certificate_that_has_lapsed_is_never_answered<
    F: PlatformClientCertificatesFixture,
>(
    f: F,
) {
    let source = f.source();
    let held = source.current().await.unwrap();
    assert_valid_now(&f, &held);

    f.set_issuer_failing(Some(PlatformCredentialsError::Unavailable));
    f.advance(f.lifetime() + Duration::from_secs(1));
    assert!(
        f.now() >= held.not_after(),
        "the clock did not pass the end of the certificate"
    );
    let error = source.current().await;
    assert!(
        error.is_err(),
        "a certificate that is over was answered: {:?}",
        error.map(|c| c.serial().to_owned())
    );

    f.set_issuer_failing(None);
    let renewed = source.current().await.unwrap();
    assert_valid_now(&f, &renewed);
}

/// A renewal is a new certificate: after the old one is over and the issuer works, the answer has
/// another serial (the provisioner rebuilds its client exactly when the serial changes) and another
/// key.
pub async fn a_renewal_is_a_new_certificate<F: PlatformClientCertificatesFixture>(f: F) {
    let source = f.source();
    let first = source.current().await.unwrap();
    f.advance(f.lifetime() + Duration::from_secs(1));
    let second = source.current().await.unwrap();
    assert_valid_now(&f, &second);
    assert_ne!(first.serial(), second.serial(), "a renewal kept the serial");
    assert_ne!(
        first.expose_key_pkcs8(),
        second.expose_key_pkcs8(),
        "a renewal kept the key"
    );
}

/// What a certificate shows when it is printed: its serial and its end, and nothing of its key.
pub async fn a_certificate_shows_no_key<F: PlatformClientCertificatesFixture>(f: F) {
    let cert = f.source().current().await.unwrap();
    let shown = format!("{cert:?}");
    assert!(
        shown.contains(cert.serial()),
        "the serial is not shown: {shown}"
    );
    assert!(!shown.contains("BEGIN"), "{shown}");
    assert!(!shown.contains("PRIVATE"), "{shown}");
    let key = cert.expose_key_pkcs8();
    let prefix = format!("{:?}", &key[..key.len().min(6)]);
    assert!(
        !shown.contains(prefix.trim_matches(['[', ']'])),
        "the key's bytes are in: {shown}"
    );
}
