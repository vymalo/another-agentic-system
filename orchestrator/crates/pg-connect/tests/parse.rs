//! A connection string names the server, the user, the database and the session parameters it
//! named to sqlx 0.9: the same URLs read by both.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_pg_connect::{PgOptions, TlsMode};
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use tokio_postgres::config::Host;

const URLS: &[&str] = &[
    "postgres://app:pw@db.example:5433/orch",
    "postgresql://app:pw@db.example/orch",
    "postgres://app@db.example:6432/orch?sslmode=require",
    "postgres://app:p%40ss@db.example/orch?application_name=orchestrator",
    "postgres://app:pw@db.example/orch?sslmode=verify-full&sslrootcert=/etc/ca.pem",
    "postgres://app:pw@db.example/orch?options=-c%20search_path%3Dorch",
    "postgres://app:pw@db.example/orch?sslmode=disable&connect_timeout=5",
];

#[test]
fn every_url_names_what_it_named_to_sqlx() {
    for url in URLS {
        let theirs: PgConnectOptions = url.parse().unwrap();
        let ours: PgOptions = url.parse().unwrap();
        let config = ours.config();
        let Host::Tcp(host) = &config.get_hosts()[0] else {
            panic!("{url}: a TCP host");
        };
        assert_eq!(host, theirs.get_host(), "{url}: host");
        assert_eq!(
            config.get_ports().first().copied().unwrap_or(5432),
            theirs.get_port(),
            "{url}: port"
        );
        assert_eq!(
            config.get_user(),
            Some(theirs.get_username()),
            "{url}: user"
        );
        assert_eq!(
            config.get_dbname(),
            theirs.get_database(),
            "{url}: database"
        );
        assert_eq!(
            config.get_application_name(),
            theirs.get_application_name(),
            "{url}: application_name"
        );
        assert_eq!(config.get_options(), theirs.get_options(), "{url}: options");
        let mode = match theirs.get_ssl_mode() {
            PgSslMode::Disable | PgSslMode::Allow => TlsMode::Disable,
            PgSslMode::Prefer => TlsMode::Prefer,
            PgSslMode::Require => TlsMode::Require,
            PgSslMode::VerifyCa => TlsMode::VerifyCa,
            PgSslMode::VerifyFull => TlsMode::VerifyFull,
        };
        assert_eq!(ours.tls(), mode, "{url}: sslmode");
    }
}
