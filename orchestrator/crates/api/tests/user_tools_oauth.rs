//! Authorizing a person's own `oauth2` MCP server over real HTTP (ADR 0046, slice 4c): the whole flow
//! (add, authorize, the person's browser at the authorization server, the callback, `connected`), and
//! everything that can go wrong on the way, with the real credential broker against an authorization
//! server of the tests' own (`orch-credential-broker`'s test support) and the real router.
//!
//! The browser here is a client that sends **nothing** to the callback but the request line: no
//! bearer token, no cookie (ADR 0047). That the logs and the definitions hold no code, state or token
//! is `user_tools_oauth_secret.rs`, a test of its own process.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

#[path = "support/oauth_world.rs"]
mod world;

use std::time::Duration;

use orch_api::{ApiConfig, AuthorizeLimits, ClientMetadata, PublicLimits};
use orch_core::UserId;
use orch_ports::{UserToolServerId, UserToolServerState, UserToolServerStore};
use serde_json::json;
use world::{
    ALICE, BOB, CALLBACK_PATH, Options, SENTINEL, World, assert_a_safe_page, param, with_param,
};

const FLOW_SECONDS: i64 = 600;

fn id_of(text: &str) -> UserToolServerId {
    UserToolServerId::parse(text).unwrap()
}

async fn state_of(world: &World, owner: &str, id: &str) -> UserToolServerState {
    world
        .servers
        .get(&UserId::new(owner), &id_of(id))
        .await
        .unwrap()
        .unwrap()
        .server()
        .state()
}

// ------------------------------------------------------------------------------------ the flow

#[tokio::test]
async fn a_person_adds_authorizes_comes_back_and_the_server_is_connected() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;

    // before: saved, nothing to offer, a call to action
    let listed = world.listed("alice", &id).await;
    assert_eq!(listed["state"], "added");
    assert_eq!(listed["status"], "needs_auth");
    assert_eq!(listed["reason"], "not_connected");
    assert_eq!(listed["hasCredential"], false);

    // authorize: the URL to send the person to, the authorization server it is at, and when it ends
    let resp = world.authorize("alice", &id).await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_eq!(resp.header("cache-control"), Some("no-store"));
    let body = resp.json();
    let authorize_url = body["authorizeUrl"].as_str().unwrap();
    assert!(
        authorize_url.starts_with(&format!("{}/authorize?", world.mock.origin())),
        "{authorize_url}"
    );
    assert_eq!(body["issuer"], world.mock.origin());
    let ends = world::start_time()
        .checked_add(jiff::SignedDuration::from_secs(FLOW_SECONDS))
        .unwrap();
    assert_eq!(body["expiresAt"], ends.to_string());
    // the state is inside the URL, and is not a member of its own
    assert_eq!(body.as_object().unwrap().len(), 3, "{body}");
    assert!(body.get("state").is_none());
    let url = url::Url::parse(authorize_url).unwrap();
    let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["resource"], world.mock.mcp_url());
    assert_eq!(
        query["redirect_uri"],
        format!("{}{CALLBACK_PATH}", world.base),
        "the redirect is the orchestrator's callback"
    );
    assert!(query.contains_key("state"));
    // authorizing wrote nothing: the server is as it was
    assert_eq!(
        state_of(&world, ALICE, &id).await,
        UserToolServerState::Added
    );
    assert_eq!(world.pending.len(), 1);

    // the person's browser: the authorization server, then the callback with no session at all
    let callback = world.consent(authorize_url).await;
    assert!(callback.starts_with(CALLBACK_PATH), "{callback}");
    let page = world.bare_get(&callback).await;
    let text = assert_a_safe_page(&page, 200);
    assert!(text.contains("You can return to the app"), "{text}");

    // after: connected and offered
    let listed = world.listed("alice", &id).await;
    assert_eq!(listed["state"], "connected");
    assert_eq!(listed["status"], "ready");
    assert!(listed.get("reason").is_none(), "{listed}");
    assert_eq!(listed["hasCredential"], true);
    let picked = world.picked("alice", &id).await;
    assert_eq!(picked["status"], "ready", "{picked}");
    assert_eq!(picked["own"], true);
    // the state was used, and the authorization server saw one code exchanged
    assert_eq!(world.pending.len(), 0);
    assert_eq!(world.mock.requests().code_exchanges, 1);
}

#[tokio::test]
async fn the_page_says_nothing_of_the_request_the_person_or_the_server() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let page = world.bare_get(&callback).await;
    let text = assert_a_safe_page(&page, 200);
    for (name, value) in [
        ("code", param(&callback, "code").unwrap()),
        ("state", param(&callback, "state").unwrap()),
        ("iss", param(&callback, "iss").unwrap()),
        ("the owner", ALICE.to_owned()),
        ("the server's name", "GitHub".to_owned()),
        ("the server's id", id.clone()),
        ("the server's url", world.mock.mcp_url()),
    ] {
        assert!(!text.contains(&value), "{name} is on the page: {text}");
    }
}

#[tokio::test]
async fn the_callback_needs_no_credential_and_ignores_the_ones_it_is_given() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;

    // bob's bearer token, a cookie and the identity header of a proxy are no identity here: the
    // `state` says whose flow it is
    let resp = world
        .browser
        .get(format!("{}{callback}", world.base))
        .bearer_auth("bob")
        .header("cookie", "oauth2_proxy=whatever")
        .header("x-auth-request-email", BOB)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        state_of(&world, ALICE, &id).await,
        UserToolServerState::Connected
    );
    assert_eq!(
        world.servers.list(&UserId::new(BOB)).await.unwrap().len(),
        0,
        "nothing of bob's was touched"
    );
}

