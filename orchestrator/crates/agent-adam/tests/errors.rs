//! What a [`LocalAgentsError`] keeps of a database's failure: never a connection string, and never
//! the server's `DETAIL`, which may quote a row. The server's case is skipped unless
//! `ORCH_TEST_DATABASE_URL` is set; the others need no database.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_agent_adam::{LocalAgents, LocalAgentsError, LocalKind, LocalOptions};
use orch_core::{Classify as _, ErrorClass, report};
use orch_pg_connect::PgOptions;

const ROW: &str = "a-row-nobody-may-read";

#[tokio::test]
async fn a_servers_detail_is_never_kept() {
    let Some(url) = std::env::var("ORCH_TEST_DATABASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
    else {
        eprintln!("skipped: ORCH_TEST_DATABASE_URL is not set");
        return;
    };
    let client = url.parse::<PgOptions>().unwrap().connect().await.unwrap();
    client
        .batch_execute("CREATE TEMP TABLE kept (v text PRIMARY KEY)")
        .await
        .unwrap();
    let insert = format!("INSERT INTO kept VALUES ('{ROW}')");
    client.batch_execute(&insert).await.unwrap();
    let driver = client.batch_execute(&insert).await.unwrap_err();
    assert!(
        report(&driver).contains(ROW),
        "the driver's own chain quotes the row in its DETAIL: {}",
        report(&driver)
    );

    let kept = LocalAgentsError::unavailable(driver);
    let said = report(&kept);
    assert!(
        said.contains("duplicate key value violates unique constraint"),
        "{said}"
    );
    assert!(said.contains("SQLSTATE 23505"), "{said}");
    assert!(!said.contains(ROW), "{said}");
    assert!(!format!("{kept:?}").contains(ROW), "{kept:?}");
    assert_eq!(kept.class(), ErrorClass::Transient);
}

#[tokio::test]
async fn a_connection_string_is_never_kept() {
    let opts = || LocalOptions::new("errors-test");
    for url in [
        // not a connection string
        "postgres-ish://u:hunter2@[not-a-host",
        // nothing listens on port 1
        "postgres://u:hunter2@127.0.0.1:1/db",
    ] {
        let err = LocalAgents::connect(url, None, opts(), &[LocalKind::Echo])
            .await
            .unwrap_err();
        assert_eq!(err.class(), ErrorClass::Transient, "{url}");
        for text in [report(&err), format!("{err:?}")] {
            assert!(!text.contains("hunter2"), "{url}: {text}");
            assert!(!text.contains("127.0.0.1:1"), "{url}: {text}");
        }
        let direct = LocalAgents::connect(
            "postgres://u:hunter2@127.0.0.1:1/db",
            Some(url),
            opts(),
            &[LocalKind::Echo],
        )
        .await
        .unwrap_err();
        assert!(!report(&direct).contains("hunter2"), "{}", report(&direct));
    }
}
