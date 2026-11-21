//! A connection string read as sqlx 0.9 read it (`PgConnectOptions::parse_from_url` and
//! `new_without_pgpass`): the `PG*` environment variables first, the URL over them, its query
//! form-decoded key by key with the last one winning, then the password file for a password
//! nothing gave.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

use crate::{ParseError, RootCert, TlsMode};

/// What a connection string names, before it becomes a `tokio-postgres` configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Parsed {
    /// sqlx's `host`: what the password file is matched against, and the server unless a socket
    /// is set (a value that starts with `/` is a socket's directory).
    pub(crate) host: String,
    /// sqlx's `socket`: set by a URL host or a `host` parameter that starts with `/`, it wins over
    /// `host`.
    pub(crate) socket: Option<PathBuf>,
    pub(crate) port: u16,
    pub(crate) user: String,
    pub(crate) password: Option<String>,
    pub(crate) dbname: Option<String>,
    pub(crate) application_name: Option<String>,
    pub(crate) options: Option<String>,
    pub(crate) tls: TlsMode,
    pub(crate) root_cert: Option<RootCert>,
}

impl Parsed {
    /// Where to connect, as sqlx's `fetch_socket` decided it.
    pub(crate) fn target(&self) -> Host {
        match &self.socket {
            Some(dir) => Host::Socket(dir.clone()),
            None if self.host.starts_with('/') => Host::Socket(PathBuf::from(&self.host)),
            None => Host::Tcp(
                self.host
                    .strip_prefix('[')
                    .and_then(|h| h.strip_suffix(']'))
                    .unwrap_or(&self.host)
                    .to_owned(),
            ),
        }
    }
}

/// A TCP host (a name or an address) or the directory of a Unix socket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Host {
    Tcp(String),
    Socket(PathBuf),
}

/// Where the environment and the password file come from: the process's, or a test's.
pub(crate) trait Source {
    /// The environment variable `name`, if set.
    fn var(&self, name: &str) -> Option<String>;
    /// The text of the password file at `path`, if it can be used: on Linux one that the group or
    /// others can read is not, as sqlx ignored it.
    fn password_file(&self, path: &Path) -> Option<String>;
    /// The home directory, for `~/.pgpass`.
    fn home(&self) -> Option<PathBuf>;
    /// The operating system's name of the process's user, the user when nothing names one.
    fn os_user(&self) -> Option<String>;
    /// Whether a Unix socket file exists, for sqlx's default host.
    fn exists(&self, path: &Path) -> bool;
}

/// The process's environment, home and files.
pub(crate) struct Process;