/// The consent-phishing review (ADR 0046, slice 4c review, departure 125): a stranger can send a person
/// the `authorizeUrl` of a server of their own, so what the person's consent page shows must say what
/// the orchestrator is and nothing of who owns the server or what they called it. Nothing that names
/// the owner, the server's id or its name is in the answer, in the URL, or in a single request the
/// authorization server received (the discovery, the registration, the authorization, the token).
#[tokio::test]
async fn nothing_that_names_the_owner_or_the_server_reaches_the_answer_or_the_authorization_server()
{
    let world = World::full().await;
    let body = json!({
        "name": "Zebra notes of Alice",
        "url": world.mock.mcp_url(),
        "auth": {"kind": "oauth2", "scopes": ["repo"]},
    });
    let resp = world
        .api(
            reqwest::Method::POST,
            "/api/user-tool-servers",
            Some("alice"),
            Some(&body),
        )
        .await;
    assert_eq!(resp.status, 201, "{}", resp.text());
    let id = resp.json()["id"].as_str().unwrap().to_owned();

    let answer = world.authorize("alice", &id).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let authorize_url = answer.json()["authorizeUrl"].as_str().unwrap().to_owned();
    let callback = world.consent(&authorize_url).await;
    assert_a_safe_page(&world.bare_get(&callback).await, 200);

    let seen = world
        .mock
        .seen()
        .iter()
        .map(|request| format!("{request:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!seen.is_empty(), "the authorization server was asked");
    let everything = format!("{}\n{authorize_url}\n{seen}", answer.text()).to_lowercase();
    for names_them in ["alice", "zebra", &id.to_lowercase(), "example.com"] {
        assert!(
            !everything.contains(names_them),
            "{names_them} reached the authorization server or the answer: {everything}"
        );
    }
    // what the consent page is built from: the client, the redirect, the resource and the scopes
    let url = url::Url::parse(&authorize_url).unwrap();
    let mut members: Vec<String> = url
        .query_pairs()
        .map(|(name, _)| name.into_owned())
        .collect();
    members.sort();
    assert_eq!(
        members,
        [
            "client_id",
            "code_challenge",
            "code_challenge_method",
            "redirect_uri",
            "resource",
            "response_type",
            "scope",
            "state"
        ]
    );
}

#[tokio::test]
async fn a_server_that_is_connected_can_be_authorized_again() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    for round in 0..2 {
        let callback = world.callback_of("alice", &id).await;
        assert_a_safe_page(&world.bare_get(&callback).await, 200);
        let listed = world.listed("alice", &id).await;
        assert_eq!(listed["status"], "ready", "round {round}");
    }
    assert_eq!(world.mock.requests().code_exchanges, 2);
}

#[tokio::test]
async fn a_grant_the_broker_ended_is_a_call_to_action_until_the_person_authorizes_again() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    assert_a_safe_page(
        &world.bare_get(&world.callback_of("alice", &id).await).await,
        200,
    );

    // the broker learns the grant is over and marks the server (its refresh was refused)
    let owner = UserId::new(ALICE);
    let stored = world
        .servers
        .get(&owner, &id_of(&id))
        .await
        .unwrap()
        .unwrap();
    world
        .servers
        .set_state(
            &owner,
            &id_of(&id),
            UserToolServerState::NeedsReauth,
            stored.revision(),
        )
        .await
        .unwrap();
    let listed = world.listed("alice", &id).await;
    assert_eq!(
        (listed["status"].as_str(), listed["reason"].as_str()),
        (Some("needs_auth"), Some("reauthorize"))
    );
    let picked = world.picked("alice", &id).await;
    assert_eq!(
        (picked["status"].as_str(), picked["reason"].as_str()),
        (Some("needs_auth"), Some("reauthorize")),
        "the picker says the same"
    );

    // and the person authorizes it again
    assert_a_safe_page(
        &world.bare_get(&world.callback_of("alice", &id).await).await,
        200,
    );
    let listed = world.listed("alice", &id).await;
    assert_eq!(
        (listed["state"].as_str(), listed["status"].as_str()),
        (Some("connected"), Some("ready"))
    );
}

// ------------------------------------------------------------------------- authorize, refused

#[tokio::test]
async fn authorize_is_refused_for_everyone_it_is_not_for_and_calls_nothing() {
    let world = World::full().await;
    let alices = world.add_oauth("alice").await;
    let none = {
        let body =
            json!({"name": "Plain", "url": world.mock.other_url(), "auth": {"kind": "none"}});
        let resp = world
            .api(
                reqwest::Method::POST,
                "/api/user-tool-servers",
                Some("alice"),
                Some(&body),
            )
            .await;
        assert_eq!(resp.status, 201, "{}", resp.text());
        resp.json()["id"].as_str().unwrap().to_owned()
    };
    let seen_before = world.mock.seen().len();

    // no permission
    let resp = world.authorize("carol", &alices).await;
    assert_eq!(
        (resp.status, resp.code().as_deref()),
        (403, Some("forbidden"))
    );
    // another person's server, one that never was, one whose id is not one, and one of a kind with
    // nothing to authorize: the same 404
    let mut bodies = Vec::new();
    for (who, id) in [
        ("bob", alices.as_str()),
        ("alice", "u-0000000000"),
        ("alice", "not-an-id"),
        ("alice", none.as_str()),
    ] {
        let resp = world.authorize(who, id).await;
        assert_eq!(resp.status, 404, "{who} {id}: {}", resp.text());
        bodies.push(resp.text());
    }
    assert!(
        bodies.windows(2).all(|w| w[0] == w[1]),
        "one answer for all: {bodies:?}"
    );
    // no credential at all
    let resp = world
        .api(
            reqwest::Method::POST,
            &format!("/api/user-tool-servers/{alices}/authorize"),
            None,
            None,
        )
        .await;
    assert_eq!(resp.status, 401);

    assert_eq!(
        world.mock.seen().len(),
        seen_before,
        "nothing was fetched from the network for any of them"
    );
    assert_eq!(world.pending.len(), 0);
}

