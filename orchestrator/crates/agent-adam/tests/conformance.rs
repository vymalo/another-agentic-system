//! The `AgentClient` conformance suite against the local-agent client, on the in-memory journal
//! and on Postgres (skipped without `ORCH_TEST_DATABASE_URL`), with the scripted adam-rs agent
//! of the testkit behind it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_agent_adam::testkit::LocalFixture;

mod memory {
    use super::*;

    async fn make() -> Option<LocalFixture> {
        Some(LocalFixture::memory().await)
    }

    orch_ports::agent_client_conformance!(make);
}

mod postgres {
    use super::*;

    async fn make() -> Option<LocalFixture> {
        let fixture = LocalFixture::postgres().await;
        if fixture.is_none() {
            eprintln!("skipping the Postgres variant: ORCH_TEST_DATABASE_URL is not set");
        }
        fixture
    }

    orch_ports::agent_client_conformance!(make);
}
