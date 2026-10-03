//! Sharing a thread by a revocable link through the application (ADR 0040): what the owner may do
//! under the deployment's cap, what somebody who has the link may read and what they are told, the
//! one answer for every link that does not work, and a stream that ends when the link does.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt as _;
use futures::stream::BoxStream;
use orch_app::{
    AgentScope, App, AppConfig, AppError, FeedItem, NewThread, Permission, Policy, PublicView,
    RoleGrant, ShareKeys, SharingMode, SharingSettings,
};
use orch_core::{
    Actor, ActorType, AgentStepData, ArtifactData, Event, EventBody, EventKind, FileRef,
    ShareLevel, StepKind, StepOutput, StepPhase, StepState, ThreadId, ThreadRecord, UserId,
    Visibility,
};
use orch_ports::memory::{
    MemoryArtifacts, MemoryStore, MemoryWakeup, ScriptedAgent, ScriptedModel, SeqIds,
};
use orch_ports::{
    ArtifactKey, ArtifactMeta, ArtifactStore, Commit, FixedRegistry, NewEvent, PortSet, Principal,
    Role, SharingChange, SystemClock, ThreadStore,
};
use secrecy::SecretString;
use serde_json::Value;
use support::*;

const SECRET: &str = "0123456789abcdef0123456789abcdef";
const PREVIOUS: &str = "fedcba9876543210fedcba9876543210";

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

fn user(email: &str) -> Principal {
    principal(email, &["user"])
}

fn keys(secret: &str, previous: Option<&str>) -> ShareKeys {
    ShareKeys::new(
        SecretString::from(secret.to_owned()),
        previous.map(|p| SecretString::from(p.to_owned())),
    )
    .unwrap()
}

fn settings(mode: SharingMode, public: PublicView) -> SharingSettings {
    SharingSettings::new(mode, Some(keys(SECRET, None)), public).unwrap()
}

fn app_with_policy(w: &World, sharing: SharingSettings, policy: Policy) -> Arc<TestApp> {
    w.app_with(AppConfig {
        sharing,
        policy,
        stream_poll: Duration::from_millis(50),
        ..AppConfig::default()
    })
}

fn app_in(w: &World, mode: SharingMode) -> Arc<TestApp> {
    app_with_policy(w, settings(mode, PublicView::default()), Policy::default())
}

fn new_thread(agent: &str, text: &str) -> NewThread {
    NewThread {
        title: None,
        target: target(agent),
        text: text.to_owned(),
    }
}

async fn started(app: &TestApp, who: &Principal) -> ThreadRecord {
    app.create_thread(who, new_thread("plain", "hello"))
        .await
        .unwrap()
}

/// The token a thread's link carries.
fn token_of<P: orch_ports::Ports>(app: &App<P>, thread: &ThreadRecord) -> String {
    let url = app.share_view(thread).unwrap().url.unwrap();
    url.strip_prefix("/s/").unwrap().to_owned()
}

async fn log_of(app: &TestApp, who: &Principal, id: ThreadId) -> Vec<Event> {
    app.list_events(who, id, 0, 500).await.unwrap()
}

async fn reread(w: &World, id: ThreadId) -> ThreadRecord {
    w.store.get_thread(None, id).await.unwrap().unwrap()
}

fn is_not_found<T>(result: Result<T, AppError>) {
    match result {
        Err(AppError::NotFound) => {}
        Err(other) => panic!("expected NotFound, got {other:?}"),
        Ok(_) => panic!("expected NotFound, got a success"),
    }
}

fn forbidden<T>(result: Result<T, AppError>) -> Permission {
    match result {
        Err(AppError::Forbidden { permission, .. }) => permission,
        Err(other) => panic!("expected Forbidden, got {other:?}"),
        Ok(_) => panic!("expected Forbidden, got a success"),
    }
}

/// Appends events to a thread straight in the store: what an agent or a person would have logged.
async fn append(w: &World, id: ThreadId, events: Vec<(Actor, EventBody)>) {
    let record = reread(w, id).await;
    w.store
        .commit(
            id,
            record.version,
            Commit {
                new_state: record.state,
                job: None,
                events: events
                    .into_iter()
                    .map(|(actor, body)| NewEvent {
                        at: jiff::Timestamp::now(),
                        actor,
                        body,
                        idempotency_key: None,
                    })
                    .collect(),
                outbox: vec![],
                binding: None,
                now: jiff::Timestamp::now(),
                lease: None,
                watches: vec![],
                timers: vec![],
                inbox: None,
                finishes_outbox: None,
                title: None,
                description: None,
                sharing: None,
                skip_unsent_delegates: false,
            },
        )
        .await
        .unwrap();
}

fn coder() -> Actor {
    Actor::agent(&orch_core::AgentId::new("plain"), None)
}

fn a_step() -> AgentStepData {
    let mut input = serde_json::Map::new();
    input.insert("command".into(), Value::from("cat notes.txt"));
    AgentStepData {
        id: "T/1".into(),
        path: vec![],
        kind: StepKind::Tool,
        label: "Read the notes".into(),
        state: StepState::Completed,
        phase: StepPhase::End,
        icon: None,
        detail: Some("ok".into()),
        input: Some(input),
        output: Some(StepOutput {
            text: "the notes".into(),
            truncated: false,
            bytes: None,
            error: false,
        }),
        io_dropped: false,
    }
}