#[tokio::test]
async fn authorize_is_a_404_with_the_feature_off_and_a_422_where_nothing_can_be_kept() {
    // the feature off
    let off = World::off().await;
    let resp = off.authorize("alice", "u-0123456789").await;
    assert_eq!(resp.status, 404);

    // enabled with no `secrets`: a server of the kind exists only from before (a `none` server is
    // the only one that can be added), so the id is the point; it is a 422 before it is a 404 only
    // for a server that is there. Add what can be added, and ask for an authorization of it.
    let world = World::without_credentials().await;
    let resp = world.authorize("alice", "u-0123456789").await;
    assert_eq!(
        (resp.status, resp.code().as_deref()),
        (422, Some("credentials_not_kept"))
    );

    // credentials are kept and the deployment has no address for the callback
    let world = World::full_with(Options {
        with_redirect: false,
        ..Options::default()
    })
    .await;
    let id = world.add_oauth("alice").await;
    let resp = world.authorize("alice", &id).await;
    assert_eq!(
        (resp.status, resp.code().as_deref()),
        (422, Some("oauth_not_configured"))
    );
}

#[tokio::test]
async fn a_url_the_guard_refuses_during_discovery_is_a_422_with_its_one_public_sentence() {
    // the server's own URL is the mock's, which the policy allows; what its metadata names as the
    // authorization server is a host the policy does not (its allow list names the mock alone)
    let world = World::full_with(Options {
        mock: Box::new(|cfg| {
            cfg.sentinel = Some(SENTINEL.to_owned());
            cfg.authorization_server = Some("http://10.1.2.3:9".to_owned());
        }),
        ..Options::default()
    })
    .await;
    let id = world.add_oauth("alice").await;
    let resp = world.authorize("alice", &id).await;
    assert_eq!(
        (resp.status, resp.code().as_deref()),
        (422, Some("egress_refused")),
        "{}",
        resp.text()
    );
    let detail = resp.json()["detail"].as_str().unwrap().to_owned();
    assert_eq!(
        detail,
        orch_ports::EgressRefusal::NotAllowed.public_reason()
    );
    // the address is not in it
    assert!(!detail.contains("10.1.2.3"), "{detail}");
    assert!(!resp.text().contains("10.1.2.3"), "{}", resp.text());
    assert_eq!(world.pending.len(), 0);
}

#[tokio::test]
async fn an_authorization_server_that_offers_no_pkce_is_a_503_and_leaves_nothing() {
    let world = World::full_with(Options {
        mock: Box::new(|cfg| {
            cfg.sentinel = Some(SENTINEL.to_owned());
            cfg.s256 = false;
        }),
        ..Options::default()
    })
    .await;
    let id = world.add_oauth("alice").await;
    let resp = world.authorize("alice", &id).await;
    assert_eq!(
        (resp.status, resp.code().as_deref()),
        (503, Some("authorization_unavailable")),
        "{}",
        resp.text()
    );
    assert_eq!(resp.header("retry-after"), Some("5"));
    assert_eq!(world.pending.len(), 0);
    assert_eq!(
        state_of(&world, ALICE, &id).await,
        UserToolServerState::Added
    );
}

#[tokio::test]
async fn a_pending_store_that_is_down_is_a_503_and_no_link_is_offered() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    world.pending.set_down(true);
    let resp = world
        .api(
            reqwest::Method::POST,
            &format!("/api/user-tool-servers/{id}/authorize"),
            Some("alice"),
            None,
        )
        .await;
    assert_eq!(resp.status, 503, "{}", resp.text());
    assert!(resp.header("retry-after").is_some());
    assert!(!resp.text().contains("authorizeUrl"));
}

#[tokio::test]
async fn a_body_on_authorize_is_not_read() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    for body in ["{\"state\":\"x\"}", "not json at all", ""] {
        let resp = world
            .browser
            .post(format!(
                "{}/api/user-tool-servers/{id}/authorize",
                world.base
            ))
            .bearer_auth("alice")
            .header("content-type", "text/plain")
            .body(body.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200, "{body:?}");
    }
}

// ---------------------------------------------------------------------------- the rate limit

#[tokio::test]
async fn a_person_authorizes_ten_times_a_minute_and_the_eleventh_is_a_429() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    for n in 0..10 {
        let resp = world.authorize("alice", &id).await;
        assert_eq!(resp.status, 200, "call {n}: {}", resp.text());
    }
    let resp = world
        .api(
            reqwest::Method::POST,
            &format!("/api/user-tool-servers/{id}/authorize"),
            Some("alice"),
            None,
        )
        .await;
    let resp = world.checked(resp, "post", "/api/user-tool-servers/{serverId}/authorize");
    assert_eq!(resp.status, 429, "{}", resp.text());
    assert_eq!(resp.code().as_deref(), Some("rate_limited"));
    let wait: u64 = resp.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=6).contains(&wait), "{wait}");
    // the broker was asked ten times and not eleven
    assert_eq!(
        world
            .mock
            .seen_on("/.well-known/oauth-protected-resource/mcp")
            .len(),
        10
    );
}

