//! What only the platform's registry has to say, over real HTTP against an in-process registry:
//! the cache headers it honours, the validators it sends, the one fetch concurrent readers share,
//! and the ways a read fails (always closed, never from a stale copy).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use orch_core::AgentSource;
use orch_ports::{AgentRegistry, AgentTransport};
use orch_registry_platform::{BuildError, PlatformConfig, PlatformRegistry};
use secrecy::SecretString;
use serde_json::json;
use support::*;

const REFUSED: &str = "the registry refused the orchestrator's credentials";
const UNREACHABLE: &str = "the registry could not be reached";
const UNREADABLE: &str = "a registry document this build cannot read";

fn registry_for(server: &Server) -> PlatformRegistry {
    PlatformRegistry::new(PlatformConfig::new(server.url.clone()).without_system_proxy()).unwrap()
}

async fn served(registry: &PlatformRegistry) -> Vec<String> {
    let listing = registry.list().await;
    assert!(listing.is_complete(), "{:?}", listing.sources);
    listing.entries.iter().map(|e| e.id().to_string()).collect()
}

async fn unavailable(registry: &PlatformRegistry) -> String {
    let listing = registry.list().await;
    assert!(listing.entries.is_empty(), "{:?}", listing.entries);
    let down: Vec<_> = listing.unavailable().collect();
    assert_eq!(down.len(), 1, "{:?}", listing.sources);
    assert_eq!(down[0].name, "platform");
    down[0].detail.clone().expect("a detail")
}

// ---------------------------------------------------------------- what the entries are

#[tokio::test]
async fn an_entry_is_an_a2a_endpoint_at_the_item_with_its_title_and_tags() {
    let server = Server::start().await;
    let mut coder = item("coder", "Coder");
    coder["tags"] = json!(["coding", "git"]);
    server.set_items(vec![coder, item("researcher", "Researcher")]);
    let registry = registry_for(&server);

    let listing = registry.list().await;
    assert_eq!(ids(&listing.entries), ["coder", "researcher"]);
    let coder = &listing.entries[0];
    assert_eq!(coder.name, "Coder");
    assert_eq!(coder.tags, ["coding", "git"]);
    assert_eq!(coder.origin, AgentSource::Registry);
    assert!(registry_origin(coder));
    match &endpoint_of(coder).transport {
        AgentTransport::A2a { card_url, bearer } => {
            assert_eq!(card_url, "http://coder.test/.well-known/agent-card.json");
            assert_eq!(*bearer, None, "no agent token configured");
        }
        other => panic!("{other:?}"),
    }
    assert!(listing.sources.iter().all(|s| s.name == "platform"));
}

#[tokio::test]
async fn every_listed_agent_is_sent_the_agent_token_and_it_is_never_printed() {
    let server = Server::start().await;
    server.set_items(vec![item("coder", "Coder")]);
    let cfg = PlatformConfig::new(server.url.clone())
        .without_system_proxy()
        .with_token(SecretString::from("registry-secret-0123"))
        .with_agent_token(SecretString::from("agent-secret-4567"));
    let debug_of_config = format!("{cfg:?}");
    let registry = PlatformRegistry::new(cfg).unwrap();
    server.script(|s| s.token = Some("registry-secret-0123".to_owned()));

    let listing = registry.list().await;
    match &endpoint_of(&listing.entries[0]).transport {
        AgentTransport::A2a { bearer, .. } => {
            assert_eq!(bearer.as_deref(), Some("agent-secret-4567"));
        }
        other => panic!("{other:?}"),
    }
    for text in [
        debug_of_config,
        format!("{registry:?}"),
        format!("{listing:?}"),
    ] {
        assert!(!text.contains("registry-secret"), "{text}");
        assert!(!text.contains("agent-secret"), "{text}");
    }
}