/// The events of a feed up to the thread's last, each as it was said.
async fn read_feed(mut feed: BoxStream<'static, FeedItem>, up_to: i64) -> Vec<Event> {
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(item) = feed.next().await {
            if let FeedItem::Event(event) = item {
                let seq = event.seq;
                seen.push(event);
                if seq >= up_to {
                    break;
                }
            }
        }
    })
    .await
    .expect("the feed to reach the end of the log");
    seen
}

// ---- the owner ----------------------------------------------------------------------------

#[tokio::test]
async fn the_owner_shares_and_the_link_is_recomputed_from_the_row() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let alice = user("alice@example.com");
    let t = started(&app, &alice).await;
    assert!(app.share_view(&t).is_none(), "a thread starts private");

    let view = app
        .share_thread(&alice, t.id, ShareLevel::Internal)
        .await
        .unwrap();
    assert_eq!(view.visibility, ShareLevel::Internal);
    assert_eq!(view.effective, Visibility::Internal);
    let row = reread(&w, t.id).await;
    let share = row.share.unwrap();
    // the row has the nonce, and the link is made from it under the secret
    let url = view.url.clone().unwrap();
    assert_eq!(
        url,
        format!("/s/{}", keys(SECRET, None).token(t.id, &share.nonce))
    );
    assert_eq!(
        app.share_view(&row).unwrap(),
        view,
        "the owner can copy it again"
    );

    // the log has the digest of the nonce, never the nonce
    let log = log_of(&app, &alice, t.id).await;
    let shared = log
        .iter()
        .find_map(|e| match &e.body {
            EventBody::ThreadShared(d) => Some((e, d)),
            _ => None,
        })
        .unwrap();
    assert_eq!(shared.1.visibility, ShareLevel::Internal);
    assert_eq!(shared.1.nonce_sha256, share.nonce.sha256_hex());
    assert_eq!(shared.0.actor, Actor::user(&alice.user));
    let text = serde_json::to_string(&log).unwrap();
    assert!(!text.contains(&url) && !text.contains(url.trim_start_matches("/s/")));

    // idempotent: the same level is the current link and writes nothing
    let again = app
        .share_thread(&alice, t.id, ShareLevel::Internal)
        .await
        .unwrap();
    assert_eq!(again, view);
    assert_eq!(log_of(&app, &alice, t.id).await.len(), log.len());
}

#[tokio::test]
async fn widening_and_narrowing_keep_the_link_and_a_new_link_replaces_it() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let alice = user("alice@example.com");
    let t = started(&app, &alice).await;
    let first = app
        .share_thread(&alice, t.id, ShareLevel::Internal)
        .await
        .unwrap();
    let widened = app
        .share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    assert_eq!(widened.effective, Visibility::Public);
    assert_eq!(widened.url, first.url, "same nonce, same link");
    let narrowed = app
        .share_thread(&alice, t.id, ShareLevel::Internal)
        .await
        .unwrap();
    assert_eq!(narrowed.url, first.url);

    let token = token_of(&app, &reread(&w, t.id).await);
    let rotated = app.rotate_share(&alice, t.id).await.unwrap();
    assert_ne!(rotated.url, first.url, "a new link");
    assert_eq!(
        rotated.visibility,
        ShareLevel::Internal,
        "at the level it had"
    );
    // the old link is dead from that moment; the new one reads
    let bob = user("bob@example.com");
    is_not_found(app.open_shared(&bob, &token).await);
    let new_token = token_of(&app, &reread(&w, t.id).await);
    assert!(app.open_shared(&bob, &new_token).await.is_ok());

    // one `thread_shared` per change: share, widen, narrow, rotate
    let log = log_of(&app, &alice, t.id).await;
    let shares: Vec<_> = log
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::ThreadShared(d) => Some((d.visibility, d.nonce_sha256.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(shares.len(), 4);
    assert_eq!(shares[0].1, shares[1].1);
    assert_eq!(shares[1].1, shares[2].1);
    assert_ne!(
        shares[2].1, shares[3].1,
        "a rotation is told apart by its digest"
    );
    let stats = app.sharing_stats();
    let counted: Vec<(&str, u64)> = stats
        .changes
        .iter()
        .map(|(a, n)| (a.as_str(), *n))
        .collect();
    assert_eq!(
        counted,
        [
            ("share", 1),
            ("widen", 1),
            ("narrow", 1),
            ("rotate", 1),
            ("revoke", 0)
        ]
    );
}

#[tokio::test]
async fn a_private_thread_has_no_link_to_make_a_new_one_of() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let alice = user("alice@example.com");
    let t = started(&app, &alice).await;
    assert!(matches!(
        app.rotate_share(&alice, t.id).await,
        Err(AppError::NotShared)
    ));
    // taking down what is not up is not an error, and writes nothing
    app.unshare_thread(&alice, t.id).await.unwrap();
    assert!(
        !log_of(&app, &alice, t.id)
            .await
            .iter()
            .any(|e| e.kind() == EventKind::ThreadUnshared)
    );
}