#[tokio::test]
async fn the_limit_is_per_person_counts_every_call_and_gives_calls_back() {
    let world = World::full_with(Options {
        api: ApiConfig {
            authorize_limits: AuthorizeLimits {
                burst: 3,
                // wide on both sides of the sleep below: a loaded runner that stalls for a second
                // still sees one call given back, and not two
                refill: Duration::from_secs(2),
            },
            ..ApiConfig::default()
        },
        ..Options::default()
    })
    .await;
    let alices = world.add_oauth("alice").await;
    let bobs = world.add_oauth("bob").await;

    // calls that fail for another reason count too: three refused for a server that is not there
    for _ in 0..3 {
        assert_eq!(world.authorize("alice", "u-0000000000").await.status, 404);
    }
    assert_eq!(world.authorize("alice", &alices).await.status, 429);
    // bob has a bucket of his own
    for _ in 0..3 {
        assert_eq!(world.authorize("bob", &bobs).await.status, 200);
    }
    assert_eq!(world.authorize("bob", &bobs).await.status, 429);
    // a call is given back every 2 s
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(world.authorize("alice", &alices).await.status, 200);
    assert_eq!(world.authorize("alice", &alices).await.status, 429);
}

// -------------------------------------------------------------------------- the callback, refused

/// The page a callback that is refused for `reason` is, and the server as it is after.
async fn refused_page(world: &World, callback: &str, status: u16) -> String {
    let text = assert_a_safe_page(&world.bare_get(callback).await, status);
    for echoed in ["SENTINEL", "access_denied"] {
        assert!(!text.contains(echoed), "{echoed} is on the page: {text}");
    }
    text
}

#[tokio::test]
async fn a_state_is_used_once() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    assert_a_safe_page(&world.bare_get(&callback).await, 200);
    let text = refused_page(&world, &callback, 400).await;
    assert!(text.contains("This link is not valid"), "{text}");
    assert_eq!(
        world.mock.requests().code_exchanges,
        1,
        "the code was exchanged once"
    );
}

#[tokio::test]
async fn a_state_that_has_ended_is_an_invalid_link_and_exchanges_nothing() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    world
        .clock
        .advance(Duration::from_secs(u64::try_from(FLOW_SECONDS).unwrap()));
    let text = refused_page(&world, &callback, 400).await;
    assert!(text.contains("This link is not valid"), "{text}");
    assert_eq!(world.mock.requests().code_exchanges, 0);
    assert_eq!(
        state_of(&world, ALICE, &id).await,
        UserToolServerState::Added
    );
}

#[tokio::test]
async fn a_state_nobody_began_is_an_invalid_link() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    for forged in [
        with_param(&callback, "state", Some("0123456789abcdef0123456789abcdef")),
        with_param(&callback, "state", Some("short")),
        with_param(&callback, "state", Some("has spaces so it is not a state")),
        with_param(&callback, "state", Some(&"x".repeat(5000))),
        with_param(&callback, "state", None),
        CALLBACK_PATH.to_owned(),
        format!("{CALLBACK_PATH}?code=x"),
        format!("{CALLBACK_PATH}?"),
    ] {
        refused_page(&world, &forged, 400).await;
    }
    assert_eq!(world.mock.requests().code_exchanges, 0);
    // none of them harmed the flow that is pending
    assert_a_safe_page(&world.bare_get(&callback).await, 200);
}

#[tokio::test]
async fn a_parameter_given_twice_is_invalid_and_spends_nothing() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let state = param(&callback, "state").unwrap();
    for twice in [
        format!("{callback}&state={state}"),
        format!("{callback}&iss=https%3A%2F%2Felsewhere.example.com"),
        format!("{callback}&code=another"),
        format!("{callback}&error=access_denied&error=again"),
    ] {
        refused_page(&world, &twice, 400).await;
    }
    assert_eq!(world.pending.len(), 1, "the state is still pending");
    assert_a_safe_page(&world.bare_get(&callback).await, 200);
}

#[tokio::test]
async fn a_callback_from_another_authorization_server_is_refused_and_sends_the_code_to_nobody() {
    // the mix-up (RFC 9700, 4.4): the browser is sent to an authorization server and comes back with
    // the `iss` of another, none, or an empty one; the broker refuses each, and the code it carries
    // is exchanged nowhere
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    for iss in [Some("https://stranger.example.com"), Some(""), None] {
        let genuine = world.callback_of("alice", &id).await;
        let forged = with_param(&genuine, "iss", iss);
        let text = refused_page(&world, &forged, 400).await;
        assert!(text.contains("This link is not valid"), "{iss:?}: {text}");
        // the state of an attempt that was refused is spent, and so is the genuine redirect of it
        refused_page(&world, &genuine, 400).await;
    }
    assert_eq!(
        world.mock.requests().code_exchanges,
        0,
        "the code went nowhere"
    );
    assert_eq!(
        state_of(&world, ALICE, &id).await,
        UserToolServerState::Added
    );
    // and the person who begins again is not harmed by any of it
    assert_a_safe_page(
        &world.bare_get(&world.callback_of("alice", &id).await).await,
        200,
    );
}

#[tokio::test]
async fn an_authorization_server_that_says_no_leaves_the_server_as_it_was() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let state = param(&callback, "state").unwrap();
    // what the authorization server sends when the person declines, with a description a stranger
    // chose (it is not echoed)
    let declined = format!(
        "{CALLBACK_PATH}?error=access_denied&error_description=SENTINEL-description&state={state}&iss={}",
        url::form_urlencoded::byte_serialize(world.mock.origin().as_bytes()).collect::<String>()
    );
    let text = refused_page(&world, &declined, 400).await;
    assert!(text.contains("was not completed"), "{text}");
    assert_eq!(world.mock.requests().code_exchanges, 0);
    let listed = world.listed("alice", &id).await;
    assert_eq!(
        (listed["state"].as_str(), listed["status"].as_str()),
        (Some("added"), Some("needs_auth"))
    );
    // the state is spent: the genuine redirect of the same flow is nothing now
    refused_page(&world, &callback, 400).await;
}

