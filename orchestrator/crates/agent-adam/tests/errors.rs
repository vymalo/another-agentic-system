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

/// Set in the child process of the test below.
const CHILD: &str = "ORCH_AGENT_ADAM_LISTENER_CHILD";

/// adam-rs's notifier connects its listener from the URL alone (`adam-postgres` reads no `PG*`
/// variable and no password file), so a URL whose password comes from `PGPASSWORD` would give
/// pools that connect and a listener that never does: it is refused at once, as `Invalid`, naming
/// the part and never its value. A URL that names everything is taken. Run in a child process, the
/// only one whose environment this test sets.
#[tokio::test]
async fn a_listener_url_the_environment_completes_is_refused() {
    if std::env::var(CHILD).is_err() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "a_listener_url_the_environment_completes_is_refused",
                "--nocapture",
            ])
            .env_clear()
            .env(CHILD, "1")
            .env("HOME", "/nonexistent-home")
            .env("PGPASSWORD", "from-the-environment");
        if let Some(path) = std::env::var_os("PATH") {
            child.env("PATH", path);
        }
        let output = child.output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let hosted = |url: &str| {
        let pool = url
            .parse::<PgOptions>()
            .unwrap()
            .pool(1, std::time::Duration::from_secs(1))
            .unwrap();
        LocalAgents::postgres(
            pool,
            url,
            LocalOptions::new("listener-test"),
            &[LocalKind::Echo],
        )
    };
    let err = hosted("postgres://u@127.0.0.1:1/db").unwrap_err();
    assert_eq!(err.class(), ErrorClass::Invalid);
    for text in [report(&err), format!("{err:?}")] {
        assert!(text.contains("password"), "{text}");
        assert!(!text.contains("from-the-environment"), "{text}");
    }
    assert!(hosted("postgres://u:pw@127.0.0.1:1/db").is_ok());
}