#[tokio::test]
async fn revoking_clears_the_nonce_and_sharing_again_makes_a_new_link() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let (alice, bob) = (user("alice@example.com"), user("bob@example.com"));
    let t = started(&app, &alice).await;
    app.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let old = token_of(&app, &reread(&w, t.id).await);
    assert!(app.open_public(&old).await.is_ok());

    app.unshare_thread(&alice, t.id).await.unwrap();
    let row = reread(&w, t.id).await;
    assert!(row.share.is_none() && row.visibility() == Visibility::Private);
    is_not_found(app.open_public(&old).await);
    is_not_found(app.open_shared(&bob, &old).await);
    assert!(
        log_of(&app, &alice, t.id)
            .await
            .iter()
            .any(|e| matches!(e.body, EventBody::ThreadUnshared(_)))
    );

    // a revoked link never comes back
    app.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let fresh = token_of(&app, &reread(&w, t.id).await);
    assert_ne!(fresh, old);
    is_not_found(app.open_public(&old).await);
    assert!(app.open_public(&fresh).await.is_ok());
}

/// The commit of a revocation, as another request of the owner makes it: the `thread_unshared`
/// event and the row cleared together.
fn a_revocation(state: orch_core::ThreadState) -> Commit {
    Commit {
        new_state: state,
        job: None,
        events: vec![NewEvent {
            at: jiff::Timestamp::now(),
            actor: Actor::user(&alice()),
            body: EventBody::ThreadUnshared(orch_core::ThreadUnsharedData {}),
            idempotency_key: None,
        }],
        outbox: vec![],
        binding: None,
        now: jiff::Timestamp::now(),
        lease: None,
        watches: vec![],
        timers: vec![],
        inbox: None,
        finishes_outbox: None,
        title: None,
        description: None,
        sharing: Some(SharingChange::Clear),
        skip_unsent_delegates: false,
    }
}

#[tokio::test]
async fn a_change_of_level_raced_by_a_revocation_never_brings_the_revoked_link_back() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let (alice, bob) = (user("alice@example.com"), user("bob@example.com"));
    let t = started(&app, &alice).await;
    app.share_thread(&alice, t.id, ShareLevel::Internal)
        .await
        .unwrap();
    let revoked = token_of(&app, &reread(&w, t.id).await);

    // the owner widens, having read the thread shared; their revocation lands before the write
    w.store
        .interleave_next_commit(t.id, a_revocation(reread(&w, t.id).await.state));
    let widened = app
        .share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();

    // the widening was decided again on a private thread: a first share, with a new link
    let fresh = token_of(&app, &reread(&w, t.id).await);
    assert_ne!(fresh, revoked, "the revoked nonce is never written back");
    assert_eq!(widened.url.as_deref(), Some(format!("/s/{fresh}").as_str()));
    is_not_found(app.open_public(&revoked).await);
    is_not_found(app.open_shared(&bob, &revoked).await);
    assert!(app.open_public(&fresh).await.is_ok());
    let kinds: Vec<_> = log_of(&app, &alice, t.id)
        .await
        .iter()
        .map(Event::kind)
        .filter(|k| matches!(k, EventKind::ThreadShared | EventKind::ThreadUnshared))
        .collect();
    assert_eq!(
        kinds,
        [
            EventKind::ThreadShared,
            EventKind::ThreadUnshared,
            EventKind::ThreadShared
        ]
    );
    let stats = app.sharing_stats();
    let counted: Vec<(&str, u64)> = stats
        .changes
        .iter()
        .map(|(a, n)| (a.as_str(), *n))
        .collect();
    assert_eq!(
        counted,
        [
            ("share", 2),
            ("widen", 0),
            ("narrow", 0),
            ("rotate", 0),
            ("revoke", 0)
        ],
        "counted as what was written: the revocation was not this process's"
    );
}

#[tokio::test]
async fn a_new_link_raced_by_a_revocation_leaves_the_thread_private() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let alice = user("alice@example.com");
    let t = started(&app, &alice).await;
    app.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    w.store
        .interleave_next_commit(t.id, a_revocation(reread(&w, t.id).await.state));
    assert!(matches!(
        app.rotate_share(&alice, t.id).await,
        Err(AppError::NotShared)
    ));
    let row = reread(&w, t.id).await;
    assert!(row.share.is_none(), "the revocation stands");
}

// ---- who may do what ------------------------------------------------------------------------

#[tokio::test]
async fn sharing_is_the_owners_and_needs_thread_share_and_nobody_is_an_exception() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let alice = user("alice@example.com");
    let (bob, root) = (
        user("bob@example.com"),
        principal("root@example.com", &["admin"]),
    );
    let t = started(&app, &alice).await;
    let nobody = ThreadId(uuid::Uuid::from_u128(0x1234));
    for (who, what) in [(&bob, "another user"), (&root, "an administrator")] {
        for id in [t.id, nobody] {
            is_not_found(app.share_thread(who, id, ShareLevel::Internal).await);
            is_not_found(app.rotate_share(who, id).await);
            is_not_found(app.unshare_thread(who, id).await);
        }
        assert!(
            reread(&w, t.id).await.share.is_none(),
            "{what} shared nothing"
        );
    }

    // a role that lacks thread.share is refused for every id alike, its own thread included
    let no_share = Policy::new(
        BTreeMap::from([(
            Role::new("user"),
            RoleGrant {
                permissions: RoleGrant::user()
                    .permissions
                    .into_iter()
                    .filter(|p| *p != Permission::ThreadShare)
                    .collect::<BTreeSet<_>>(),
                agents: AgentScope::All,
            },
        )]),
        Some(Role::new("user")),
    )
    .unwrap();
    let strict = app_with_policy(
        &w,
        settings(SharingMode::Public, PublicView::default()),
        no_share,
    );
    for id in [t.id, nobody] {
        assert_eq!(
            forbidden(strict.share_thread(&alice, id, ShareLevel::Internal).await),
            Permission::ThreadShare
        );
        assert_eq!(
            forbidden(strict.rotate_share(&alice, id).await),
            Permission::ThreadShare
        );
    }
    // `GET /api/me` says nothing is on offer to such a person
    assert_eq!(strict.sharing_mode_for(&alice), SharingMode::Disabled);
    assert_eq!(app.sharing_mode_for(&alice), SharingMode::Public);
}