#[tokio::test]
async fn a_redirect_with_no_code_is_not_completed_and_spends_the_state() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let no_code = with_param(&callback, "code", None);
    let text = refused_page(&world, &no_code, 400).await;
    assert!(text.contains("was not completed"), "{text}");
    refused_page(&world, &callback, 400).await;
    assert_eq!(world.mock.requests().code_exchanges, 0);
}

#[tokio::test]
async fn a_code_the_authorization_server_refuses_is_not_completed_and_the_server_is_unchanged() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let wrong = with_param(&callback, "code", Some("a-code-nobody-issued"));
    let text = refused_page(&world, &wrong, 400).await;
    assert!(text.contains("was not completed"), "{text}");
    assert_eq!(
        world.mock.requests().code_exchanges,
        1,
        "the authorization server was asked, and refused"
    );
    let listed = world.listed("alice", &id).await;
    assert_eq!(listed["state"], "added");
    assert_eq!(listed["hasCredential"], false);
}

#[tokio::test]
async fn a_token_endpoint_that_is_down_is_a_503_page_and_the_person_begins_again() {
    // the token endpoint the authorization server names is one nothing listens at
    let world = World::full_with(Options {
        mock: Box::new(|cfg| {
            cfg.sentinel = Some(SENTINEL.to_owned());
            cfg.token_endpoint = Some("http://127.0.0.1:1/token".to_owned());
        }),
        ..Options::default()
    })
    .await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let text = assert_a_safe_page(&world.bare_get(&callback).await, 503);
    assert!(text.contains("could not be finished"), "{text}");
    assert_eq!(
        state_of(&world, ALICE, &id).await,
        UserToolServerState::Added
    );
}

#[tokio::test]
async fn a_server_deleted_before_the_callback_is_an_invalid_link() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let resp = world
        .api(
            reqwest::Method::DELETE,
            &format!("/api/user-tool-servers/{id}"),
            Some("alice"),
            None,
        )
        .await;
    assert_eq!(resp.status, 204);
    refused_page(&world, &callback, 400).await;
    assert_eq!(world.mock.requests().code_exchanges, 0);
}

// ------------------------------------------------------------------ an edit around the callback

/// An edit as the window of an edit shows it: the definition written (as `added`, no credential) and
/// the revoke not yet run. Done on the store underneath, so nothing deletes the pending flow.
async fn written_but_not_revoked(world: &World, owner: &str, id: &str, edit: Edit<'_>) {
    let who = UserId::new(owner);
    let stored = world.servers.get(&who, &id_of(id)).await.unwrap().unwrap();
    let current = stored.server();
    let url = match edit {
        Edit::Url(url) => url.to_owned(),
        _ => current.url().to_owned(),
    };
    let auth = match edit {
        Edit::Scopes(scopes) => {
            orch_ports::UserAuth::oauth2(scopes.iter().map(|s| (*s).to_owned()).collect(), None)
                .unwrap()
        }
        Edit::Client(client) => {
            orch_ports::UserAuth::oauth2(vec!["repo".to_owned()], Some(client.to_owned())).unwrap()
        }
        _ => current.auth().clone(),
    };
    let target = world::as_support::policy().check_url(&url).unwrap();
    let edited = orch_ports::UserToolServer::new(
        current.id().clone(),
        if matches!(edit, Edit::Name) {
            "Renamed"
        } else {
            current.name()
        },
        &target,
        auth,
    )
    .unwrap();
    world
        .servers
        .replace(&who, &edited, stored.revision())
        .await
        .unwrap();
}

#[derive(Clone, Copy)]
enum Edit<'a> {
    Url(&'a str),
    Scopes(&'a [&'a str]),
    Client(&'a str),
    Name,
}

/// The review of slice 4c: an edit writes the definition before it revokes, so there is a moment when
/// the definition is the new one and the pending flow the old one's. A callback in that moment is an
/// invalid link, and the code is sent to nobody.
#[tokio::test]
async fn a_url_edited_between_authorize_and_the_callback_is_an_invalid_link_and_exchanges_nothing()
{
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let other = world.mock.other_url();
    written_but_not_revoked(&world, ALICE, &id, Edit::Url(&other)).await;

    let text = refused_page(&world, &callback, 400).await;
    assert!(text.contains("This link is not valid"), "{text}");
    assert_eq!(world.mock.requests().code_exchanges, 0, "no token exchange");
    let listed = world.listed("alice", &id).await;
    assert_eq!(listed["state"], "added", "{listed}");
    assert_eq!(listed["hasCredential"], false);
    assert!(world.pending.is_empty(), "the state was spent");
}

#[tokio::test]
async fn scopes_edited_between_authorize_and_the_callback_are_an_invalid_link_and_exchange_nothing()
{
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    written_but_not_revoked(&world, ALICE, &id, Edit::Scopes(&["repo", "admin"])).await;

    let text = refused_page(&world, &callback, 400).await;
    assert!(text.contains("This link is not valid"), "{text}");
    assert_eq!(world.mock.requests().code_exchanges, 0, "no token exchange");
    assert_eq!(
        state_of(&world, ALICE, &id).await,
        UserToolServerState::Added
    );
}

