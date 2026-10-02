//! `toolServers` (ADR 0024): the servers a person may attach to a conversation. A good file, every
//! kind of mistake listed at once and naming the key, the credentials resolved from references and
//! never printed.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::{DEFAULT_TOOL_SERVER_TIMEOUT_SECS, ErrorKind, MAX_ICON_BYTES, render};
use support::{Fake, lines, load};

const HEAD: &str = "\
version: 1
database:
  url: { env: DATABASE_URL }
agents:
  file: agents.yaml
";

fn env() -> Fake {
    Fake::default()
        .env("DATABASE_URL", "postgres://u:pw@db/orch")
        .env("SEARCH_TOKEN", "tok-SEARCH-1234")
        .env("SEARCH_KEY", "key-SEARCH-5678")
}

fn file(servers: &str) -> String {
    format!("{HEAD}toolServers:\n{servers}")
}

const SVG: &str =
    "data:image/svg+xml;base64,PHN2ZyB4bWxucz0naHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmcnLz4=";

#[test]
fn a_file_without_the_key_offers_nothing() {
    let valid = load(HEAD, &env()).unwrap();
    assert!(valid.config.tool_servers.is_empty());
    assert!(valid.secrets.tool_servers.is_empty());
    // and the effective configuration does not print an empty list
    let shown = serde_norway::to_string(&valid.config.effective()).unwrap();
    assert!(!shown.contains("toolServers"), "{shown}");
}

#[test]
fn a_server_has_what_the_contract_names_and_its_credentials_are_resolved() {
    let text = file(&format!(
        "  - id: websearch
    name: Web search
    description: Search the web.
    url: https://search.example.com/mcp
    icon: {SVG}
    bearer: {{ env: SEARCH_TOKEN }}
    headers:
      X-Api-Key: {{ env: SEARCH_KEY }}
    tools: [search, fetch_page]
    agents: [chat, researcher]
    timeoutSecs: 60
  - id: docs
    name: Documentation
    url: http://docs.internal:9000/mcp
"
    ));
    let valid = load(&text, &env()).unwrap();
    let servers = &valid.config.tool_servers;
    assert_eq!(servers.len(), 2);
    let web = &servers[0];
    assert_eq!(web.id, "websearch");
    assert_eq!(web.name, "Web search");
    assert_eq!(web.description.as_deref(), Some("Search the web."));
    assert_eq!(web.icon.as_deref(), Some(SVG));
    assert_eq!(
        web.tools.as_deref(),
        Some(&["search".to_owned(), "fetch_page".to_owned()][..])
    );
    assert_eq!(web.timeout_secs, 60);
    // the second has defaults
    assert_eq!(servers[1].timeout_secs, DEFAULT_TOOL_SERVER_TIMEOUT_SECS);
    assert!(servers[1].agents.is_none() && servers[1].tools.is_none());
    // credentials: resolved, by server id
    let secrets = &valid.secrets.tool_servers;
    assert_eq!(secrets.len(), 1, "`docs` has none");
    let web = &secrets["websearch"];
    assert_eq!(web.bearer.as_ref().unwrap().expose(), "tok-SEARCH-1234");
    assert_eq!(web.headers.len(), 1);
    assert_eq!(web.headers[0].0, "X-Api-Key");
    assert_eq!(web.headers[0].1.expose(), "key-SEARCH-5678");
}