#[tokio::test]
async fn the_owner_can_always_take_a_link_down() {
    let w = World::new();
    let alice = user("alice@example.com");
    let on = app_in(&w, SharingMode::Public);
    let t = started(&on, &alice).await;
    on.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let token = token_of(&on, &reread(&w, t.id).await);

    // the deployment disabled sharing and the role lost `thread.share`: the owner can still stop
    let no_share = Policy::new(
        BTreeMap::from([(
            Role::new("user"),
            RoleGrant {
                permissions: BTreeSet::from([Permission::ThreadRead, Permission::AgentRead]),
                agents: AgentScope::All,
            },
        )]),
        Some(Role::new("user")),
    )
    .unwrap();
    let off = app_with_policy(&w, SharingSettings::default(), no_share);
    assert!(matches!(
        off.share_thread(&alice, t.id, ShareLevel::Internal).await,
        Err(AppError::Forbidden { .. })
    ));
    off.unshare_thread(&alice, t.id).await.unwrap();
    assert!(reread(&w, t.id).await.share.is_none());
    is_not_found(on.open_public(&token).await);
}

#[tokio::test]
async fn the_cap_limits_what_can_be_shared() {
    let w = World::new();
    let alice = user("alice@example.com");
    let disabled = app_in(&w, SharingMode::Disabled);
    let t = started(&disabled, &alice).await;
    for level in [ShareLevel::Internal, ShareLevel::Public] {
        assert!(matches!(
            disabled.share_thread(&alice, t.id, level).await,
            Err(AppError::SharingDisabled)
        ));
    }
    assert!(matches!(
        disabled.rotate_share(&alice, t.id).await,
        Err(AppError::SharingDisabled)
    ));

    let internal = app_in(&w, SharingMode::Internal);
    assert!(matches!(
        internal
            .share_thread(&alice, t.id, ShareLevel::Public)
            .await,
        Err(AppError::OverCap {
            cap: SharingMode::Internal
        })
    ));
    assert!(
        reread(&w, t.id).await.share.is_none(),
        "an over-cap share writes nothing"
    );
    internal
        .share_thread(&alice, t.id, ShareLevel::Internal)
        .await
        .unwrap();

    let public = app_in(&w, SharingMode::Public);
    public
        .share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
}

#[tokio::test]
async fn lowering_the_cap_narrows_every_link_at_once_and_raising_it_brings_them_back() {
    let w = World::new();
    let alice = user("alice@example.com");
    let bob = user("bob@example.com");
    let public = app_in(&w, SharingMode::Public);
    let t = started(&public, &alice).await;
    public
        .share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let token = token_of(&public, &reread(&w, t.id).await);
    assert!(public.open_public(&token).await.is_ok());

    // a restart under `internal`: the public link is a 404, the signed-in one reads, as internal
    let internal = app_in(&w, SharingMode::Internal);
    is_not_found(internal.open_public(&token).await);
    let read = internal.open_shared(&bob, &token).await.unwrap();
    assert_eq!(read.effective(), Visibility::Internal);
    // the owner sees what the thread is stored as, and what is served
    let view = internal.share_view(&reread(&w, t.id).await).unwrap();
    assert_eq!(
        (view.visibility, view.effective),
        (ShareLevel::Public, Visibility::Internal)
    );

    // under `disabled` the links answer 404 for everybody, and nothing was rewritten
    let disabled = app_in(&w, SharingMode::Disabled);
    is_not_found(disabled.open_shared(&bob, &token).await);
    is_not_found(disabled.open_public(&token).await);
    let paused = disabled.share_view(&reread(&w, t.id).await).unwrap();
    assert_eq!(
        paused.effective,
        Visibility::Private,
        "paused by this deployment"
    );
    assert_eq!(paused.url, None, "no secret, no link to show");
    assert_eq!(reread(&w, t.id).await.visibility(), Visibility::Public);

    // raising the cap again brings the same link back
    let again = app_in(&w, SharingMode::Public);
    assert!(again.open_public(&token).await.is_ok());
}

// ---- the token ------------------------------------------------------------------------------