#[tokio::test]
async fn the_request_asks_for_a_linkset_and_carries_the_registry_token() {
    let server = Server::start().await;
    let cfg = PlatformConfig::new(server.url.clone())
        .without_system_proxy()
        .with_token(SecretString::from("tok-123456"));
    let registry = PlatformRegistry::new(cfg).unwrap();
    registry.list().await;
    let seen = &server.journal()[0];
    assert_eq!(seen.accept.as_deref(), Some("application/linkset+json"));
    assert_eq!(seen.authorization.as_deref(), Some("Bearer tok-123456"));
    assert_eq!(seen.if_none_match, None);

    // No token configured: no Authorization header at all.
    let plain = registry_for(&server);
    plain.list().await;
    assert_eq!(server.journal()[1].authorization, None);
}

// ---------------------------------------------------------------- freshness

#[tokio::test]
async fn a_max_age_serves_the_copy_without_asking_again() {
    let server = Server::start().await;
    server.set_items(vec![item("coder", "Coder")]);
    server.script(|s| s.cache_control = Some("private, max-age=30".into()));
    let registry = registry_for(&server);
    for _ in 0..5 {
        assert_eq!(served(&registry).await, ["coder"]);
    }
    assert_eq!(server.hits(), 1);
    assert!(registry.get(&agent_id("coder")).await.unwrap().is_some());
    assert!(registry.get(&agent_id("other")).await.unwrap().is_none());
    assert_eq!(server.hits(), 1, "get reads the same copy");
}

#[tokio::test]
async fn no_store_and_no_max_age_ask_every_time() {
    let server = Server::start().await;
    server.set_items(vec![item("coder", "Coder")]);
    let registry = registry_for(&server);
    for cache_control in [
        Some("no-store"),
        Some("no-cache, max-age=30"),
        None,
        Some("private"),
    ] {
        let before = server.hits();
        server.script(|s| s.cache_control = cache_control.map(str::to_owned));
        served(&registry).await;
        served(&registry).await;
        assert_eq!(server.hits(), before + 2, "{cache_control:?}");
    }
    // A change shows on the very next read.
    server.push(item("writer", "Writer"));
    assert_eq!(served(&registry).await, ["coder", "writer"]);
}

#[tokio::test]
async fn a_copy_that_is_stale_is_asked_about_with_its_etag_and_a_304_renews_it() {
    let server = Server::start().await;
    server.set_items(vec![item("coder", "Coder")]);
    // Age = max-age: stale on arrival, so the next read has to ask.
    server.script(|s| {
        s.cache_control = Some("max-age=1".into());
        s.age = Some("1".into());
        s.etag = Some("\"r-1\"".into());
    });
    let registry = registry_for(&server);
    assert_eq!(served(&registry).await, ["coder"]);
    assert_eq!(served(&registry).await, ["coder"]);
    let journal = server.journal();
    assert_eq!(journal.len(), 2);
    assert_eq!(journal[0].if_none_match, None);
    assert_eq!(journal[1].if_none_match.as_deref(), Some("\"r-1\""));

    // The 304 states freshness of its own: the copy is renewed, and the next reads cost nothing.
    server.script(|s| {
        s.cache_control = Some("max-age=60".into());
        s.age = None;
    });
    assert_eq!(served(&registry).await, ["coder"]);
    assert_eq!(server.hits(), 3);
    assert_eq!(
        server.journal()[2].if_none_match.as_deref(),
        Some("\"r-1\"")
    );
    for _ in 0..3 {
        assert_eq!(served(&registry).await, ["coder"]);
    }
    assert_eq!(server.hits(), 3, "renewed by the 304");
}

#[tokio::test]
async fn a_changed_document_replaces_the_copy_on_revalidation() {
    let server = Server::start().await;
    server.set_items(vec![item("coder", "Coder")]);
    server.script(|s| {
        s.cache_control = Some("max-age=0".into());
        s.etag = Some("\"r-1\"".into());
    });
    let registry = registry_for(&server);
    assert_eq!(served(&registry).await, ["coder"]);
    server.push(item("writer", "Writer"));
    server.script(|s| s.etag = Some("\"r-2\"".into()));
    assert_eq!(served(&registry).await, ["coder", "writer"]);
    // The new validator is the one sent next.
    served(&registry).await;
    assert_eq!(
        server.journal()[2].if_none_match.as_deref(),
        Some("\"r-2\"")
    );
}

