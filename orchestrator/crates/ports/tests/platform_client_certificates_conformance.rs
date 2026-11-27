//! Runs the `PlatformClientCertificates` testkit against the in-memory source.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use jiff::Timestamp;
use orch_ports::PlatformCredentialsError;
use orch_ports::memory::{FixedClock, MemoryPlatformClientCertificates};
use orch_ports::testkit::platform_client_certificates::PlatformClientCertificatesFixture;

const LIFETIME: Duration = Duration::from_secs(86_400);

struct Memory {
    clock: FixedClock,
    source: MemoryPlatformClientCertificates,
}

impl PlatformClientCertificatesFixture for Memory {
    type Source = MemoryPlatformClientCertificates;

    fn source(&self) -> &MemoryPlatformClientCertificates {
        &self.source
    }

    fn now(&self) -> Timestamp {
        orch_ports::Clock::now(&self.clock)
    }

    fn advance(&self, by: Duration) {
        self.clock.advance(by);
    }

    fn lifetime(&self) -> Duration {
        LIFETIME
    }

    fn set_issuer_failing(&self, error: Option<PlatformCredentialsError>) {
        self.source.set_failing(error);
    }
}

async fn make() -> Option<Memory> {
    let clock = FixedClock::new("2026-11-27T12:00:00Z".parse().unwrap());
    Some(Memory {
        source: MemoryPlatformClientCertificates::new(clock.clone(), LIFETIME),
        clock,
    })
}

orch_ports::platform_client_certificates_conformance!(make);