impl Source for Process {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn password_file(&self, path: &Path) -> Option<String> {
        let file = std::fs::File::open(path).ok()?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::PermissionsExt;
            if file.metadata().ok()?.permissions().mode() & 0o77 != 0 {
                return None;
            }
        }
        std::io::read_to_string(file).ok()
    }

    fn home(&self) -> Option<PathBuf> {
        std::env::home_dir()
    }

    fn os_user(&self) -> Option<String> {
        whoami::username().ok()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

fn mode(text: &str) -> Result<TlsMode, ParseError> {
    Ok(match text.to_ascii_lowercase().as_str() {
        "disable" | "allow" => TlsMode::Disable,
        "prefer" => TlsMode::Prefer,
        "require" => TlsMode::Require,
        "verify-ca" => TlsMode::VerifyCa,
        "verify-full" => TlsMode::VerifyFull,
        _ => return Err(ParseError::SslMode),
    })
}

/// sqlx's default host: the first directory that holds the server's Unix socket, else localhost.
fn default_host(port: u16, source: &dyn Source) -> String {
    let socket = format!(".s.PGSQL.{port}");
    ["/var/run/postgresql", "/private/tmp", "/tmp"]
        .into_iter()
        .find(|dir| source.exists(&Path::new(dir).join(&socket)))
        .unwrap_or("localhost")
        .to_owned()
}

fn decoded(text: &str) -> Result<String, ParseError> {
    percent_encoding::percent_decode_str(text)
        .decode_utf8()
        .map(|t| t.into_owned())
        .map_err(|_| ParseError::NotAUrl)
}

/// One pair of sqlx's `options([(key, value)])`: `-c key=value`, a space or a backslash in either
/// escaped.
fn option_pair(key: &str, value: &str) -> String {
    let escape = |s: &str| s.replace('\\', "\\\\").replace(' ', "\\ ");
    format!("-c {}={}", escape(key), escape(value))
}

/// sqlx's `options([(key, value)])`, appended to what `options` already holds.
fn push_option(options: &mut Option<String>, key: &str, value: &str) {
    let options = options.get_or_insert_with(String::new);
    if !options.is_empty() {
        options.push(' ');
    }
    options.push_str(&option_pair(key, value));
}

/// An invalid value of a parameter sqlx checked.
fn invalid(key: &'static str) -> impl FnOnce(std::num::ParseIntError) -> ParseError {
    move |_| ParseError::Parameter(key)
}

pub(crate) fn parse(url: &str, source: &dyn Source) -> Result<Parsed, ParseError> {
    let url = url::Url::parse(url).map_err(|_| ParseError::NotAUrl)?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        return Err(ParseError::Scheme);
    }
    // The environment, as `new_without_pgpass`.
    if source.var("PGSSLCERT").is_some() || source.var("PGSSLKEY").is_some() {
        return Err(ParseError::ClientCertificate);
    }
    let port = source
        .var("PGPORT")
        .and_then(|v| v.parse().ok())
        .unwrap_or(5432);
    let mut parsed = Parsed {
        host: source
            .var("PGHOSTADDR")
            .or_else(|| source.var("PGHOST"))
            .unwrap_or_else(|| default_host(port, source)),
        socket: None,
        port,
        user: source
            .var("PGUSER")
            .or_else(|| source.os_user())
            .unwrap_or_else(|| "unknown".to_owned()),
        password: source.var("PGPASSWORD"),
        dbname: source.var("PGDATABASE"),
        application_name: source.var("PGAPPNAME"),
        options: source.var("PGOPTIONS"),
        tls: match source.var("PGSSLMODE") {
            Some(m) => mode(&m)?,
            None => TlsMode::Prefer,
        },
        root_cert: source.var("PGSSLROOTCERT").map(|c| RootCert::from_text(&c)),
    };

    // The URL's parts, percent-decoded (a TCP host kept as written, as sqlx kept it).
    if let Some(host) = url.host_str() {
        let text = decoded(host)?;
        if text.starts_with('/') {
            parsed.socket = Some(PathBuf::from(text));
        } else {
            parsed.host = host.to_owned();
        }
    }
    if let Some(port) = url.port() {
        parsed.port = port;
    }
    if !url.username().is_empty() {
        parsed.user = decoded(url.username())?;
    }
    if let Some(password) = url.password() {
        parsed.password = Some(decoded(password)?);
    }
    let path = url.path().trim_start_matches('/');
    if !path.is_empty() {
        parsed.dbname = Some(decoded(path)?);
    }

    // The query, form-decoded (a `+` is a space), key by key: the last one wins, but `options`
    // grows.
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "sslmode" | "ssl-mode" => parsed.tls = mode(&value)?,
            "sslrootcert" | "ssl-root-cert" | "ssl-ca" => {
                parsed.root_cert = Some(RootCert::from_text(&value));
            }
            "sslcert" | "ssl-cert" | "sslkey" | "ssl-key" | "sslpassword" => {
                return Err(ParseError::ClientCertificate);
            }
            // Read and checked by sqlx, meaningless here: `tokio-postgres` prepares statements
            // only when told to.
            "statement-cache-capacity" => {
                value
                    .parse::<usize>()
                    .map_err(invalid("statement-cache-capacity"))?;
            }
            "host" => {
                if value.starts_with('/') {
                    parsed.socket = Some(PathBuf::from(value.as_ref()));
                } else {
                    parsed.host = value.into_owned();
                }
            }
            "hostaddr" => {
                value
                    .parse::<IpAddr>()
                    .map_err(|_| ParseError::Parameter("hostaddr"))?;
                parsed.host = value.into_owned();
            }
            "port" => parsed.port = value.parse().map_err(invalid("port"))?,
            "dbname" => parsed.dbname = Some(value.into_owned()),
            "user" => parsed.user = value.into_owned(),
            "password" => parsed.password = Some(value.into_owned()),
            "application_name" => parsed.application_name = Some(value.into_owned()),
            "options" => {
                parsed.options = Some(match parsed.options.take() {
                    Some(mut options) => {
                        options.push(' ');
                        options.push_str(&value);
                        options
                    }
                    None => value.into_owned(),
                });
            }
            k if k.starts_with("options[") => {
                if let Some(name) = k["options[".len()..].strip_suffix(']') {
                    push_option(&mut parsed.options, name, &value);
                }
            }
            // sqlx ignored every other key (with a warning); so does this.
            _ => {}
        }
    }

    // The password file, for a password nothing gave.
    if parsed.password.is_none() {
        let files = [
            source.var("PGPASSFILE").map(PathBuf::from),
            source.home().map(|h| h.join(".pgpass")),
        ];
        parsed.password = files.into_iter().flatten().find_map(|path| {
            let text = source.password_file(&path)?;
            password_in(
                &text,
                &parsed.host,
                parsed.port,
                &parsed.user,
                parsed.dbname.as_deref(),
            )
        });
    }
    Ok(parsed)
}