#[tokio::test]
async fn a_link_that_does_not_work_is_one_answer() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let (alice, bob) = (user("alice@example.com"), user("bob@example.com"));
    let shared = started(&app, &alice).await;
    app.share_thread(&alice, shared.id, ShareLevel::Public)
        .await
        .unwrap();
    let other = started(&app, &alice).await;
    app.share_thread(&alice, other.id, ShareLevel::Public)
        .await
        .unwrap();
    let token = token_of(&app, &reread(&w, shared.id).await);
    let other_token = token_of(&app, &reread(&w, other.id).await);
    let private = started(&app, &alice).await;

    // the nonce of one thread with the MAC of another
    let (a, b) = (
        reread(&w, shared.id).await.share.unwrap().nonce,
        reread(&w, other.id).await.share.unwrap().nonce,
    );
    let k = keys(SECRET, None);
    let swapped = {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let mut bytes = engine.decode(k.token(shared.id, &a)).unwrap();
        let other_mac = engine.decode(k.token(other.id, &b)).unwrap();
        bytes[16..].copy_from_slice(&other_mac[16..]);
        engine.encode(bytes)
    };
    // a token made under another secret
    let foreign = keys(PREVIOUS, None).token(shared.id, &a);
    // the id of a thread is not a capability
    let by_id = shared.id.to_string();
    let mut tampered = token.clone().into_bytes();
    tampered[40] = if tampered[40] == b'A' { b'B' } else { b'A' };
    let tampered = String::from_utf8(tampered).unwrap();

    for bad in [
        "",
        "x",
        &tampered,
        &swapped,
        &foreign,
        &by_id,
        &format!("{token}="),
        &token[..42],
        &"A".repeat(43),
    ] {
        is_not_found(app.open_public(bad).await);
        is_not_found(app.open_shared(&bob, bad).await);
        is_not_found(app.open_public_artifact(bad, &"a".repeat(64)).await);
    }
    // a private thread has no link: its nonce does not exist
    assert!(private.share.is_none());
    // and the good ones read
    assert!(app.open_public(&token).await.is_ok());
    assert!(app.open_public(&other_token).await.is_ok());
}

#[tokio::test]
async fn the_previous_secret_still_opens_a_link_and_the_owner_copy_carries_the_new_one() {
    let w = World::new();
    let alice = user("alice@example.com");
    let bob = user("bob@example.com");
    let old = app_with_policy(
        &w,
        SharingSettings::new(
            SharingMode::Public,
            Some(keys(PREVIOUS, None)),
            PublicView::default(),
        )
        .unwrap(),
        Policy::default(),
    );
    let t = started(&old, &alice).await;
    old.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let old_token = token_of(&old, &reread(&w, t.id).await);

    // rotate the secret: the new one current, the old one previous
    let rotated = app_with_policy(
        &w,
        SharingSettings::new(
            SharingMode::Public,
            Some(keys(SECRET, Some(PREVIOUS))),
            PublicView::default(),
        )
        .unwrap(),
        Policy::default(),
    );
    assert!(rotated.open_public(&old_token).await.is_ok());
    assert!(rotated.open_shared(&bob, &old_token).await.is_ok());
    let copy = token_of(&rotated, &reread(&w, t.id).await);
    assert_ne!(copy, old_token, "a re-copied link carries the new MAC");
    assert!(rotated.open_public(&copy).await.is_ok());

    // the previous secret dropped: the old link is dead, the owner's next copy works again
    let dropped = app_in(&w, SharingMode::Public);
    is_not_found(dropped.open_public(&old_token).await);
    assert!(dropped.open_public(&copy).await.is_ok());
}

// ---- reading --------------------------------------------------------------------------------

#[tokio::test]
async fn who_may_read_through_a_link() {
    let w = World::new();
    let reader_only = Policy::new(
        BTreeMap::from([
            (Role::new("user"), RoleGrant::user()),
            (Role::new("admin"), RoleGrant::admin()),
            (
                Role::new("reader"),
                RoleGrant {
                    permissions: BTreeSet::from([Permission::ThreadRead]),
                    agents: AgentScope::All,
                },
            ),
            (
                Role::new("agents-only"),
                RoleGrant {
                    permissions: BTreeSet::from([Permission::AgentRead]),
                    agents: AgentScope::All,
                },
            ),
        ]),
        None,
    )
    .unwrap();
    let app = app_with_policy(
        &w,
        settings(SharingMode::Public, PublicView::default()),
        reader_only,
    );
    let alice = user("alice@example.com");
    let t = started(&app, &alice).await;
    app.share_thread(&alice, t.id, ShareLevel::Internal)
        .await
        .unwrap();
    let token = token_of(&app, &reread(&w, t.id).await);

    // the link is the grant: another user, an administrator, a role that only reads
    for (who, writes) in [
        (user("bob@example.com"), true),
        (principal("root@example.com", &["admin"]), true),
        (principal("rita@example.com", &["reader"]), false),
    ] {
        let read = app.open_shared(&who, &token).await.unwrap();
        assert!(!read.is_owner(), "{}", who.user);
        assert_eq!(read.effective(), Visibility::Internal);
        // ...and that is all it grants: the thread is still not theirs
        is_not_found(app.get_thread(&who, t.id).await);
        is_not_found(app.export_thread(&who, t.id).await);
        is_not_found(app.list_events(&who, t.id, 0, 10).await);
        is_not_found(app.event_stream(&who, t.id, 0).await.map(|_| ()));
        is_not_found(app.unshare_thread(&who, t.id).await);
        if writes {
            is_not_found(app.rename_thread(&who, t.id, "mine").await);
            is_not_found(app.post_message(&who, t.id, "hi".to_owned()).await);
            is_not_found(app.cancel(&who, t.id).await);
            is_not_found(app.share_thread(&who, t.id, ShareLevel::Public).await);
        } else {
            // a role that cannot write is refused for what it lacks, as it is for any thread
            assert_eq!(
                forbidden(app.rename_thread(&who, t.id, "mine").await),
                Permission::ThreadWrite
            );
        }
    }
    // a role that does not read is refused, the same for every token
    let agents_only = principal("ann@example.com", &["agents-only"]);
    assert_eq!(
        forbidden(app.open_shared(&agents_only, &token).await),
        Permission::ThreadRead
    );
    assert_eq!(
        forbidden(app.open_shared(&agents_only, "not a token").await),
        Permission::ThreadRead
    );
    // an internal link is not a public one
    is_not_found(app.open_public(&token).await);
    // the owner reading their own link is told so
    assert!(app.open_shared(&alice, &token).await.unwrap().is_owner());
    // read counters: by the visibility served at
    assert!(app.sharing_stats().reads_internal >= 4);
}

