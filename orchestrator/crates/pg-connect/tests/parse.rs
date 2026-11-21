//! A connection string names the server, the user, the password, the database and the session
//! parameters it named to sqlx 0.9: the same URLs read by both, in this process's environment and,
//! in a child process each, in environments of `PG*` variables and a password file.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::PathBuf;

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
    // What the first version of this crate read differently.
    "postgres://app:pw@db.example/orch?application_name=a+b",
    "postgres://app:pw@db.example/orch?host=other.example&port=7000&host=third.example",
    "postgres://app:pw@db.example/orch?options=-c%20a%3D1&options=-c%20b%3D2",
    "postgres://app:pw@db.example/orch?options%5Bsearch_path%5D=my%20schema&options=-c%20c%3D3",
    "postgres://app:pw@db.example/orch?application_name=a@b",
    "postgres://app:pw@db.example/orch#fragment",
    "postgres://app:pw@db.example/orch?hostaddr=192.0.2.7",
    "postgres://app:pw@db.example/orch?ssl-mode=VERIFY-CA&ssl-ca=/etc/ca.pem",
    "postgres://app:pw@db.example/orch?sslmode=Require&statement-cache-capacity=10&unknown=1",
    "postgres://app:pw@db.example/orch?dbname=other&user=someone&password=else",
    "postgres://app:pw@%2Frun%2Fpostgresql/orch",
    "postgres://app:pw@db.example/orch?host=/run/postgresql",
    "postgres://app:pw@[::1]:5433/orch",
    "postgres://app:pw@db.example/my%20db",
];

/// What sqlx connects to: its socket, else its host when it starts with `/`, else its host (an
/// IPv6 address without its brackets).
fn their_host(theirs: &PgConnectOptions) -> Host {
    match theirs.get_socket() {
        Some(dir) => Host::Unix(dir.clone()),
        None if theirs.get_host().starts_with('/') => Host::Unix(PathBuf::from(theirs.get_host())),
        None => Host::Tcp(
            theirs
                .get_host()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_owned(),
        ),
    }
}

/// sqlx keeps the password private; its `Debug` shows it.
fn their_password(theirs: &PgConnectOptions) -> String {
    let debug = format!("{theirs:?}");
    let start = debug.find("password: ").unwrap() + "password: ".len();
    let end = start + debug[start..].find(", database: ").unwrap();
    debug[start..end].to_owned()
}

fn same(url: &str, context: &str) {
    let theirs: PgConnectOptions = url.parse().unwrap();
    let ours: PgOptions = url.parse().unwrap();
    let config = ours.config();
    assert_eq!(
        config.get_hosts(),
        [their_host(&theirs)],
        "{context} {url}: host"
    );
    assert_eq!(
        config.get_ports(),
        [theirs.get_port()],
        "{context} {url}: port"
    );
    assert_eq!(
        config.get_user(),
        Some(theirs.get_username()),
        "{context} {url}: user"
    );
    let password = config
        .get_password()
        .map(|p| std::str::from_utf8(p).unwrap());
    assert_eq!(
        format!("{password:?}"),
        their_password(&theirs),
        "{context} {url}: password"
    );
    assert_eq!(
        config.get_dbname(),
        theirs.get_database(),
        "{context} {url}: database"
    );
    assert_eq!(
        config.get_application_name(),
        theirs.get_application_name(),
        "{context} {url}: application_name"
    );
    assert_eq!(
        config.get_options(),
        theirs.get_options(),
        "{context} {url}: options"
    );
    let mode = match theirs.get_ssl_mode() {
        PgSslMode::Disable | PgSslMode::Allow => TlsMode::Disable,
        PgSslMode::Prefer => TlsMode::Prefer,
        PgSslMode::Require => TlsMode::Require,
        PgSslMode::VerifyCa => TlsMode::VerifyCa,
        PgSslMode::VerifyFull => TlsMode::VerifyFull,
    };
    assert_eq!(ours.tls(), mode, "{context} {url}: sslmode");
}

#[test]
fn every_url_names_what_it_named_to_sqlx() {
    for url in URLS {
        same(url, "");
    }
}

/// Set in the child processes of the test below, to the case they check.
const CASE: &str = "ORCH_PG_CONNECT_ENV_CASE";

/// The URLs read in each environment: one that names nothing, and some that name a part.
const ENV_URLS: &[&str] = &[
    "postgres://",
    "postgres:///db",
    "postgres://h:1/d?options=-c%20b%3D2",
    "postgres://u@%2Fsock%2Fdir/d",
    "postgres://u@h/d",
    "postgres://pgpass@h/d",
];

#[test]
fn the_environment_is_read_as_sqlx_reads_it() {
    if let Ok(case) = std::env::var(CASE) {
        for url in ENV_URLS {
            same(url, &case);
        }
        return;
    }
    let dir = std::env::temp_dir().join(format!("orch-pg-connect-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pgpass = dir.join("pgpass");
    std::fs::write(
        &pgpass,
        "# comment\nh:5432:d:pgpass:from\\:file\n*:*:*:*:any\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&pgpass, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let open = dir.join("pgpass-open");
    std::fs::write(&open, "*:*:*:*:readable\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let pgpass = pgpass.to_str().unwrap().to_owned();
    let open = open.to_str().unwrap().to_owned();
    let home = dir.to_str().unwrap().to_owned();
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("none", vec![]),
        (
            "all",
            vec![
                ("PGHOST", "envhost"),
                ("PGPORT", "6000"),
                ("PGUSER", "envuser"),
                ("PGPASSWORD", "envpw"),
                ("PGDATABASE", "envdb"),
                ("PGAPPNAME", "envapp"),
                ("PGOPTIONS", "-c a=1"),
                ("PGSSLMODE", "verify-full"),
                ("PGSSLROOTCERT", "/etc/ca.pem"),
            ],
        ),
        (
            "hostaddr",
            vec![
                ("PGHOSTADDR", "10.1.2.3"),
                ("PGHOST", "name"),
                ("PGSSLMODE", "Require"),
            ],
        ),
        (
            "socket",
            vec![("PGHOST", "/run/postgresql"), ("PGPORT", "notaport")],
        ),
        ("pgpassfile", vec![("PGPASSFILE", pgpass.as_str())]),
        ("pgpass-open", vec![("PGPASSFILE", open.as_str())]),
        ("pgpass-home", vec![("HOME", home.as_str())]),
    ];
    std::fs::write(dir.join(".pgpass"), "*:*:*:*:home-only-if-private\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(".pgpass");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let me = std::env::current_exe().unwrap();
    for (name, vars) in cases {
        let mut child = std::process::Command::new(&me);
        child
            .args([
                "--exact",
                "the_environment_is_read_as_sqlx_reads_it",
                "--nocapture",
            ])
            .env_clear()
            .env(CASE, name)
            .env("HOME", "/nonexistent-home");
        if let Some(path) = std::env::var_os("PATH") {
            child.env("PATH", path);
        }
        child.envs(vars);
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "case {name}:\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "case {name}: the child ran the test"
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