#[tokio::test]
async fn last_modified_is_the_validator_when_there_is_no_etag() {
    let server = Server::start().await;
    server.set_items(vec![item("coder", "Coder")]);
    server.script(|s| {
        s.cache_control = Some("max-age=0".into());
        s.last_modified = Some("Wed, 01 Oct 2026 09:00:00 GMT".into());
    });
    let registry = registry_for(&server);
    served(&registry).await;
    assert_eq!(served(&registry).await, ["coder"]);
    let second = &server.journal()[1];
    assert_eq!(second.if_none_match, None);
    assert_eq!(
        second.if_modified_since.as_deref(),
        Some("Wed, 01 Oct 2026 09:00:00 GMT")
    );
}

#[tokio::test]
async fn the_cap_bounds_how_long_a_copy_is_fresh_whatever_the_registry_allows() {
    let server = Server::start().await;
    server.set_items(vec![item("coder", "Coder")]);
    server.script(|s| s.cache_control = Some("max-age=3600".into()));
    let cfg = PlatformConfig::new(server.url.clone())
        .without_system_proxy()
        .with_max_age(Duration::from_secs(1));
    let registry = PlatformRegistry::new(cfg).unwrap();
    served(&registry).await;
    served(&registry).await;
    assert_eq!(server.hits(), 1);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    served(&registry).await;
    assert_eq!(server.hits(), 2, "an hour is more than the cap");
}

// ---------------------------------------------------------------- failing closed

#[tokio::test]
async fn a_stale_copy_is_never_served_when_the_registry_is_down() {
    let server = Server::start().await;
    server.set_items(vec![item("coder", "Coder")]);
    server.script(|s| {
        s.cache_control = Some("max-age=0, stale-if-error=600, stale-while-revalidate=600".into());
        s.etag = Some("\"r-1\"".into());
    });
    let registry = registry_for(&server);
    assert_eq!(served(&registry).await, ["coder"]);

    server.script(|s| s.mode = Mode::Status(503));
    assert_eq!(unavailable(&registry).await, UNREACHABLE);
    let err = registry.get(&agent_id("coder")).await.unwrap_err();
    assert!(err.to_string().contains("platform"), "{err}");

    // The copy is gone with its validator: the next read asks with nothing to ask about.
    server.script(|s| s.mode = Mode::Serve);
    assert_eq!(served(&registry).await, ["coder"]);
    let journal = server.journal();
    assert_eq!(journal.last().unwrap().if_none_match, None);
}

#[tokio::test]
async fn a_fresh_copy_is_served_without_a_request_even_while_the_registry_is_down() {
    // Fresh is fresh: nothing was fetched, so nothing failed. It is the stale copy that is never
    // served.
    let server = Server::start().await;
    server.set_items(vec![item("coder", "Coder")]);
    server.script(|s| s.cache_control = Some("max-age=60".into()));
    let registry = registry_for(&server);
    served(&registry).await;
    server.script(|s| s.mode = Mode::Status(503));
    assert_eq!(served(&registry).await, ["coder"]);
    assert_eq!(server.hits(), 1);
}

#[tokio::test]
async fn a_failure_is_one_of_five_things_and_says_which_without_a_url() {
    let server = Server::start().await;
    server.set_items(vec![item("coder", "Coder")]);
    let registry = registry_for(&server);

    // 401 and 403: the credentials.
    server.script(|s| s.token = Some("right".into()));
    assert_eq!(unavailable(&registry).await, REFUSED);
    server.script(|s| {
        s.token = None;
        s.mode = Mode::Status(403);
    });
    assert_eq!(unavailable(&registry).await, REFUSED);
    // Another status, a redirect (never followed), a server that does not answer in time.
    for mode in [
        Mode::Status(500),
        Mode::Status(404),
        Mode::Status(204),
        Mode::Redirect,
    ] {
        server.script(|s| s.mode = mode);
        assert_eq!(unavailable(&registry).await, UNREACHABLE);
    }
    // Back.
    server.script(|s| s.mode = Mode::Serve);
    assert_eq!(served(&registry).await, ["coder"]);
}

