//! A connection string names the server, the user, the password, the database and the session
//! parameters it named to sqlx 0.9: what sqlx read from each of these URLs, in an environment with
//! no `PG*` variable and, in a child process each, in environments of `PG*` variables and a password
//! file, was recorded by sqlx itself before it left the workspace (`tests/sqlx-0.9-reads.tsv`, sys#382
//! E1), and every URL must still read the same. Every case runs in a child process whose
//! environment is cleared, so a `PG*` variable or a `~/.pgpass` of the machine running the tests
//! changes nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::Path;

use orch_pg_connect::PgOptions;

/// What sqlx 0.9 read, one line per case and URL.
const SQLX_READS: &str = include_str!("sqlx-0.9-reads.tsv");

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

/// What `ours` reads, in the columns of the recording: the host, the port, the user, the password,
/// the database, the `application_name`, the `options` and the `sslmode`, each as the recording
/// wrote sqlx's (`Debug`).
fn reading(ours: &PgOptions) -> Vec<String> {
    let config = ours.config();
    let user = format!("{:?}", config.get_user().unwrap_or_default());
    let password = config
        .get_password()
        .map(|p| std::str::from_utf8(p).unwrap());
    vec![
        format!("{:?}", config.get_hosts()[0]),
        config.get_ports()[0].to_string(),
        user,
        format!("{password:?}"),
        format!("{:?}", config.get_dbname()),
        format!("{:?}", config.get_application_name()),
        format!("{:?}", config.get_options()),
        format!("{:?}", ours.tls()),
    ]
}

/// sqlx 0.9's user where neither the URL nor `PGUSER` names one (`PgConnectOptions::new_without_pgpass`):
/// the operating system's, else `unknown`. The recording wrote it `<os-user>`, because it depends on
/// the machine (a container's uid may have no name).
fn sqlx_default_user() -> String {
    whoami::username().unwrap_or_else(|_| "unknown".to_owned())
}

/// The recorded reading of `url` in `case`, `<os-user>` replaced by this machine's.
fn recorded(case: &str, url: &str) -> Vec<String> {
    SQLX_READS
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.split('\t').collect::<Vec<_>>())
        .find(|fields| fields[0] == case && fields[1] == url)
        .unwrap_or_else(|| panic!("no recording of {url} in case {case}"))[2..]
        .iter()
        .map(|field| match *field {
            "<os-user>" => format!("{:?}", sqlx_default_user()),
            field => field.to_owned(),
        })
        .collect()
}

/// sqlx's default host where no host is named: the first of these directories that holds the
/// server's socket, else `localhost`. The recording was made where none did; where one does, sqlx
/// would have said its directory, as this crate does.
fn default_host_here(theirs: &str, ours: &str, port: &str) -> bool {
    theirs == "Tcp(\"localhost\")"
        && ["/var/run/postgresql", "/private/tmp", "/tmp"]
            .iter()
            .any(|dir| {
                ours == format!("Unix({dir:?})")
                    && Path::new(dir).join(format!(".s.PGSQL.{port}")).exists()
            })
}

fn same(url: &str, case: &str) {
    let ours: PgOptions = url.parse().unwrap();
    let mut ours = reading(&ours);
    let theirs = recorded(case, url);
    if default_host_here(&theirs[0], &ours[0], &ours[1]) {
        ours[0] = theirs[0].clone();
    }
    assert_eq!(
        ours, theirs,
        "case {case}, {url}: host, port, user, password, database, application_name, options, sslmode"
    );
}

/// Set in the child processes of the tests below, to the case they check.
const CASE: &str = "ORCH_PG_CONNECT_ENV_CASE";

/// Runs `test` of this binary again in a child process whose environment holds only `CASE`, a
/// `HOME` with no password file, `PATH` and `vars`, and asserts it passed.
fn in_child(test: &str, case: &str, vars: &[(&str, &str)]) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", test, "--nocapture"])
        .env_clear()
        .env(CASE, case)
        .env("HOME", "/nonexistent-home");
    if let Some(path) = std::env::var_os("PATH") {
        child.env("PATH", path);
    }
    child.envs(vars.iter().copied());
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "case {case}:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "case {case}: the child ran the test"
    );
}

#[test]
fn every_url_names_what_it_named_to_sqlx() {
    if std::env::var(CASE).is_ok() {
        for url in URLS {
            same(url, "-");
        }
        return;
    }
    in_child("every_url_names_what_it_named_to_sqlx", "-", &[]);
}

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
    for (name, vars) in cases {
        in_child("the_environment_is_read_as_sqlx_reads_it", name, &vars);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}