#[tokio::test]
async fn a_client_edited_between_authorize_and_the_callback_is_an_invalid_link_and_exchanges_nothing()
 {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    written_but_not_revoked(&world, ALICE, &id, Edit::Client("another-client")).await;

    refused_page(&world, &callback, 400).await;
    assert_eq!(world.mock.requests().code_exchanges, 0, "no token exchange");
}

/// The edit as the API does it, whole (write, then revoke, which deletes the pending flow): the
/// callback after it was always an invalid link; it still is.
#[tokio::test]
async fn an_edit_of_the_url_or_the_scopes_through_the_api_ends_the_flow_too() {
    for body in [
        json!({"url": "OTHER"}),
        json!({"auth": {"kind": "oauth2", "scopes": ["repo", "admin"]}}),
    ] {
        let world = World::full().await;
        let id = world.add_oauth("alice").await;
        let callback = world.callback_of("alice", &id).await;
        let body = serde_json::from_str::<serde_json::Value>(
            &body.to_string().replace("OTHER", &world.mock.other_url()),
        )
        .unwrap();
        let resp = world
            .api(
                reqwest::Method::PATCH,
                &format!("/api/user-tool-servers/{id}"),
                Some("alice"),
                Some(&body),
            )
            .await;
        assert_eq!(resp.status, 200, "{}", resp.text());
        let text = refused_page(&world, &callback, 400).await;
        assert!(text.contains("This link is not valid"), "{text}");
        assert_eq!(world.mock.requests().code_exchanges, 0);
    }
}

/// A rename is not a different definition: the flow stays and the server is connected.
#[tokio::test]
async fn a_rename_between_authorize_and_the_callback_keeps_the_flow() {
    // the rename through the API ...
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let resp = world
        .api(
            reqwest::Method::PATCH,
            &format!("/api/user-tool-servers/{id}"),
            Some("alice"),
            Some(&json!({"name": "Renamed"})),
        )
        .await;
    assert_eq!(resp.status, 200, "{}", resp.text());
    assert_a_safe_page(&world.bare_get(&callback).await, 200);
    assert_eq!(world.listed("alice", &id).await["state"], "connected");
    assert_eq!(world.mock.requests().code_exchanges, 1);

    // ... and as a write that has not revoked anything (nothing does for a rename)
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    written_but_not_revoked(&world, ALICE, &id, Edit::Name).await;
    assert_a_safe_page(&world.bare_get(&callback).await, 200);
    assert_eq!(world.listed("alice", &id).await["state"], "connected");
}

/// The race of the review, with a store that stops after the edit's `replace` as the reviewer's did:
/// the callback runs in the window, the edit's revoke runs after it. Whatever order the two end in,
/// the server must not be left `connected` with a credential issued for the other URL.
#[tokio::test]
async fn a_callback_in_the_window_of_a_url_edit_never_leaves_a_connected_server_with_the_old_url_grant()
 {
    let (world, gate) = World::full_pausing().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let body = json!({"url": world.mock.other_url()});
    let path = format!("/api/user-tool-servers/{id}");
    gate.arm();

    let edit = world.api(reqwest::Method::PATCH, &path, Some("alice"), Some(&body));
    let browser = async {
        gate.reached().await;
        // the new definition is written; the edit has not revoked yet
        let page = world.bare_get(&callback).await;
        gate.proceed();
        page
    };
    let (edited, page) = tokio::join!(edit, browser);

    assert_eq!(edited.status, 200, "{}", edited.text());
    let text = assert_a_safe_page(&page, 400);
    assert!(text.contains("This link is not valid"), "{text}");
    assert_eq!(world.mock.requests().code_exchanges, 0, "no token exchange");
    let listed = world.listed("alice", &id).await;
    assert_eq!(listed["state"], "added", "{listed}");
    assert_eq!(listed["status"], "needs_auth", "{listed}");
    assert_eq!(listed["hasCredential"], false, "{listed}");
    assert!(
        world.secrets.is_empty(),
        "no credential and no flow is kept for a server that moved"
    );
}

/// A request line as it is written, with no client in between to tidy the path (a URL parser resolves
/// `..` and the like): the status of the answer.
async fn raw_status(world: &World, method: &str, target: &str) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let addr = world.base.trim_start_matches("http://");
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request =
        format!("{method} {target} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.unwrap();
    let answer = String::from_utf8_lossy(&answer);
    answer
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or_else(|| panic!("no status in: {answer}"))
}

/// The dev edge's Caddy matcher reads more paths than the callback's as the callback (another case, a
/// doubled slash, `%2F`, `..`), drops `Authorization` and sends the request on as it came (found by the
/// review of slice 4c). The router matches exactly and its fallback is behind the identity layer, so
/// each of those is a 401 with no credential, and none of them reaches the callback: a real flow's
/// `state` sent to them is not spent.
#[tokio::test]
async fn a_near_miss_of_the_callback_path_is_the_identity_layers_401_and_spends_nothing() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let query = callback.split_once('?').unwrap().1.to_owned();
    for near in [
        "/API/user-tool-servers/oauth/callback",
        "/api/USER-tool-servers/oauth/Callback",
        "//api/user-tool-servers/oauth/callback",
        "/api//user-tool-servers/oauth/callback",
        "/api/user-tool-servers/oauth%2Fcallback",
        "/api/user-tool-servers/oauth%2fcallback",
        "/api/user-tool-servers/oauth/../oauth/callback",
        "/api/./user-tool-servers/oauth/callback",
        "/api/user-tool-servers/oauth/callback/",
        "/api/user-tool-servers/oauth/callbackx",
        "/api/user-tool-servers/oauth/callback/x",
    ] {
        let status = raw_status(&world, "GET", &format!("{near}?{query}")).await;
        assert_eq!(status, 401, "{near}");
    }
    // the identity layer answered all of them: nothing was exchanged or spent, and the real callback works
    assert_eq!(world.mock.requests().code_exchanges, 0);
    assert_eq!(world.pending.len(), 1, "the state is still there");
    assert_a_safe_page(&world.bare_get(&callback).await, 200);
    assert_eq!(world.listed("alice", &id).await["state"], "connected");
}

