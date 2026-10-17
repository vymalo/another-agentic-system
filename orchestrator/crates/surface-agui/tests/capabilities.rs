//! `GET /agui/agents/{agentId}/capabilities`: the `AgentCapabilities` document, read live.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::RELEASE_CHANNELS_URI;
use orch_agui_proto::testkit::assert_capabilities_json_conforms;
use orch_core::KnownExtension;
use serde_json::json;
use support::*;

#[tokio::test]
async fn the_document_conforms_and_describes_the_agent() {
    let h = Harness::start().await;
    let resp = h.get("/agui/agents/plain/capabilities", Some(ALICE)).await;
    assert_eq!(resp.status, 200);
    assert!(resp.content_type.starts_with("application/json"));
    assert_eq!(resp.headers["cache-control"], "no-store");
    let doc = resp.json();
    assert_capabilities_json_conforms(&doc);
    assert_eq!(doc["identity"]["name"], "Plain");
    assert_eq!(doc["identity"]["description"], "scripted agent");
    assert_eq!(doc["identity"]["version"], "1.0.0");
    assert_eq!(
        doc["transport"],
        json!({"streaming": true, "resumable": true})
    );
    assert_eq!(
        doc["humanInTheLoop"],
        json!({"supported": true, "interrupts": true})
    );
    assert_eq!(doc["multiAgent"]["subagents"][0]["name"], "plain");
    assert!(
        doc.get("custom").is_none(),
        "a card without releases declares none: {doc}"
    );
}

#[tokio::test]
async fn release_channels_are_declared_only_while_the_card_lists_them() {
    let h = Harness::start().await;
    let get = || async {
        let doc = h.get("/agui/agents/coder/capabilities", Some(ALICE)).await;
        assert_eq!(doc.status, 200);
        let doc = doc.json();
        assert_capabilities_json_conforms(&doc);
        doc
    };
    let doc = get().await;
    let releases = &doc["custom"][RELEASE_CHANNELS_URI];
    assert!(releases["defaultChannel"].is_string(), "{doc}");
    assert!(releases["channels"].is_object(), "{doc}");

    // Read live, never cached, fail closed: with the card unreadable the document is smaller,
    // and it is back when the card is.
    h.agent.set_card_down("coder", true);
    let down = get().await;
    assert!(down.get("custom").is_none(), "{down}");
    assert!(down["identity"].get("description").is_none(), "{down}");
    assert_eq!(down["identity"]["name"], "Coder");
    assert_eq!(down["transport"]["resumable"], true);
    h.agent.set_card_down("coder", false);
    assert_eq!(get().await, doc);
}

#[tokio::test]
async fn our_extensions_are_declared_only_while_the_card_lists_them() {
    let h = Harness::start().await;
    let get = || async {
        let doc = h.get("/agui/agents/plain/capabilities", Some(ALICE)).await;
        assert_eq!(doc.status, 200);
        let doc = doc.json();
        assert_capabilities_json_conforms(&doc);
        doc
    };
    assert!(get().await.get("custom").is_none());

    // the card lists two: each is a key of `custom`, with nothing more to say
    h.agent.set_extensions(
        "plain",
        &[KnownExtension::UiCatalog, KnownExtension::ThreadTools],
    );
    let doc = get().await;
    let custom = doc["custom"].as_object().unwrap();
    assert_eq!(custom.len(), 2, "{doc}");
    assert_eq!(custom[KnownExtension::UiCatalog.uri()], json!({}));
    assert_eq!(custom[KnownExtension::ThreadTools.uri()], json!({}));

    // read live: the card without them, the next document without them
    h.agent.set_extensions("plain", &[]);
    assert!(get().await.get("custom").is_none());
    // fail closed: an unreadable card declares none
    h.agent.set_extensions("plain", &[KnownExtension::Steps]);
    assert!(
        get().await["custom"]
            .get(KnownExtension::Steps.uri())
            .is_some()
    );
    h.agent.set_card_down("plain", true);
    assert!(get().await.get("custom").is_none());
}

#[tokio::test]
async fn an_unknown_agent_is_a_404_and_no_identity_is_a_401() {
    let h = Harness::start().await;
    h.get("/agui/agents/nobody/capabilities", Some(ALICE))
        .await
        .problem(404);
    h.get("/agui/agents/plain/capabilities", None)
        .await
        .problem(401);
}

#[tokio::test]
async fn the_access_tier_is_declared_with_its_tiers_and_default_only_while_the_card_lists_it() {
    // the web draws its tier chip from this key (ADR 0061); nothing is cached
    let h = Harness::start().await;
    let get = || async {
        let doc = h.get("/agui/agents/plain/capabilities", Some(ALICE)).await;
        assert_eq!(doc.status, 200);
        let doc = doc.json();
        assert_capabilities_json_conforms(&doc);
        doc
    };
    let uri = orch_core::ACCESS_TIER_EXTENSION;
    assert!(get().await.get("custom").is_none());

    h.agent.set_access_tier(
        "plain",
        orch_core::TierOffer::from_params(
            &json!({"tiers": ["bypass", "plan", "edits"], "default": "edits"}),
        ),
    );
    let doc = get().await;
    assert_eq!(
        doc["custom"][uri],
        json!({"tiers": ["plan", "edits", "bypass"], "default": "edits"}),
        "{doc}"
    );

    // the card changes between two reads: the document follows
    h.agent.set_access_tier("plain", None);
    assert!(get().await.get("custom").is_none());
    // and a card that cannot be read declares nothing
    h.agent.set_access_tier(
        "plain",
        orch_core::TierOffer::from_params(&json!({"tiers": ["plan"], "default": "plan"})),
    );
    h.agent.set_card_down("plain", true);
    assert!(get().await.get("custom").is_none());
}

#[tokio::test]
async fn the_document_says_what_the_person_asking_may_use_of_this_agent() {
    // the web draws its chip from this document, so the cap goes in it per agent: the highest
    // cap of the roles that let the person use *this* agent (`App::max_tier_for`; the per-agent
    // arithmetic is pinned in the application's tests), never a hint over all their roles
    use std::collections::BTreeSet;

    use orch_app::{AgentScope, Permission, Policy, RoleGrant};
    use orch_core::AccessTier;
    use orch_ports::Role;

    let uri = orch_core::ACCESS_TIER_EXTENSION;
    for (cap, expected) in [
        (
            Some(AccessTier::Plan),
            json!({"tiers": ["plan", "edits", "auto"], "default": "plan", "maxTier": "plan"}),
        ),
        (
            None,
            json!({"tiers": ["plan", "edits", "auto"], "default": "plan"}),
        ),
    ] {
        let grant = RoleGrant {
            permissions: BTreeSet::from([
                Permission::AgentRead,
                Permission::AgentInvoke,
                Permission::ThreadRead,
                Permission::ThreadWrite,
            ]),
            agents: AgentScope::All,
            max_tier: cap,
        };
        let policy = Policy::new(
            [(Role::new("careful"), grant)].into(),
            Some(Role::new("careful")),
        )
        .unwrap();
        let h = Harness::start_with_policy(policy).await;
        h.agent.set_access_tier(
            "plain",
            orch_core::TierOffer::from_params(
                &json!({"tiers": ["plan", "edits", "auto"], "default": "plan"}),
            ),
        );
        let doc = h.get("/agui/agents/plain/capabilities", Some(ALICE)).await;
        assert_eq!(doc.status, 200);
        let doc = doc.json();
        assert_capabilities_json_conforms(&doc);
        assert_eq!(doc["custom"][uri], expected, "cap {cap:?}: {doc}");
    }
}
