//! The `PlatformClientCertificates` testkit catches what it claims to: each fake below breaks
//! **one** rule of the port and the case of that rule must fail on it. The control fake, which
//! breaks nothing, passes every case, so a red case is the rule's and not the fake's.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use orch_ports::memory::FixedClock;
use orch_ports::testkit::platform_client_certificates as kit;
use orch_ports::{
    Clock, PlatformCertificateFuture, PlatformClientCertificate, PlatformClientCertificates,
    PlatformCredentialsError,
};
use zeroize::Zeroizing;

const LIFETIME: Duration = Duration::from_secs(86_400);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Break {
    /// Nothing: the control.
    Nothing,
    /// A certificate that is over is still answered when the issuer is down.
    ServesALapsedLeaf,
    /// With the issuer down and nothing held, a certificate is answered anyway.
    AnswersALeafOnError,
    /// A renewal keeps the serial and the key.
    ReusesTheSerialAndKey,
    /// A refusal is reported as an unavailable issuer.
    ReportsARefusalAsUnavailable,
    /// The certificate answered is already over.
    AnswersAnExpiredLeaf,
}

struct Fake {
    how: Break,
    clock: FixedClock,
    failing: Mutex<Option<PlatformCredentialsError>>,
    held: Mutex<Option<PlatformClientCertificate>>,
    issued: Mutex<u64>,
}

impl Fake {
    fn new(how: Break) -> Self {
        Fake {
            how,
            clock: FixedClock::new("2026-11-27T12:00:00Z".parse().unwrap()),
            failing: Mutex::new(None),
            held: Mutex::new(None),
            issued: Mutex::new(0),
        }
    }

    fn mint(&self, n: u64, not_after: Timestamp) -> PlatformClientCertificate {
        PlatformClientCertificate::new(
            vec![format!("leaf-{n}").into_bytes()],
            Zeroizing::new(format!("key-{n}-0123456789abcdef").into_bytes()),
            format!("{n:02x}"),
            not_after,
        )
    }

    fn answer(&self) -> Result<PlatformClientCertificate, PlatformCredentialsError> {
        let now = self.clock.now();
        let failing = *self.failing.lock().unwrap();
        let mut held = self.held.lock().unwrap();
        if let Some(cert) = held.as_ref() {
            let valid = now < cert.not_after();
            if self.how == Break::ServesALapsedLeaf && failing.is_some() {
                return Ok(cert.clone());
            }
            if valid && failing.is_none() && self.how != Break::ReusesTheSerialAndKey {
                return Ok(cert.clone());
            }
            if valid && failing.is_some() {
                return Ok(cert.clone());
            }
        }
        let mut issued = self.issued.lock().unwrap();
        if let Some(error) = failing {
            if self.how == Break::AnswersALeafOnError {
                return Ok(self.mint(99, now.checked_add(LIFETIME).unwrap()));
            }
            return Err(match (self.how, error) {
                (Break::ReportsARefusalAsUnavailable, PlatformCredentialsError::Refused) => {
                    PlatformCredentialsError::Unavailable
                }
                _ => error,
            });
        }
        // A renewal that keeps the serial and the key reissues the first certificate's identity.
        if self.how == Break::ReusesTheSerialAndKey && held.is_some() {
            let again = self.mint(1, now.checked_add(LIFETIME).unwrap());
            *held = Some(again.clone());
            return Ok(again);
        }
        *issued += 1;
        let not_after = if self.how == Break::AnswersAnExpiredLeaf {
            now.checked_sub(SignedDuration::from_secs(1)).unwrap()
        } else {
            now.checked_add(LIFETIME).unwrap()
        };
        let cert = self.mint(*issued, not_after);
        *held = Some(cert.clone());
        Ok(cert)
    }
}

impl PlatformClientCertificates for Fake {
    fn current(&self) -> PlatformCertificateFuture<'_> {
        let answer = self.answer();
        Box::pin(async move { answer })
    }
}

struct Fixture(Fake);

impl kit::PlatformClientCertificatesFixture for Fixture {
    type Source = Fake;

    fn source(&self) -> &Fake {
        &self.0
    }

    fn now(&self) -> Timestamp {
        self.0.clock.now()
    }

    fn advance(&self, by: Duration) {
        self.0.clock.advance(by);
    }

    fn lifetime(&self) -> Duration {
        LIFETIME
    }

    fn set_issuer_failing(&self, error: Option<PlatformCredentialsError>) {
        *self.0.failing.lock().unwrap() = error;
    }
}

fn fixture(how: Break) -> Fixture {
    Fixture(Fake::new(how))
}

/// Whether the case panicked (its assertion failed) on the fake.
async fn goes_red<Fut>(case: Fut) -> bool
where
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(case).await.is_err()
}

#[tokio::test]
async fn the_control_fake_passes_every_case() {
    let b = Break::Nothing;
    kit::a_certificate_is_valid_now_and_complete(fixture(b)).await;
    kit::an_error_means_no_identity_and_is_classified(fixture(b)).await;
    kit::a_certificate_that_has_lapsed_is_never_answered(fixture(b)).await;
    kit::a_renewal_is_a_new_certificate(fixture(b)).await;
    kit::a_certificate_shows_no_key(fixture(b)).await;
}

#[tokio::test]
async fn a_fake_that_serves_a_lapsed_leaf_fails_the_lapsed_case() {
    let f = fixture(Break::ServesALapsedLeaf);
    assert!(goes_red(kit::a_certificate_that_has_lapsed_is_never_answered(f)).await);
}

#[tokio::test]
async fn a_fake_that_answers_a_leaf_on_error_fails_the_no_identity_case() {
    let f = fixture(Break::AnswersALeafOnError);
    assert!(goes_red(kit::an_error_means_no_identity_and_is_classified(f)).await);
}

#[tokio::test]
async fn a_fake_that_reuses_the_serial_and_key_fails_the_renewal_case() {
    let f = fixture(Break::ReusesTheSerialAndKey);
    assert!(goes_red(kit::a_renewal_is_a_new_certificate(f)).await);
}

#[tokio::test]
async fn a_fake_that_reports_a_refusal_as_unavailable_fails_the_classification_case() {
    let f = fixture(Break::ReportsARefusalAsUnavailable);
    assert!(goes_red(kit::an_error_means_no_identity_and_is_classified(f)).await);
}

#[tokio::test]
async fn a_fake_that_answers_an_expired_leaf_fails_the_valid_now_case() {
    let f = fixture(Break::AnswersAnExpiredLeaf);
    assert!(goes_red(kit::a_certificate_is_valid_now_and_complete(f)).await);
}