#[tokio::test]
async fn head_is_not_get_and_spends_nothing() {
    let world = World::full().await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let resp = world
        .browser
        .head(format!("{}{callback}", world.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 405);
    assert_eq!(resp.headers()["allow"], "GET");
    assert_eq!(
        world.pending.len(),
        1,
        "a prefetch does not spend the state"
    );
    assert_a_safe_page(&world.bare_get(&callback).await, 200);
}

#[tokio::test]
async fn other_methods_on_the_callback_are_not_the_callback() {
    let world = World::full().await;
    for method in [
        reqwest::Method::POST,
        reqwest::Method::PUT,
        reqwest::Method::DELETE,
    ] {
        let resp = world
            .browser
            .request(
                method.clone(),
                format!("{}{CALLBACK_PATH}?state=x", world.base),
            )
            .send()
            .await
            .unwrap();
        assert!(
            matches!(resp.status().as_u16(), 401 | 404 | 405),
            "{method}: {}",
            resp.status()
        );
    }
}

#[tokio::test]
async fn with_the_feature_off_the_callback_is_a_404() {
    let world = World::off().await;
    let resp = world
        .bare_get(&format!("{CALLBACK_PATH}?state=0123456789abcdef&code=x"))
        .await;
    assert_eq!(resp.status, 404);
    assert_eq!(
        resp.header("content-type"),
        Some("application/problem+json")
    );
}

// ------------------------------------------------------------------------- a callback that is dropped

#[tokio::test]
async fn a_browser_that_closes_the_connection_does_not_stop_the_authorization() {
    // the token endpoint takes a moment; the browser gives up long before it answers
    let world = World::full_with(Options {
        mock: Box::new(|cfg| {
            cfg.sentinel = Some(SENTINEL.to_owned());
            cfg.token_delay = Duration::from_millis(500);
        }),
        ..Options::default()
    })
    .await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let impatient = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    let error = impatient
        .get(format!("{}{callback}", world.base))
        .send()
        .await
        .expect_err("the browser gave up");
    assert!(error.is_timeout(), "{error}");
    // the request is gone; the work was not
    let ended = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if state_of(&world, ALICE, &id).await == UserToolServerState::Connected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(
        ended.is_ok(),
        "the authorization finished without the browser"
    );
    let listed = world.listed("alice", &id).await;
    assert_eq!(
        (listed["status"].as_str(), listed["hasCredential"].as_bool()),
        (Some("ready"), Some(true))
    );
}

// ------------------------------------------------------------------------- the rate limit, public

#[tokio::test]
async fn the_callback_is_rate_limited_with_the_other_public_routes() {
    let world = World::full_with(Options {
        api: ApiConfig {
            public_limits: Some(PublicLimits {
                per_link_per_second: 100,
                total_per_second: 3,
                streams_per_link: 5,
                streams_total: 50,
            }),
            ..ApiConfig::default()
        },
        ..Options::default()
    })
    .await;
    let mut statuses = Vec::new();
    for _ in 0..6 {
        let resp = world
            .bare_get(&format!("{CALLBACK_PATH}?state=0123456789abcdef"))
            .await;
        statuses.push(resp.status);
        if resp.status == 429 {
            assert!(resp.header("retry-after").is_some());
            assert_eq!(resp.header("cache-control"), Some("no-store"));
        }
    }
    assert_eq!(&statuses[..3], &[400, 400, 400], "{statuses:?}");
    assert!(statuses.contains(&429), "{statuses:?}");
}

/// The page, not a problem: the browser that is limited is a person's, and it is shown a page with the
/// same hardening as every other answer of the callback (and `Retry-After`). The link has not been
/// spent: a limited request never reached the handler.
#[tokio::test]
async fn a_limited_callback_is_the_fixed_page_with_the_hardening_and_retry_after() {
    let world = World::full_with(Options {
        api: ApiConfig {
            public_limits: Some(PublicLimits {
                per_link_per_second: 1,
                total_per_second: 100,
                streams_per_link: 5,
                streams_total: 50,
            }),
            ..ApiConfig::default()
        },
        ..Options::default()
    })
    .await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    // a request of a stranger takes the one token of the callback's bucket ...
    assert_eq!(
        world
            .bare_get(&format!("{CALLBACK_PATH}?state=x"))
            .await
            .status,
        400
    );
    // ... and the person's own request, in the same second, is limited
    let resp = world.bare_get(&callback).await;
    let text = assert_a_safe_page(&resp, 429);
    assert!(text.contains("Too many requests"), "{text}");
    let wait: u64 = resp.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=2).contains(&wait), "{wait}");
    for echoed in ["SENTINEL", "state", "code"] {
        assert!(!text.contains(echoed), "{echoed} is on the page: {text}");
    }
    // nothing was spent: the same link, a moment later, connects the server
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_a_safe_page(&world.bare_get(&callback).await, 200);
    assert_eq!(world.mock.requests().code_exchanges, 1);
}

/// One anonymous client must not be able to push every callback of everybody, and every fetch of the
/// document by an authorization server, into 429 with one flood: the two routes have a key each.
#[tokio::test]
async fn flooding_the_callback_leaves_the_document_served() {
    let (world, callback_path) = limited_world(1).await;
    let mut limited = false;
    for _ in 0..5 {
        limited |= world.bare_get(&callback_path).await.status == 429;
    }
    assert!(limited, "the callback's bucket is empty");
    let resp = world.bare_get("/oauth/client-metadata.json").await;
    assert_eq!(resp.status, 200, "{}", resp.text());
}