#[tokio::test]
async fn a_server_that_does_not_answer_in_time_is_unavailable() {
    let server = Server::start().await;
    server.script(|s| s.mode = Mode::Hang);
    let cfg = PlatformConfig::new(server.url.clone())
        .without_system_proxy()
        .with_timeout(Duration::from_millis(300));
    let registry = PlatformRegistry::new(cfg).unwrap();
    let started = std::time::Instant::now();
    assert_eq!(unavailable(&registry).await, UNREACHABLE);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn nothing_listening_is_unavailable_and_no_text_names_the_address() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let url = format!("http://127.0.0.1:{port}/registry/v1/agents?secret=hunter2");
    let registry = PlatformRegistry::new(PlatformConfig::new(url).without_system_proxy()).unwrap();
    let detail = unavailable(&registry).await;
    assert_eq!(detail, UNREACHABLE);
    let err = registry.get(&agent_id("coder")).await.unwrap_err();
    let everything = format!("{err} {}", orch_core::report(&err));
    assert!(!everything.contains("hunter2"), "{everything}");
    assert!(!everything.contains(&port.to_string()), "{everything}");
    let debug = format!("{registry:?}");
    assert!(!debug.contains("hunter2"), "{debug}");
}

#[tokio::test]
async fn a_document_this_build_cannot_read_makes_the_source_unavailable_not_empty() {
    let server = Server::start().await;
    let registry = registry_for(&server);
    let linkset = |profile: &str| {
        json!({"linkset": [{"profile": [{"href": profile}],
               "item": [item("coder", "Coder")]}]})
        .to_string()
        .into_bytes()
    };
    let linkset_json = "application/linkset+json";
    let cases: Vec<(&str, String, Vec<u8>)> = vec![
        (
            "another version",
            linkset_json.into(),
            linkset("https://agents.vymalo.com/registry/v2"),
        ),
        (
            "not json",
            linkset_json.into(),
            b"<html>login</html>".to_vec(),
        ),
        ("no linkset", linkset_json.into(), b"{}".to_vec()),
        (
            "wrong media type",
            "application/json".into(),
            linkset(PROFILE),
        ),
        ("html", "text/html".into(), linkset(PROFILE)),
        (
            "over 1 MiB",
            linkset_json.into(),
            vec![b' '; 1024 * 1024 + 1],
        ),
    ];
    for (what, content_type, body) in cases {
        server.script(|s| s.mode = Mode::Raw { content_type, body });
        assert_eq!(unavailable(&registry).await, UNREADABLE, "{what}");
    }
    // The same document with the right profile and media type reads, whatever the parameters or
    // the case of the media type.
    for content_type in [
        "application/linkset+json",
        "Application/Linkset+JSON; charset=utf-8",
        "application/linkset+json; profile=\"https://www.rfc-editor.org/info/rfc9727 https://agents.vymalo.com/registry/v1\"",
    ] {
        server.script(|s| {
            s.mode = Mode::Raw {
                content_type: content_type.into(),
                body: linkset(PROFILE),
            };
        });
        assert_eq!(served(&registry).await, ["coder"], "{content_type}");
    }
}

#[tokio::test]
async fn a_304_nobody_asked_for_is_unavailable() {
    let server = Server::start().await;
    server.script(|s| s.mode = Mode::Status(304));
    let registry = registry_for(&server);
    assert_eq!(unavailable(&registry).await, UNREACHABLE);
}

#[tokio::test]
async fn a_registry_that_lists_nothing_is_available_and_empty() {
    let server = Server::start().await;
    let registry = registry_for(&server);
    let listing = registry.list().await;
    assert!(listing.entries.is_empty());
    assert!(listing.is_complete(), "empty is not unavailable");
    assert!(registry.get(&agent_id("coder")).await.unwrap().is_none());
}