#[tokio::test]
async fn a_signed_in_reader_may_read_a_public_link_and_a_public_reader_only_a_public_one() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let (alice, bob) = (user("alice@example.com"), user("bob@example.com"));
    let t = started(&app, &alice).await;
    app.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let token = token_of(&app, &reread(&w, t.id).await);
    let signed_in = app.open_shared(&bob, &token).await.unwrap();
    assert_eq!(signed_in.effective(), Visibility::Public);
    let anonymous = app.open_public(&token).await.unwrap();
    assert!(!anonymous.is_owner());
    let view = serde_json::to_value(app.shared_view(&anonymous)).unwrap();
    assert!(!view.to_string().contains("alice@example.com"));
    assert_eq!(view["visibility"], "public");
    assert_eq!(view["title"], "hello");
    // counted by what the thread is served at: a signed-in read of a public link is one too
    assert_eq!(app.sharing_stats().reads_public, 2);
}

#[tokio::test]
async fn the_feed_a_reader_gets_names_nobody_and_hides_what_the_audience_may_not_see() {
    let w = World::new();
    let public_view = PublicView::default();
    let app = app_with_policy(
        &w,
        settings(SharingMode::Public, public_view),
        Policy::default(),
    );
    let (alice, bob) = (user("alice@example.com"), user("bob@example.com"));
    let t = started(&app, &alice).await;
    append(
        &w,
        t.id,
        vec![
            (coder(), EventBody::AgentStep(a_step())),
            (
                coder(),
                EventBody::Artifact(ArtifactData {
                    name: "pr".into(),
                    mime_type: None,
                    uri: Some("https://github.com/acme/demo/pull/1".into()),
                    text: None,
                    file: None,
                }),
            ),
        ],
    )
    .await;
    app.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let token = token_of(&app, &reread(&w, t.id).await);
    let head = reread(&w, t.id).await.last_seq;

    let step_of = |events: &[Event]| -> AgentStepData {
        events
            .iter()
            .find_map(|e| match &e.body {
                EventBody::AgentStep(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap()
    };

    // the public reader: labels, not input or output; and never the owner's address
    let public = app.open_public(&token).await.unwrap();
    let seen = read_feed(app.shared_feed(&public, 0), head).await;
    assert_eq!(
        seen.len(),
        usize::try_from(head).unwrap(),
        "every seq, in order"
    );
    for (i, e) in seen.iter().enumerate() {
        assert_eq!(e.seq, i64::try_from(i).unwrap() + 1);
        let json = serde_json::to_string(e).unwrap();
        assert!(!json.contains("alice@example.com"), "{json}");
        if e.actor.r#type == ActorType::User && e.kind() != EventKind::ThreadUnshared {
            assert_eq!(e.actor.name, "the owner");
        }
    }
    let step = step_of(&seen);
    assert_eq!(step.label, "Read the notes");
    assert!(step.input.is_none() && step.output.is_none() && step.detail.is_none());
    // the share's own events are there as inert ones, so numbering and cursors hold
    assert!(
        seen.iter()
            .any(|e| e.kind() == EventKind::ThreadUnshared && e.actor == Actor::system())
    );
    assert!(!seen.iter().any(|e| e.kind() == EventKind::ThreadShared));

    // a signed-in reader gets the step whole
    let internal = app.open_shared(&bob, &token).await.unwrap();
    let seen = read_feed(app.shared_feed(&internal, 0), head).await;
    let step = step_of(&seen);
    assert!(step.input.is_some() && step.output.is_some());
    assert!(
        !serde_json::to_string(&seen)
            .unwrap()
            .contains("alice@example.com")
    );

    // and a deployment that lets the public have step input and output says so
    let generous = app_with_policy(
        &w,
        settings(
            SharingMode::Public,
            PublicView {
                step_io: true,
                files: false,
            },
        ),
        Policy::default(),
    );
    let public = generous.open_public(&token).await.unwrap();
    let seen = read_feed(generous.shared_feed(&public, 0), head).await;
    assert!(step_of(&seen).input.is_some());
}

// ---- a stream ends when the link does -------------------------------------------------------

async fn ends(mut feed: BoxStream<'static, FeedItem>) -> bool {
    tokio::time::timeout(Duration::from_secs(5), async {
        while feed.next().await.is_some() {}
    })
    .await
    .is_ok()
}

/// A feed that has been read to the end of the log: what a connected page holds.
async fn open_feed(
    app: &Arc<TestApp>,
    read: &orch_app::SharedRead,
    head: i64,
) -> BoxStream<'static, FeedItem> {
    let mut feed = app.shared_feed(read, 0);
    for _ in 0..head {
        tokio::time::timeout(Duration::from_secs(5), feed.next())
            .await
            .expect("the replay")
            .expect("an item");
    }
    feed
}

#[tokio::test]
async fn a_revocation_ends_a_follow() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let (alice, bob) = (user("alice@example.com"), user("bob@example.com"));
    let t = started(&app, &alice).await;
    app.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let token = token_of(&app, &reread(&w, t.id).await);
    let head = reread(&w, t.id).await.last_seq;
    let public = app.open_public(&token).await.unwrap();
    let internal = app.open_shared(&bob, &token).await.unwrap();
    let mut public_feed = open_feed(&app, &public, head).await;
    let mut internal_feed = open_feed(&app, &internal, head).await;

    // what the owner writes after the share is followed
    app.post_message(&alice, t.id, "more".to_owned())
        .await
        .unwrap();
    for feed in [&mut public_feed, &mut internal_feed] {
        let next = tokio::time::timeout(Duration::from_secs(5), feed.next())
            .await
            .unwrap();
        assert!(matches!(next, Some(FeedItem::Event(_))));
    }

    app.unshare_thread(&alice, t.id).await.unwrap();
    assert!(ends(public_feed).await, "the public stream ends");
    assert!(ends(internal_feed).await, "the signed-in stream ends");
}