#[test]
fn no_debug_or_print_shows_a_credential() {
    let text = file(
        "  - id: websearch
    name: Web search
    url: https://search.example.com/mcp
    bearer: { env: SEARCH_TOKEN }
    headers: { X-Api-Key: { env: SEARCH_KEY } }
",
    );
    let valid = load(&text, &env()).unwrap();
    let shown = format!("{valid:?} {:?}", valid.secrets);
    for secret in ["tok-SEARCH-1234", "key-SEARCH-5678"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    assert!(
        shown.contains("SEARCH_TOKEN"),
        "the reference is shown: {shown}"
    );
    let printed = serde_norway::to_string(&valid.config.effective()).unwrap();
    for secret in ["tok-SEARCH-1234", "key-SEARCH-5678"] {
        assert!(!printed.contains(secret), "{printed}");
    }
    assert!(
        printed.contains("SEARCH_TOKEN") && printed.contains("SEARCH_KEY"),
        "{printed}"
    );
}

#[test]
fn a_credential_is_a_reference_never_a_string() {
    let text = file(
        "  - id: a
    name: A
    url: https://a.example.com/mcp
    bearer: tok-SEARCH-1234
    headers: { X-Api-Key: key-SEARCH-5678 }
",
    );
    let got = lines(load(&text, &env()));
    assert!(
        got.iter()
            .any(|l| l.starts_with("toolServers[0].bearer: a secret is a reference")),
        "{got:?}"
    );
    assert!(
        got.iter()
            .any(|l| l.starts_with("toolServers[0].headers.X-Api-Key: a secret is a reference")),
        "{got:?}"
    );
    let shown = got.join("\n");
    assert!(
        !shown.contains("SEARCH-1234") && !shown.contains("SEARCH-5678"),
        "{shown}"
    );
}

#[test]
fn an_unresolved_reference_names_the_key_and_the_variable_never_a_value() {
    let text = file(
        "  - id: a
    name: A
    url: https://a.example.com/mcp
    bearer: { env: NOT_SET }
    headers: { X-Api-Key: { file: /nope } }
",
    );
    let got = lines(load(&text, &env()));
    assert!(
        got.iter()
            .any(|l| l.contains("toolServers[0].bearer") && l.contains("NOT_SET")),
        "{got:?}"
    );
    assert!(
        got.iter()
            .any(|l| l.contains("toolServers[0].headers.X-Api-Key") && l.contains("/nope")),
        "{got:?}"
    );
}

#[test]
fn a_value_a_header_cannot_hold_is_refused_by_key() {
    let fake = env().env("BAD", "line one\nline two");
    let text = file(
        "  - id: a
    name: A
    url: https://a.example.com/mcp
    bearer: { env: BAD }
",
    );
    let got = lines(load(&text, &fake));
    assert_eq!(
        got,
        ["toolServers[0].bearer: the value is not one a header can hold"]
    );
}

#[test]
fn every_mistake_is_listed_at_once_and_names_its_key() {
    let text = file(&format!(
        "  - id: Web_Search
    name: ' '
    url: ftp://x
    icon: https://a.example.com/icon.png
    headers: {{ Authorization: {{ env: SEARCH_TOKEN }}, accept: {{ env: SEARCH_TOKEN }}, 'Bad Header': {{ env: SEARCH_TOKEN }} }}
    tools: [ '_hidden', 'a.b' ]
    agents: [ '', chat, chat ]
  - id: docs
    name: Docs
    url: https://u:pw@docs.example.com/mcp
  - id: docs
    name: Again
    url: https://docs.example.com/mcp?key=abc
    description: {}
",
        "x".repeat(501)
    ));
    let got = lines(load(&text, &env()));
    let has = |key: &str| got.iter().any(|l| l.starts_with(key));
    for key in [
        "toolServers[0].id",
        "toolServers[0].name",
        "toolServers[0].url",
        "toolServers[0].icon",
        "toolServers[0].headers.Authorization",
        "toolServers[0].headers.accept",
        "toolServers[0].headers.\"Bad Header\"",
        "toolServers[0].tools[0]",
        "toolServers[0].tools[1]",
        "toolServers[0].agents[0]",
        "toolServers[0].agents[2]",
        "toolServers[1].url",
        "toolServers[2].id",
        "toolServers[2].url",
        "toolServers[2].description",
    ] {
        assert!(has(key), "no error for {key}:\n{}", got.join("\n"));
    }
    // none of them quotes what was written
    let shown = got.join("\n");
    for value in [
        "Web_Search",
        "ftp://x",
        "a.example.com/icon.png",
        "abc",
        "pw@",
        "_hidden",
    ] {
        assert!(!shown.contains(value), "{value} reached an error:\n{shown}");
    }
}

#[test]
fn an_icon_is_a_small_data_uri_of_an_image_and_nothing_else() {
    let at = |icon: &str| {
        file(&format!(
            "  - id: a\n    name: A\n    url: https://a.example.com/mcp\n    icon: '{icon}'\n"
        ))
    };
    for good in [
        SVG,
        "data:image/png;base64,iVBORw0KGgo=",
        "data:image/webp;base64,UklGRg==",
    ] {
        load(&at(good), &env()).unwrap_or_else(|e| panic!("{good}: {}", render(&e)));
    }
    let long = format!("data:image/png;base64,{}", "A".repeat(MAX_ICON_BYTES));
    for bad in [
        "https://a.example.com/icon.svg",
        "http://a.example.com/icon.svg",
        "data:text/html;base64,PGI+",
        "data:image/gif;base64,R0lGODlh",
        "data:image/svg+xml,<svg/>",
        "data:image/svg+xml;base64,",
        "data:image/svg+xml;base64,PHN2Zy8+!",
        "data:image/svg+xml;base64,PHN2Zy8",
        "data:image/svg+xml;base64,PH=2Zy8+",
        "javascript:alert(1)",
        "",
        long.as_str(),
    ] {
        let got = lines(load(&at(bad), &env()));
        assert_eq!(got.len(), 1, "{bad}: {got:?}");
        assert!(got[0].starts_with("toolServers[0].icon:"), "{bad}: {got:?}");
    }
}

#[test]
fn a_url_is_http_with_a_host_and_no_credential_in_it() {
    let at = |url: &str| file(&format!("  - id: a\n    name: A\n    url: '{url}'\n"));
    for good in [
        "http://search:8080/mcp",
        "https://search.example.com",
        "https://search.example.com:8443/a/b",
    ] {
        load(&at(good), &env()).unwrap();
    }
    for bad in [
        "search.example.com",
        "ftp://search.example.com",
        "https://u:p@search.example.com/mcp",
        "https://u@search.example.com/mcp",
        "https://search.example.com/mcp?token=abc",
        "https://search.example.com/mcp#frag",
    ] {
        let got = lines(load(&at(bad), &env()));
        assert_eq!(got.len(), 1, "{bad}: {got:?}");
        assert!(got[0].starts_with("toolServers[0].url:"), "{bad}: {got:?}");
    }
}

#[test]
fn the_headers_the_client_owns_are_refused_whatever_their_case() {
    for name in [
        "Authorization",
        "authorization",
        "Accept",
        "CONTENT-TYPE",
        "Host",
        "Mcp-Session-Id",
        "mcp-protocol-version",
        "Last-Event-ID",
    ] {
        let text = file(&format!(
            "  - id: a\n    name: A\n    url: https://a.example.com/mcp\n    headers: {{ {name}: {{ env: SEARCH_KEY }} }}\n"
        ));
        let got = lines(load(&text, &env()));
        assert_eq!(got.len(), 1, "{name}: {got:?}");
        assert!(
            got[0].starts_with(&format!("toolServers[0].headers.{name}:")),
            "{got:?}"
        );
    }
    // a header that is not reserved, in any case, is fine; two that differ by case are not
    let ok = file(
        "  - id: a\n    name: A\n    url: https://a.example.com/mcp\n    headers: { X-Api-Key: { env: SEARCH_KEY } }\n",
    );
    load(&ok, &env()).unwrap();
    let twice = file(
        "  - id: a\n    name: A\n    url: https://a.example.com/mcp\n    headers: { X-Api-Key: { env: SEARCH_KEY }, x-api-key: { env: SEARCH_KEY } }\n",
    );
    assert_eq!(lines(load(&twice, &env())).len(), 1);
    // `bearer` and an `Authorization` header are not both there to say it twice
    let both = file(
        "  - id: a\n    name: A\n    url: https://a.example.com/mcp\n    bearer: { env: SEARCH_TOKEN }\n    headers: { Authorization: { env: SEARCH_KEY } }\n",
    );
    assert!(!lines(load(&both, &env())).is_empty());
}

#[test]
fn an_id_is_unique_and_has_no_underscore_so_a_tool_name_splits_at_the_first_double_one() {
    for bad in ["a_b", "A", "-a", "a b", "", "é", &"a".repeat(32)] {
        let text = file(&format!(
            "  - id: '{bad}'\n    name: A\n    url: https://a.example.com/mcp\n"
        ));
        let got = lines(load(&text, &env()));
        assert!(
            got.iter().any(|l| l.starts_with("toolServers[0].id")),
            "{bad:?}: {got:?}"
        );
    }
    for good in ["a", "0", "web-search", &"a".repeat(31)] {
        let text = file(&format!(
            "  - id: '{good}'\n    name: A\n    url: https://a.example.com/mcp\n"
        ));
        load(&text, &env()).unwrap();
    }
    let twice = file(
        "  - id: a\n    name: A\n    url: https://a.example.com/mcp\n  - id: a\n    name: B\n    url: https://b.example.com/mcp\n",
    );
    assert_eq!(
        lines(load(&twice, &env())),
        ["toolServers[1].id: another server has this id"]
    );
}

#[test]
fn the_timeout_the_tools_and_the_agents_have_bounds() {
    let at = |extra: &str| {
        file(&format!(
            "  - id: a\n    name: A\n    url: https://a.example.com/mcp\n{extra}"
        ))
    };
    load(&at("    timeoutSecs: 1\n"), &env()).unwrap();
    load(&at("    timeoutSecs: 600\n"), &env()).unwrap();
    for bad in ["0", "601", "-1"] {
        let errors = load(&at(&format!("    timeoutSecs: {bad}\n")), &env()).unwrap_err();
        assert_eq!(errors.len(), 1, "{bad}");
        assert_eq!(errors[0].path, "toolServers[0].timeoutSecs", "{bad}");
        assert!(matches!(
            errors[0].kind,
            ErrorKind::TooSmall { .. } | ErrorKind::TooLarge { .. }
        ));
    }
    // a tool the relay could not expose is a mistake, and so is a name that makes `<id>__<tool>`
    // longer than 64
    let long_tool = "t".repeat(63);
    let got = lines(load(&at(&format!("    tools: [{long_tool}]\n")), &env()));
    assert!(got[0].starts_with("toolServers[0].tools[0]"), "{got:?}");
    load(&at("    tools: [search, Fetch_Page-2]\n"), &env()).unwrap();
    // unknown members are errors like everywhere else
    let got = lines(load(&at("    token: abc\n"), &env()));
    assert_eq!(got, ["toolServers[0].token: unknown key"]);
    // missing ones too
    let got = lines(load(&file("  - id: a\n"), &env()));
    assert!(
        got.contains(&"toolServers[0].name: required key is missing".to_owned()),
        "{got:?}"
    );
    assert!(
        got.contains(&"toolServers[0].url: required key is missing".to_owned()),
        "{got:?}"
    );
}

#[test]
fn an_empty_list_of_tools_or_agents_is_a_shape_error_and_leaving_the_key_out_is_the_way_to_say_all()
{
    for key in ["tools", "agents"] {
        let text = file(&format!(
            "  - id: a\n    name: A\n    url: https://a.example.com/mcp\n    {key}: []\n"
        ));
        let errors = load(&text, &env()).unwrap_err();
        assert_eq!(errors.len(), 1, "{key}");
        assert_eq!(errors[0].path, format!("toolServers[0].{key}"));
        assert!(matches!(errors[0].kind, ErrorKind::TooFewItems { min: 1 }));
    }
}

#[test]
fn at_most_sixty_four_servers() {
    let many: String = (0..65)
        .map(|n| format!("  - id: s{n}\n    name: S\n    url: https://a.example.com/mcp\n"))
        .collect();
    let errors = load(&file(&many), &env()).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].path, "toolServers");
    assert!(matches!(
        errors[0].kind,
        ErrorKind::TooManyItems { max: 64 }
    ));
    let fits: String = (0..64)
        .map(|n| format!("  - id: s{n}\n    name: S\n    url: https://a.example.com/mcp\n"))
        .collect();
    assert_eq!(
        load(&file(&fits), &env())
            .unwrap()
            .config
            .tool_servers
            .len(),
        64
    );
}