#[tokio::test]
async fn flooding_the_document_leaves_the_callback_served() {
    let (world, callback_path) = limited_world(1).await;
    let mut limited = false;
    for _ in 0..5 {
        limited |= world.bare_get("/oauth/client-metadata.json").await.status == 429;
    }
    assert!(limited, "the document's bucket is empty");
    let resp = world.bare_get(&callback_path).await;
    assert_eq!(resp.status, 400, "answered, not limited: {}", resp.text());
}

/// A world with a document and a limit of `per_link` requests a second for each public key.
async fn limited_world(per_link: u32) -> (World, String) {
    let world = World::full_with(Options {
        api: ApiConfig {
            public_limits: Some(PublicLimits {
                per_link_per_second: per_link,
                total_per_second: 1000,
                streams_per_link: 5,
                streams_total: 50,
            }),
            client_metadata: Some(
                ClientMetadata::new(
                    "https://chat.example.com/oauth/client-metadata.json",
                    "Acme Chat",
                    "https://chat.example.com/api/user-tool-servers/oauth/callback",
                )
                .unwrap(),
            ),
            ..ApiConfig::default()
        },
        ..Options::default()
    })
    .await;
    let callback = format!("{CALLBACK_PATH}?state=0123456789abcdef");
    (world, callback)
}

/// The timeout layer answers a bare 503 when a callback takes longer than the API's limit; on this
/// route it is the page of "could not be finished", with the hardening of every other answer (the
/// work itself runs on, as `a_browser_that_closes_the_connection_does_not_stop_the_authorization`
/// shows).
#[tokio::test]
async fn a_callback_the_timeout_ends_is_the_fixed_503_page_with_the_hardening() {
    let world = World::full_with(Options {
        api: ApiConfig {
            request_timeout: Duration::from_millis(250),
            ..ApiConfig::default()
        },
        mock: Box::new(|cfg| {
            cfg.sentinel = Some(SENTINEL.to_owned());
            cfg.token_delay = Duration::from_millis(1200);
        }),
        ..Options::default()
    })
    .await;
    let id = world.add_oauth("alice").await;
    let callback = world.callback_of("alice", &id).await;
    let resp = world.bare_get(&callback).await;
    let text = assert_a_safe_page(&resp, 503);
    assert!(text.contains("could not be finished"), "{text}");
}

// ----------------------------------------------------------------------------- the document

fn document_world(url: &str, name: &str, callback: &str) -> Options {
    Options {
        api: ApiConfig {
            client_metadata: Some(ClientMetadata::new(url, name, callback).unwrap()),
            ..ApiConfig::default()
        },
        ..Options::default()
    }
}

#[tokio::test]
async fn the_client_metadata_document_is_served_to_anybody_at_its_path() {
    let world = World::full_with(document_world(
        "https://chat.example.com/oauth/client-metadata.json",
        "Acme Chat",
        "https://chat.example.com/api/user-tool-servers/oauth/callback",
    ))
    .await;
    // no credential at all: the fetcher is an authorization server
    let resp = world.bare_get("/oauth/client-metadata.json").await;
    let resp = world.checked(resp, "get", "/oauth/client-metadata.json");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.header("content-type"), Some("application/json"));
    assert_eq!(resp.header("cache-control"), Some("public, max-age=300"));
    assert_eq!(
        resp.json(),
        json!({
            "client_id": "https://chat.example.com/oauth/client-metadata.json",
            "client_name": "Acme Chat",
            "redirect_uris": ["https://chat.example.com/api/user-tool-servers/oauth/callback"],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        })
    );
    // any credential is ignored, a query too
    let with_credential = world
        .api(
            reqwest::Method::GET,
            "/oauth/client-metadata.json?v=1",
            Some("carol"),
            None,
        )
        .await;
    assert_eq!(with_credential.status, 200);
    // and nothing else is served there: another path is the identity layer's 401
    assert_eq!(world.bare_get("/oauth/other.json").await.status, 401);
    assert_eq!(
        world.bare_get("/oauth/client-metadata.json/x").await.status,
        401
    );
}

#[tokio::test]
async fn without_a_document_configured_its_path_is_not_served() {
    let world = World::full().await;
    assert_eq!(
        world.bare_get("/oauth/client-metadata.json").await.status,
        401
    );
}

#[tokio::test]
async fn a_path_inside_the_api_cannot_be_a_document() {
    // the router is never built over a path the API serves: the document refuses to be made
    for path in ["/api/public/oauth/c.json", "/mcp", "/healthz", "/agui/x"] {
        assert!(
            matches!(
                ClientMetadata::new(
                    &format!("https://chat.example.com{path}"),
                    "n",
                    "https://chat.example.com/api/user-tool-servers/oauth/callback"
                ),
                Err(orch_api::ClientMetadataError::Reserved(_))
            ),
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_document_cannot_list_a_redirect_the_router_does_not_serve() {
    // the redirect the document lists is where the callback is, or no document is made
    for redirect in [
        "https://chat.example.com/cb",
        "https://chat.example.com/api/user-tool-servers/oauth/callback/",
        "https://chat.example.com/",
    ] {
        assert_eq!(
            ClientMetadata::new("https://chat.example.com/oauth/c.json", "n", redirect),
            Err(orch_api::ClientMetadataError::RedirectPath),
            "{redirect}"
        );
    }
}
