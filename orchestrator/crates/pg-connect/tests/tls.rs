//! TLS as sqlx did it, against a server with TLS on and a certificate for `localhost` signed by a
//! test CA: what each `sslmode` takes and refuses. Skipped unless `ORCH_TEST_TLS_DATABASE_URL`
//! (a `postgres://` URL of that server at `localhost`, no `sslmode`) and `ORCH_TEST_TLS_ROOT_CERT`
//! (the CA's PEM file) are set. The plain server of `ORCH_TEST_DATABASE_URL`, when set, shows that
//! `prefer` falls back and `require` does not.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_pg_connect::PgOptions;

fn tls_server() -> Option<(String, String)> {
    Some((
        std::env::var("ORCH_TEST_TLS_DATABASE_URL").ok()?,
        std::env::var("ORCH_TEST_TLS_ROOT_CERT").ok()?,
    ))
}

/// `url` with `localhost` replaced by `127.0.0.1`: the same server under a name its
/// certificate does not have.
fn by_address(url: &str) -> String {
    url.replace("@localhost", "@127.0.0.1")
}

async fn encrypted(url: &str) -> Result<bool, String> {
    let options: PgOptions = url.parse().map_err(|e| format!("{e}"))?;
    let client = options.connect().await.map_err(|e| format!("{e:?}"))?;
    let row = client
        .query_one(
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &[],
        )
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(row.get(0))
}

#[tokio::test]
async fn each_sslmode_takes_and_refuses_what_sqlx_did() {
    let Some((url, ca)) = tls_server() else {
        return;
    };
    let with = |mode: &str| format!("{url}?sslmode={mode}");
    let trusted = |mode: &str, base: &str| format!("{base}?sslmode={mode}&sslrootcert={ca}");

    assert_eq!(encrypted(&with("disable")).await, Ok(false));
    assert_eq!(
        encrypted(&with("prefer")).await,
        Ok(true),
        "TLS, the certificate not checked"
    );
    assert_eq!(encrypted(&url).await, Ok(true), "prefer is the default");
    assert_eq!(
        encrypted(&with("require")).await,
        Ok(true),
        "the certificate not checked"
    );

    // The test CA is no Mozilla root: without sslrootcert neither verifying mode trusts it.
    assert!(encrypted(&with("verify-ca")).await.is_err());
    assert!(encrypted(&with("verify-full")).await.is_err());

    // verify-ca checks the chain only: the address the certificate does not name is fine.
    assert_eq!(
        encrypted(&trusted("verify-ca", &by_address(&url))).await,
        Ok(true)
    );
    // verify-full checks the name too.
    assert_eq!(encrypted(&trusted("verify-full", &url)).await, Ok(true));
    assert!(
        encrypted(&trusted("verify-full", &by_address(&url)))
            .await
            .is_err(),
        "the certificate names localhost, not 127.0.0.1"
    );
}

#[tokio::test]
async fn a_pool_connects_with_the_same_tls() {
    let Some((url, ca)) = tls_server() else {
        return;
    };
    let options: PgOptions = format!("{url}?sslmode=verify-full&sslrootcert={ca}")
        .parse()
        .unwrap();
    let pool = options.pool(2, std::time::Duration::from_secs(5)).unwrap();
    let client = pool.get().await.unwrap();
    let row = client
        .query_one(
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &[],
        )
        .await
        .unwrap();
    assert!(row.get::<_, bool>(0));
}

#[tokio::test]
async fn prefer_falls_back_on_a_plain_server_and_require_does_not() {
    let Ok(plain) = std::env::var("ORCH_TEST_DATABASE_URL") else {
        return;
    };
    let base = plain.split('?').next().unwrap().to_owned();
    assert_eq!(
        encrypted(&format!("{base}?sslmode=prefer")).await,
        Ok(false)
    );
    assert!(encrypted(&format!("{base}?sslmode=require")).await.is_err());
}