#[test]
fn no_error_carries_a_value() {
    const SECRET: &str = "S3CR3T-VALUE-THAT-MUST-NOT-LEAK";
    let files = [
        format!(
            "{HEAD}toolServers:\n  - id: '{SECRET}'\n    name: '{SECRET}\\n'\n    url: 'https://u:{SECRET}@a.example.com/{SECRET}?k={SECRET}'\n    icon: '{SECRET}'\n    bearer: {SECRET}\n    headers: {{ X-Key: {SECRET} }}\n    tools: ['{SECRET}']\n    agents: [' {SECRET}']\n    timeoutSecs: '{SECRET}'\n"
        ),
        format!(
            "{HEAD}toolServers:\n  - id: a\n    name: A\n    url: https://a.example.com/mcp\n    bearer: {{ env: SHORT }}\n    headers: {{ Accept: {{ env: SHORT }} }}\n"
        ),
        format!("{HEAD}toolServers: {SECRET}\n"),
        format!("{HEAD}toolServers: [{SECRET}]\n"),
    ];
    let fake = env().env("SHORT", SECRET);
    for text in files {
        let errors = match load(&text, &fake) {
            Ok(_) => panic!("expected errors for {text}"),
            Err(errors) => errors,
        };
        let shown = format!("{} {errors:?}", render(&errors));
        assert!(
            !shown.contains(SECRET),
            "a value reached an error:\n{shown}\nfor:\n{text}"
        );
    }
}