#[tokio::test]
async fn a_new_link_and_a_narrowing_end_the_streams_that_need_what_is_gone() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let (alice, bob) = (user("alice@example.com"), user("bob@example.com"));
    let t = started(&app, &alice).await;
    app.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let token = token_of(&app, &reread(&w, t.id).await);
    let head = reread(&w, t.id).await.last_seq;
    let public_read = app.open_public(&token).await.unwrap();
    let internal_read = app.open_shared(&bob, &token).await.unwrap();
    let public_feed = open_feed(&app, &public_read, head).await;
    let mut internal_feed = open_feed(&app, &internal_read, head).await;

    // narrowed to signed-in people: the public stream ends, the signed-in one goes on
    app.share_thread(&alice, t.id, ShareLevel::Internal)
        .await
        .unwrap();
    assert!(ends(public_feed).await);
    app.post_message(&alice, t.id, "still here".to_owned())
        .await
        .unwrap();
    // its next items are the narrowing's events, then the message
    let mut saw_message = false;
    while let Ok(Some(item)) =
        tokio::time::timeout(Duration::from_secs(5), internal_feed.next()).await
    {
        if let FeedItem::Event(e) = item
            && e.kind() == EventKind::UserMessage
        {
            saw_message = true;
            break;
        }
    }
    assert!(saw_message, "the signed-in stream followed on");

    // a new link ends it: the nonce it was opened for is gone
    app.rotate_share(&alice, t.id).await.unwrap();
    assert!(ends(internal_feed).await);
}

#[tokio::test]
async fn a_stream_re_reads_the_share_when_nothing_wakes_it() {
    let w = World::new();
    let alice = user("alice@example.com");
    let app = app_with_policy(
        &w,
        settings(SharingMode::Public, PublicView::default())
            .with_recheck(Duration::from_millis(60)),
        Policy::default(),
    );
    let t = started(&app, &alice).await;
    app.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let token = token_of(&app, &reread(&w, t.id).await);
    let head = reread(&w, t.id).await.last_seq;
    let read = app.open_public(&token).await.unwrap();
    let feed = open_feed(&app, &read, head).await;

    // the row is cleared behind the application's back: no `thread_unshared` event passes
    let row = reread(&w, t.id).await;
    w.store
        .commit(
            t.id,
            row.version,
            Commit {
                new_state: row.state,
                job: None,
                events: vec![],
                outbox: vec![],
                binding: None,
                now: jiff::Timestamp::now(),
                lease: None,
                watches: vec![],
                timers: vec![],
                inbox: None,
                finishes_outbox: None,
                title: None,
                description: None,
                sharing: Some(SharingChange::Clear),
                skip_unsent_delegates: false,
            },
        )
        .await
        .unwrap();
    assert!(ends(feed).await, "the periodic re-read ends it");
}

// ---- files ----------------------------------------------------------------------------------

type FilePorts = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    ScriptedModel,
    FixedRegistry,
    orch_ports::RefuseAll,
    MemoryArtifacts,
>;