/// The password of the first line of a password file that matches, as sqlx's `pgpass` read it:
/// `host:port:database:user:password`, `*` matching anything, `\` escaping the next character, a
/// line that starts with `#` skipped, a malformed line skipped.
pub(crate) fn password_in(
    text: &str,
    host: &str,
    port: u16,
    user: &str,
    dbname: Option<&str>,
) -> Option<String> {
    let port = port.to_string();
    text.lines().find_map(|line| {
        if line.trim_end().is_empty() || line.starts_with('#') {
            return None;
        }
        let mut rest = line;
        for want in [host, port.as_str(), dbname.unwrap_or_default(), user] {
            let field = next_field(&mut rest)?;
            if field != "*" && field != want {
                return None;
            }
        }
        Some(unescape(rest))
    })
}

/// The next `:`-separated field, unescaped; `None` at the end of the line (malformed).
fn next_field(line: &mut &str) -> Option<String> {
    let mut field = String::new();
    let mut escaping = false;
    for (idx, c) in line.char_indices() {
        if escaping {
            field.push(c);
            escaping = false;
        } else if c == '\\' {
            escaping = true;
        } else if c == ':' {
            *line = &line[idx + 1..];
            return Some(field);
        } else {
            field.push(c);
        }
    }
    None
}

fn unescape(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[derive(Default)]
    struct Fake {
        vars: HashMap<&'static str, &'static str>,
        files: HashMap<PathBuf, &'static str>,
        sockets: Vec<PathBuf>,
        os_user: Option<&'static str>,
    }

    impl Source for Fake {
        fn var(&self, name: &str) -> Option<String> {
            self.vars.get(name).map(|v| (*v).to_owned())
        }
        fn password_file(&self, path: &Path) -> Option<String> {
            self.files.get(path).map(|t| (*t).to_owned())
        }
        fn home(&self) -> Option<PathBuf> {
            Some(PathBuf::from("/home/test"))
        }
        fn os_user(&self) -> Option<String> {
            self.os_user.map(str::to_owned)
        }
        fn exists(&self, path: &Path) -> bool {
            self.sockets.iter().any(|s| s == path)
        }
    }

    fn with(vars: &[(&'static str, &'static str)]) -> Fake {
        Fake {
            vars: vars.iter().copied().collect(),
            ..Fake::default()
        }
    }

    fn tcp(host: &str) -> Host {
        Host::Tcp(host.to_owned())
    }

    #[test]
    fn the_environment_is_read_first_and_the_url_wins() {
        let env = with(&[
            ("PGHOST", "envhost"),
            ("PGPORT", "6000"),
            ("PGUSER", "envuser"),
            ("PGPASSWORD", "envpw"),
            ("PGDATABASE", "envdb"),
            ("PGAPPNAME", "envapp"),
            ("PGOPTIONS", "-c search_path=env"),
            ("PGSSLMODE", "verify-full"),
            ("PGSSLROOTCERT", "/etc/env-ca.pem"),
        ]);
        let bare = parse("postgres://", &env).unwrap();
        assert_eq!(bare.target(), tcp("envhost"));
        assert_eq!(bare.port, 6000);
        assert_eq!(bare.user, "envuser");
        assert_eq!(bare.password.as_deref(), Some("envpw"));
        assert_eq!(bare.dbname.as_deref(), Some("envdb"));
        assert_eq!(bare.application_name.as_deref(), Some("envapp"));
        assert_eq!(bare.options.as_deref(), Some("-c search_path=env"));
        assert_eq!(bare.tls, TlsMode::VerifyFull, "PGSSLMODE is not dropped");
        assert_eq!(
            bare.root_cert,
            Some(RootCert::Path("/etc/env-ca.pem".into()))
        );

        let url = parse("postgres://u:p@h:7000/db?sslmode=require", &env).unwrap();
        assert_eq!(url.target(), tcp("h"));
        assert_eq!((url.port, url.user.as_str()), (7000, "u"));
        assert_eq!(url.password.as_deref(), Some("p"));
        assert_eq!(url.tls, TlsMode::Require);

        let addr = with(&[("PGHOSTADDR", "10.0.0.1"), ("PGHOST", "name")]);
        assert_eq!(
            parse("postgres://", &addr).unwrap().target(),
            tcp("10.0.0.1")
        );
        let socket = with(&[("PGHOST", "/run/pg")]);
        let at = parse("postgres://", &socket).unwrap().target();
        assert_eq!(at, Host::Socket("/run/pg".into()));
    }

    #[test]
    fn the_user_is_the_os_user_and_then_unknown() {
        let os = Fake {
            os_user: Some("osuser"),
            ..Fake::default()
        };
        assert_eq!(parse("postgres://h", &os).unwrap().user, "osuser");
        assert_eq!(
            parse("postgres://h", &Fake::default()).unwrap().user,
            "unknown"
        );
        let both = Fake {
            os_user: Some("osuser"),
            ..with(&[("PGUSER", "envuser")])
        };
        assert_eq!(parse("postgres://h", &both).unwrap().user, "envuser");
    }

    #[test]
    fn a_client_certificate_and_a_bad_mode_are_refused_from_the_environment_too() {
        for name in ["PGSSLCERT", "PGSSLKEY"] {
            assert_eq!(
                parse("postgres://h/db", &with(&[(name, "/c.pem")])),
                Err(ParseError::ClientCertificate),
                "{name}"
            );
        }
        assert_eq!(
            parse("postgres://h/db", &with(&[("PGSSLMODE", "maybe")])),
            Err(ParseError::SslMode)
        );
        let upper = parse("postgres://h/db", &with(&[("PGSSLMODE", "Verify-Full")])).unwrap();
        assert_eq!(upper.tls, TlsMode::VerifyFull);
    }

    #[test]
    fn the_default_host_is_sqlx_s() {
        let none = parse("postgres:///db", &Fake::default()).unwrap();
        assert_eq!(none.target(), tcp("localhost"));
        let socket = Fake {
            sockets: vec![
                PathBuf::from("/tmp/.s.PGSQL.5432"),
                PathBuf::from("/private/tmp/.s.PGSQL.6000"),
            ],
            ..Fake::default()
        };
        let found = parse("postgres:///db", &socket).unwrap();
        assert_eq!(found.target(), Host::Socket("/tmp".into()));
        let other_port = Fake {
            sockets: socket.sockets.clone(),
            ..with(&[("PGPORT", "6000")])
        };
        let found = parse("postgres:///db", &other_port).unwrap();
        assert_eq!(found.target(), Host::Socket("/private/tmp".into()));
    }

    #[test]
    fn the_query_is_read_key_by_key_and_the_last_wins() {
        let env = Fake::default();
        let p = parse(
            "postgres://u:p@first:1/db1?host=second&port=2&host=third&port=3&dbname=db2\
             &user=v&password=q&application_name=a%20b+c&sslmode=VERIFY-CA&ssl-mode=require\
             &ssl-root-cert=/one.pem&ssl-ca=/two.pem&connect_timeout=5&unknown=x\
             &statement-cache-capacity=0",
            &env,
        )
        .unwrap();
        assert_eq!(p.target(), tcp("third"));
        assert_eq!(p.port, 3);
        assert_eq!(p.dbname.as_deref(), Some("db2"));
        assert_eq!((p.user.as_str(), p.password.as_deref()), ("v", Some("q")));
        assert_eq!(
            p.application_name.as_deref(),
            Some("a b c"),
            "a + is a space"
        );
        assert_eq!(p.tls, TlsMode::Require);
        assert_eq!(p.root_cert, Some(RootCert::Path("/two.pem".into())));

        let addr = parse("postgres://h/db?hostaddr=192.0.2.7", &env).unwrap();
        assert_eq!(addr.target(), tcp("192.0.2.7"));
        let six = parse("postgres://[::1]:5433/db", &env).unwrap();
        assert_eq!(six.target(), tcp("::1"));
        assert_eq!(six.host, "[::1]", "kept as written, as sqlx kept it");
        for (url, key) in [
            ("postgres://h/db?hostaddr=name", "hostaddr"),
            ("postgres://h/db?port=x", "port"),
            (
                "postgres://h/db?statement-cache-capacity=-1",
                "statement-cache-capacity",
            ),
        ] {
            assert_eq!(parse(url, &env), Err(ParseError::Parameter(key)), "{url}");
        }
        for key in ["sslcert", "ssl-cert", "sslkey", "ssl-key", "sslpassword"] {
            let url = format!("postgres://h/db?{key}=/c.pem");
            assert_eq!(
                parse(&url, &env),
                Err(ParseError::ClientCertificate),
                "{key}"
            );
        }
    }

    #[test]
    fn a_socket_is_set_apart_from_the_host_as_sqlx_set_it() {
        let env = Fake::default();
        let url = parse("postgres://%2Frun%2Fpg/db", &env).unwrap();
        assert_eq!(url.target(), Host::Socket("/run/pg".into()));
        assert_eq!(
            url.host, "localhost",
            "the password file is matched on the host"
        );
        let query = parse("postgres://h/db?host=/run/pg&host=other", &env).unwrap();
        assert_eq!(
            query.target(),
            Host::Socket("/run/pg".into()),
            "a socket wins"
        );
        assert_eq!(query.host, "other");
    }

    #[test]
    fn options_grow_and_options_brackets_are_escaped() {
        let env = with(&[("PGOPTIONS", "-c a=1")]);
        let p = parse(
            "postgres://h/db?options=-c%20b%3D2&options%5Bsearch_path%5D=my%20schema\
             &options%5Bx%5D=back%5Cslash&options%5Bbroken=1",
            &env,
        )
        .unwrap();
        assert_eq!(
            p.options.as_deref(),
            Some(r"-c a=1 -c b=2 -c search_path=my\ schema -c x=back\\slash")
        );
        let alone = parse("postgres://h/db?options%5Bk%5D=v", &Fake::default()).unwrap();
        assert_eq!(alone.options.as_deref(), Some("-c k=v"));
    }

    #[test]
    fn the_url_s_parts_are_decoded_and_its_fragment_is_dropped() {
        let p = parse(
            "postgres://us%40er:p%3Aa%2Bss+@h/my%20db#frag",
            &Fake::default(),
        )
        .unwrap();
        assert_eq!(p.user, "us@er");
        assert_eq!(
            p.password.as_deref(),
            Some("p:a+ss+"),
            "a + in the URL's parts is a +"
        );
        assert_eq!(p.dbname.as_deref(), Some("my db"));
        let at = parse("postgres://u@h/db?application_name=a@b", &Fake::default()).unwrap();
        assert_eq!(at.target(), tcp("h"), "an @ in the query moves nothing");
        assert_eq!(at.application_name.as_deref(), Some("a@b"));
    }

    #[test]
    fn a_root_certificate_is_a_path_or_inline_pem() {
        let pem = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----";
        assert_eq!(RootCert::from_text(pem), RootCert::Pem(pem.to_owned()));
        assert_eq!(
            RootCert::from_text(&format!("  {pem}\n")),
            RootCert::Pem(format!("  {pem}\n"))
        );
        assert_eq!(
            RootCert::from_text("/etc/ca.pem"),
            RootCert::Path("/etc/ca.pem".into())
        );
        assert_eq!(
            RootCert::from_text("-----BEGIN CERTIFICATE----- truncated"),
            RootCert::Path("-----BEGIN CERTIFICATE----- truncated".into())
        );
    }

    #[test]
    fn the_password_file_gives_a_password_nothing_else_gave() {
        let mut env = Fake::default();
        env.files.insert(
            PathBuf::from("/home/test/.pgpass"),
            "# a comment\nother:*:*:*:no\nh\\:x:5432:db:u:esc\nh:5432:db:u:pa\\:ss\\\\w\n\
             h:5432::nodb:empty\nbroken\n*:*:*:*:any\n",
        );
        let found = parse("postgres://u@h/db", &env).unwrap();
        assert_eq!(found.password.as_deref(), Some("pa:ss\\w"));
        let nodb = parse("postgres://nodb@h", &env).unwrap();
        assert_eq!(
            nodb.password.as_deref(),
            Some("empty"),
            "no database matches ''"
        );
        let wildcard = parse("postgres://v@x/y", &env).unwrap();
        assert_eq!(wildcard.password.as_deref(), Some("any"));
        let given = parse("postgres://u:mine@h/db", &env).unwrap();
        assert_eq!(given.password.as_deref(), Some("mine"), "a given one wins");
        let from_env = parse(
            "postgres://u@h/db",
            &Fake {
                files: env.files.clone(),
                ..with(&[("PGPASSWORD", "envpw")])
            },
        )
        .unwrap();
        assert_eq!(from_env.password.as_deref(), Some("envpw"));
    }

    #[test]
    fn pgpassfile_comes_first_and_home_after_it() {
        let mut env = with(&[("PGPASSFILE", "/custom")]);
        env.files
            .insert(PathBuf::from("/custom"), "h:*:*:u:custom\n");
        env.files
            .insert(PathBuf::from("/home/test/.pgpass"), "*:*:*:*:home\n");
        assert_eq!(
            parse("postgres://u@h/db", &env)
                .unwrap()
                .password
                .as_deref(),
            Some("custom")
        );
        assert_eq!(
            parse("postgres://w@h/db", &env)
                .unwrap()
                .password
                .as_deref(),
            Some("home"),
            "no match in PGPASSFILE falls through to ~/.pgpass"
        );
    }

    #[test]
    fn password_lines_are_read_as_sqlx_reads_them() {
        assert_eq!(
            password_in("h:5432:db", "h", 5432, "u", Some("db")),
            None,
            "malformed"
        );
        assert_eq!(
            password_in("  # x\n*:*:*:*:p", "h", 1, "u", None),
            Some("p".into())
        );
        assert_eq!(password_in("#*:*:*:*:no\n", "h", 1, "u", None), None);
        assert_eq!(
            password_in("#h:1:d:u:no\n", "#h", 1, "u", Some("d")),
            None,
            "a comment"
        );
        assert_eq!(
            password_in("h:5432:db:u:p\\", "h", 5432, "u", Some("db")),
            Some("p".into()),
            "a trailing escape is dropped"
        );
        assert_eq!(
            password_in("h:1:d:u:a\\:b\\\\c\\d", "h", 1, "u", Some("d")),
            Some("a:b\\cd".into())
        );
        assert_eq!(
            password_in("h\\:x:1:d:u:p", "h:x", 1, "u", Some("d")),
            Some("p".into())
        );
        assert_eq!(
            password_in("h:1:d:u:p\r\n", "h", 1, "u", Some("d")),
            Some("p".into())
        );
    }
}