#[tokio::test]
async fn items_this_build_skips_leave_the_rest_and_the_source_available() {
    let server = Server::start().await;
    let mut tagged = item("tagged", "Tagged");
    tagged["tags"] = json!("not-an-array");
    server.set_items(vec![
        item("first", "First"),
        json!({"href": "/relative", "service": ["relative"]}),
        item("first", "Again"),
        json!({"href": "http://u:p@x.test/card", "service": ["creds"]}),
        tagged,
    ]);
    let registry = registry_for(&server);
    let listing = registry.list().await;
    assert_eq!(ids(&listing.entries), ["first", "tagged"]);
    assert_eq!(listing.entries[0].name, "First");
    assert!(listing.entries[1].tags.is_empty());
    assert!(listing.is_complete());
}

// ---------------------------------------------------------------- single flight

#[tokio::test]
async fn concurrent_readers_share_one_fetch() {
    let server = Server::start().await;
    server.set_items(vec![item("coder", "Coder")]);
    server.script(|s| s.delay = Duration::from_millis(300));
    let registry = registry_for(&server);
    let readers: Vec<_> = (0..8)
        .map(|_| {
            let registry = registry.clone();
            tokio::spawn(async move { registry.list().await })
        })
        .collect();
    for reader in readers {
        let listing = reader.await.unwrap();
        assert_eq!(ids(&listing.entries), ["coder"]);
    }
    assert_eq!(
        server.hits(),
        1,
        "no-store, and still one request for eight readers"
    );
}

#[tokio::test]
async fn concurrent_readers_share_a_failure_and_do_not_each_wait_for_their_own() {
    let server = Server::start().await;
    server.script(|s| {
        s.mode = Mode::Status(503);
        s.delay = Duration::from_millis(300);
    });
    let registry = registry_for(&server);
    let started = std::time::Instant::now();
    let readers: Vec<_> = (0..8)
        .map(|_| {
            let registry = registry.clone();
            tokio::spawn(async move { registry.get(&agent_id("coder")).await })
        })
        .collect();
    for reader in readers {
        assert!(reader.await.unwrap().is_err());
    }
    assert_eq!(server.hits(), 1);
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "eight readers did not queue behind eight fetches"
    );
}

// ---------------------------------------------------------------- building it

#[tokio::test]
async fn a_url_that_is_not_an_absolute_http_url_without_credentials_is_refused() {
    for bad in [
        "",
        "   ",
        "registry",
        "/registry/v1/agents",
        "ftp://platform.example.com/registry",
        "file:///etc/passwd",
        "https://",
        "https://user:pw@platform.example.com/registry/v1/agents",
        "https://user@platform.example.com/registry/v1/agents",
    ] {
        assert!(
            matches!(
                PlatformRegistry::new(PlatformConfig::new(bad)),
                Err(BuildError::BadUrl)
            ),
            "{bad:?} must be refused"
        );
    }
    for good in [
        "https://platform.example.com/registry/v1/agents",
        "http://platform.agents.svc:8080/registry/v1/agents",
        "  https://platform.example.com/registry/v1/agents?tenant=a  ",
    ] {
        PlatformRegistry::new(PlatformConfig::new(good)).unwrap();
    }
}

#[tokio::test]
async fn a_token_that_cannot_be_a_header_is_refused_and_an_empty_one_is_none() {
    let url = "https://platform.example.com/registry/v1/agents";
    let bad = |cfg: PlatformConfig| PlatformRegistry::new(cfg).unwrap_err().to_string();
    assert_eq!(
        bad(PlatformConfig::new(url).with_token(SecretString::from("a\nb"))),
        "the registry token cannot be sent as a header"
    );
    assert_eq!(
        bad(PlatformConfig::new(url).with_agent_token(SecretString::from("a\nb"))),
        "the agent token cannot be sent as a header"
    );
    PlatformRegistry::new(
        PlatformConfig::new(url)
            .with_token(SecretString::from(""))
            .with_agent_token(SecretString::from("")),
    )
    .unwrap();
}