fn file_app(w: &World, store: &MemoryArtifacts, public: PublicView) -> Arc<App<FilePorts>> {
    Arc::new(
        App::<FilePorts>::new(
            PortSet {
                artifacts: store.clone(),
                store: w.store.clone(),
                wakeup: w.wakeup.clone(),
                agents: w.agent.clone(),
                clock: SystemClock,
                ids: w.ids.clone(),
                model: w.model.clone(),
                auth: orch_ports::RefuseAll,
                registry: directory().fixed_registry(),
            },
            directory(),
            AppConfig {
                sharing: settings(SharingMode::Public, public),
                ..AppConfig::default()
            },
        )
        .unwrap(),
    )
}

async fn body_of((meta, mut stream): (ArtifactMeta, orch_ports::ByteStream)) -> (u64, Vec<u8>) {
    let mut body = Vec::new();
    while let Some(piece) = stream.next().await {
        body.extend_from_slice(&piece.unwrap());
    }
    (meta.size, body)
}

#[tokio::test]
async fn a_shared_file_is_one_the_threads_log_names_and_a_public_reader_needs_the_setting() {
    let w = World::new();
    let store = MemoryArtifacts::new();
    let app = file_app(&w, &store, PublicView::default());
    let (alice, bob) = (user("alice@example.com"), user("bob@example.com"));
    let t = app
        .create_thread(&alice, new_thread("plain", "hi"))
        .await
        .unwrap();
    let other = app
        .create_thread(&alice, new_thread("plain", "other"))
        .await
        .unwrap();

    let put = |thread: ThreadId, bytes: &'static [u8]| {
        let store = store.clone();
        async move {
            let bytes = Bytes::from_static(bytes);
            let meta = ArtifactMeta::of("text/plain", Some("n.txt".to_owned()), &bytes);
            let key = ArtifactKey::new(thread, meta.sha256);
            store.put(&key, bytes, &meta).await.unwrap();
            (key.sha256_hex(), meta)
        }
    };
    let (named, meta) = put(t.id, b"hello, file").await;
    let (unnamed, _) = put(t.id, b"never announced").await;
    let (others, _) = put(other.id, b"another thread's").await;
    append(
        &w,
        t.id,
        vec![(
            coder(),
            EventBody::Artifact(ArtifactData {
                name: "n.txt".into(),
                mime_type: Some("text/plain".into()),
                uri: None,
                text: None,
                file: Some(FileRef {
                    sha256: named.clone(),
                    size: meta.size,
                    filename: Some("n.txt".into()),
                }),
            }),
        )],
    )
    .await;
    app.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let token = token_of(&app, &reread(&w, t.id).await);

    // a signed-in reader (artifact.read, thread.read) opens the file the log names
    let (size, body) = body_of(
        app.open_shared_artifact(&bob, &token, &named)
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((size, body.as_slice()), (11, &b"hello, file"[..]));
    // a key that is in the store but not in this thread's log, another thread's, a made-up one
    for sha in [&unnamed, &others, &"f".repeat(64), &"nope".to_owned()] {
        is_not_found(app.open_shared_artifact(&bob, &token, sha).await);
    }
    // a public reader has no files unless the deployment says so
    is_not_found(app.open_public_artifact(&token, &named).await);
    let generous = file_app(
        &w,
        &store,
        PublicView {
            step_io: false,
            files: true,
        },
    );
    let (_, body) = body_of(generous.open_public_artifact(&token, &named).await.unwrap()).await;
    assert_eq!(body, b"hello, file");
    for sha in [&unnamed, &others] {
        is_not_found(generous.open_public_artifact(&token, sha).await);
    }
    // an internal link is no public one, files included
    generous
        .share_thread(&alice, t.id, ShareLevel::Internal)
        .await
        .unwrap();
    is_not_found(generous.open_public_artifact(&token, &named).await);
    assert!(
        generous
            .open_shared_artifact(&bob, &token, &named)
            .await
            .is_ok()
    );
}

// ---- a fork is private ----------------------------------------------------------------------

#[tokio::test]
async fn a_fork_of_a_shared_thread_is_private_and_the_link_is_still_the_parents() {
    let w = World::new();
    let app = app_in(&w, SharingMode::Public);
    let alice = user("alice@example.com");
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = app
        .create_thread(&alice, new_thread("plain", "echo one"))
        .await
        .unwrap();
    eventually("the first turn to end", || async {
        (state_of(&w, t.id).await == orch_core::ThreadState::Done).then_some(())
    })
    .await;
    app.share_thread(&alice, t.id, ShareLevel::Public)
        .await
        .unwrap();
    let token = token_of(&app, &reread(&w, t.id).await);

    let forked = app
        .fork_thread(
            &alice,
            t.id,
            orch_app::ForkRequest {
                at: orch_app::ForkAt::AfterTurn {
                    seq: 1,
                    first: None,
                },
                target: None,
                id: None,
            },
        )
        .await
        .unwrap();
    run.shutdown().await;
    let fork = reread(&w, forked.thread.id).await;
    assert!(fork.share.is_none(), "a fork starts private");
    assert!(app.share_view(&fork).is_none());
    // the parent's link finds the parent, and the fork has no link of its own to find
    let read = app.open_public(&token).await.unwrap();
    assert_eq!(read.thread().id, t.id);
    // sharing the fork is the owner's own act, with a link of its own
    app.share_thread(&alice, fork.id, ShareLevel::Internal)
        .await
        .unwrap();
    let fork_token = token_of(&app, &reread(&w, fork.id).await);
    assert_ne!(fork_token, token);
}
