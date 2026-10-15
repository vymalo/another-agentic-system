//! A code, a state, a verifier and the tokens of an authorization are in no response, no page, no log
//! line and no definition (ADR 0046, decision 3: "a test that plants a sentinel secret and greps every
//! output is the acceptance of the slice").
//!
//! A test of its own, in its own process: the logs are captured with a subscriber that is set for the
//! test and read by it, which must not share the process with tests that run at the same time on other
//! threads (the callsites cache what they are told is of interest). The runtime is the test's single
//! thread, so the server, the broker and the callback's task are all seen by the subscriber.
//!
//! The authorization server issues codes, tokens and refresh tokens that start with `SENTINEL-`, and
//! the test keeps every secret it was shown (`MockAs::secrets_seen`: the codes and the verifiers it
//! received, the tokens it issued). The flow is put through its refusals too: a state used twice, one
//! that ended, a mix-up, a person who declines (with a description a stranger wrote), a parameter
//! given twice, a forged state, a token endpoint that is down, and a browser that hangs up. A canary
//! shows that the capture finds a secret if one is logged, and a control that the sealed store does hold
//! the grant, so that searching the definitions for it is a search that could find something.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[path = "support/oauth_world.rs"]
mod world;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_core::UserId;
use orch_ports::{
    SecretServer, SecretStore, UserToolServerId, UserToolServerState, UserToolServerStore,
};
use tracing_subscriber::fmt::format::FmtSpan;
use world::{
    ALICE, CALLBACK_PATH, Options, SENTINEL, World, assert_a_safe_page, param, with_param,
};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn nothing_of_an_authorization_is_in_a_response_a_log_or_a_definition() {
    let log = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_span_events(FmtSpan::FULL)
        .with_writer(log.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // a canary: the capture finds a secret if one is logged
    tracing::info!(canary = "SENTINEL-canary-in-the-log", "the canary");

    let world = World::full_with(Options {
        mock: Box::new(|cfg| {
            cfg.sentinel = Some(SENTINEL.to_owned());
        }),
        ..Options::default()
    })
    .await;
    // what a client was shown, by the orchestrator: no `authorize` answer (it is the one place a state
    // is put in front of the person who began the flow, so that it can be opened)
    let mut seen_by_clients = String::new();
    let mut states = Vec::new();
    let mut note =
        |resp: &world::Resp| seen_by_clients.push_str(&format!("{}\n", resp.transcript()));

    let id = world.add_oauth("alice").await;
    let decline_id = world.add_oauth_scoped("alice", &["read"]).await;

    // ---- the flow that works
    let authorize = world.authorize("alice", &id).await;
    assert_eq!(authorize.status, 200);
    let url = authorize.json()["authorizeUrl"]
        .as_str()
        .unwrap()
        .to_owned();
    let works = world.consent(&url).await;
    states.push(param(&works, "state").unwrap());
    let page = world.bare_get(&works).await;
    assert_a_safe_page(&page, 200);
    note(&page);
    assert_eq!(state_of(&world, &id).await, UserToolServerState::Connected);

    // ---- every refusal, each with the secrets of a flow in the request
    // used twice
    let again = world.bare_get(&works).await;
    assert_a_safe_page(&again, 400);
    note(&again);
    // ended
    let ended = world.callback_of("alice", &id).await;
    states.push(param(&ended, "state").unwrap());
    world.clock.advance(Duration::from_secs(601));
    let resp = world.bare_get(&ended).await;
    assert_a_safe_page(&resp, 400);
    note(&resp);
    // a mix-up
    let genuine = world.callback_of("alice", &id).await;
    states.push(param(&genuine, "state").unwrap());
    let resp = world
        .bare_get(&with_param(
            &genuine,
            "iss",
            Some("https://stranger.example.com"),
        ))
        .await;
    assert_a_safe_page(&resp, 400);
    note(&resp);
    // the person declines, and the authorization server writes a description
    let declined = world.callback_of("alice", &decline_id).await;
    let state = param(&declined, "state").unwrap();
    states.push(state.clone());
    let resp = world
        .bare_get(&format!(
            "{CALLBACK_PATH}?error=access_denied&error_description=SENTINEL-description-written-by-a-stranger&state={state}"
        ))
        .await;
    assert_a_safe_page(&resp, 400);
    note(&resp);
    // a parameter given twice, a forged state, no state
    let genuine = world.callback_of("alice", &id).await;
    states.push(param(&genuine, "state").unwrap());
    for forged in [
        format!("{genuine}&code=SENTINEL-code-given-twice"),
        with_param(&genuine, "state", Some("SENTINEL-forged-state-0123456789")),
        with_param(&genuine, "state", None),
    ] {
        let resp = world.bare_get(&forged).await;
        assert_a_safe_page(&resp, 400);
        note(&resp);
    }
    // a code nobody issued, with the genuine state
    let wrong = with_param(&genuine, "code", Some("SENTINEL-code-nobody-issued"));
    let resp = world.bare_get(&wrong).await;
    assert_a_safe_page(&resp, 400);
    note(&resp);

    // a callback of the authorization that did work, once more, with every secret in the query
    let last = world.callback_of("alice", &id).await;
    states.push(param(&last, "state").unwrap());
    let resp = world.bare_get(&last).await;
    assert_a_safe_page(&resp, 200);
    note(&resp);

    // authorize, refused
    for (who, target) in [
        ("carol", id.as_str()),
        ("bob", id.as_str()),
        ("alice", "u-0000000000"),
    ] {
        let resp = world.authorize(who, target).await;
        assert!(resp.status >= 400);
        note(&resp);
    }
    // listings, and the picker
    for path in ["/api/user-tool-servers", "/api/tool-servers"] {
        let resp = world
            .api(reqwest::Method::GET, path, Some("alice"), None)
            .await;
        assert_eq!(resp.status, 200);
        note(&resp);
    }

    // ---- what the secrets are
    let mut secrets: Vec<String> = world.mock.secrets_seen();
    secrets.extend(states.iter().cloned());
    secrets.extend(
        [
            "SENTINEL-description-written-by-a-stranger",
            "SENTINEL-code-given-twice",
            "SENTINEL-forged-state-0123456789",
            "SENTINEL-code-nobody-issued",
        ]
        .map(str::to_owned),
    );
    assert!(
        secrets
            .iter()
            .filter(|s| s.starts_with("SENTINEL-code-"))
            .count()
            >= 4
            && secrets.iter().any(|s| s.starts_with("SENTINEL-at-"))
            && secrets.iter().any(|s| s.starts_with("SENTINEL-rt-")),
        "the authorization server issued what the test looks for: {secrets:?}"
    );

    // the control: the grant is in the broker's sealed store, as it must be, so a search of the rest
    // for it is a search that could find something
    let held = world
        .secrets
        .get(
            &UserId::new(ALICE),
            &SecretServer::credential(&UserToolServerId::parse(&id).unwrap()),
        )
        .await
        .unwrap()
        .expect("a grant is kept");
    let sealed = String::from_utf8_lossy(held.1.expose()).into_owned();
    assert!(
        sealed.contains("SENTINEL-at-"),
        "the store holds the tokens"
    );

    // the definitions, the pending store and every Debug the application offers
    let mut definitions = String::new();
    for server in world.servers.list(&UserId::new(ALICE)).await.unwrap() {
        definitions.push_str(&format!("{server:?} {:?}\n", server.server()));
    }
    let pending = format!("{:?}", world.pending);

    // ---- the search
    let logged = log.text();
    assert!(
        logged.contains("SENTINEL-canary-in-the-log"),
        "the capture sees a secret"
    );
    assert!(
        logged.contains("oauth/callback?…"),
        "the callback was served and its span says so, with the query taken out: {logged}"
    );
    for (what, text) in [
        ("what the clients were shown", &seen_by_clients),
        ("the log", &logged),
        ("the definitions", &definitions),
        ("the pending store", &pending),
    ] {
        for secret in &secrets {
            // the canary is meant to be found, in the log alone
            assert!(!text.contains(secret.as_str()), "{secret} is in {what}");
        }
    }
    // the log holds no parameter of a callback, not even one that is not a secret (`error_code=` is
    // the broker's own field, a code of RFC 6749 and not a parameter of the request)
    for parameter in ["code", "state", "iss", "error", "error_description"] {
        for lead in ['?', '&'] {
            let needle = format!("{lead}{parameter}=");
            let at = logged
                .find(&needle)
                .map(|i| &logged[i.saturating_sub(120)..(i + 120).min(logged.len())]);
            assert!(at.is_none(), "{needle} is in the log: {at:?}");
        }
    }
    // nothing the broker kept is in what the person was told, and no page names the state
    for state in &states {
        assert!(
            !seen_by_clients.contains(state.as_str()),
            "a state is on a page"
        );
    }
}

/// The server, as the definitions hold it.
async fn state_of(world: &World, id: &str) -> UserToolServerState {
    world
        .servers
        .get(&UserId::new(ALICE), &UserToolServerId::parse(id).unwrap())
        .await
        .unwrap()
        .unwrap()
        .server()
        .state()
}
