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
                refill: Duration::from_millis(300),
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
    // a call is given back every 300 ms
    tokio::time::sleep(Duration::from_millis(450)).await;
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
    assert_eq!(resp.header("cache-control"), Some("public, max-age=3600"));
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
            ClientMetadata::new(
                &format!("https://chat.example.com{path}"),
                "n",
                "https://chat.example.com/cb"
            )
            .is_err(),
            "{path}"
        );
    }
}
