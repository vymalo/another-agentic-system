//! The access tier of a message (ADR 0061, `access-tier/v1`) through the application, the
//! dispatcher and the scripted agent: what is checked before anything is written (the person's
//! cap, the live card), what the dispatcher does when it sends (the card is read again, the message
//! never goes without its tier), and what the agent's echo becomes in the log.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use orch_app::{
    AgentScope, AppConfig, AppError, Creation, Inbound, NewThread, Permission, Policy, RoleGrant,
};
use orch_core::{
    AccessTier, Classify, ErrorClass, EventBody, Input, Origin, ThreadId, ThreadState, TierOffer,
};
use orch_ports::memory::Call;
use orch_ports::{OutboxPayload, OutboxStatus, Principal, Role, ThreadStore};
use serde_json::json;
use support::*;

use AccessTier::{Auto, Bypass, Edits, Plan};

fn offer(tiers: &[&str]) -> TierOffer {
    TierOffer::from_params(&json!({"tiers": tiers, "default": tiers[0]})).unwrap()
}

/// A world whose agent `plain` lists `access-tier/v1` with all four tiers.
fn tiered() -> World {
    let w = World::new();
    w.agent
        .set_access_tier("plain", Some(offer(&["plan", "edits", "auto", "bypass"])));
    w
}

fn id() -> ThreadId {
    ThreadId(uuid::Uuid::now_v7())
}

fn new_thread(text: &str) -> NewThread {
    NewThread {
        title: None,
        target: target("plain"),
        text: text.to_owned(),
    }
}

async fn start(
    app: &TestApp,
    who: &impl orch_app::Requester,
    text: &str,
    tier: Option<AccessTier>,
) -> Result<ThreadId, AppError> {
    let thread = id();
    match app
        .create_thread_as(
            who,
            thread,
            new_thread(text),
            Inbound {
                tier,
                ..Inbound::default()
            },
        )
        .await?
    {
        Creation::Created { .. } => Ok(thread),
        Creation::Exists => panic!("a fresh id"),
    }
}

fn message(text: &str, tier: Option<AccessTier>) -> Input {
    Input::UserMessage {
        user: alice(),
        text: text.to_owned(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier,
    }
}

fn stop(text: &str, tier: Option<AccessTier>) -> Input {
    Input::StopAndSend {
        user: alice(),
        text: text.to_owned(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier,
    }
}

/// What reached the agent `plain` as messages: the text and the tier, in order.
fn sent(w: &World) -> Vec<(String, bool, Option<AccessTier>)> {
    w.agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send {
                text, steer, tier, ..
            } => Some((text, steer, tier)),
            _ => None,
        })
        .collect()
}

fn card_reads(w: &World) -> usize {
    w.agent
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::ReadCard { .. }))
        .count()
}

async fn tiers_logged(app: &TestApp, thread: ThreadId) -> Vec<Option<AccessTier>> {
    events(app, &alice(), thread)
        .await
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::UserMessage(m) => Some(m.tier),
            _ => None,
        })
        .collect()
}

async fn echoes(app: &TestApp, thread: ThreadId) -> Vec<Option<AccessTier>> {
    events(app, &alice(), thread)
        .await
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::AgentStatus(s) => Some(s.tier),
            _ => None,
        })
        .collect()
}

// ---- before anything is written ---------------------------------------------------------

#[tokio::test]
async fn a_tier_the_card_honours_is_written_as_sent_with_the_message_and_the_delegation() {
    for tier in AccessTier::ALL {
        let w = tiered();
        let app = w.app();
        let thread = start(&app, &alice(), "go", Some(tier)).await.unwrap();
        assert_eq!(tiers_logged(&app, thread).await, [Some(tier)], "{tier}");
        let rows = w.store.list_open_outbox(thread).await.unwrap();
        let [row] = rows.as_slice() else {
            panic!("{rows:?}")
        };
        let OutboxPayload::Delegate { tier: held, .. } = &row.payload else {
            panic!("{:?}", row.payload)
        };
        assert_eq!(*held, Some(tier), "the row carries it for the dispatcher");
    }
}

#[tokio::test]
async fn a_message_that_names_no_tier_reads_no_card_and_is_written_as_it_always_was() {
    let w = World::new();
    let app = w.app();
    let thread = start(&app, &alice(), "go", None).await.unwrap();
    assert_eq!(tiers_logged(&app, thread).await, [None]);
    assert_eq!(card_reads(&w), 0, "the card is read only for a tier");
    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let row = serde_json::to_value(&rows[0].payload).unwrap();
    assert!(row["delegate"].get("tier").is_none(), "{row}");
}

#[tokio::test]
async fn a_card_that_does_not_honour_the_tier_is_a_422_and_nothing_is_written() {
    // not listed; listed but not this tier; listed badly (a card that does not list it)
    let w = World::new();
    let app = w.app();
    let refused = start(&app, &alice(), "go", Some(Plan)).await.unwrap_err();
    assert!(
        matches!(&refused, AppError::Unprocessable(m) if m == "this agent does not take the tier 'plan'"),
        "{refused:?}"
    );
    assert_eq!(refused.class(), ErrorClass::Rejected);

    let w = World::new();
    w.agent
        .set_access_tier("plain", Some(offer(&["plan", "edits"])));
    let app = w.app();
    let refused = start(&app, &alice(), "go", Some(Auto)).await.unwrap_err();
    assert!(
        matches!(&refused, AppError::Unprocessable(m) if m == "this agent does not take the tier 'auto'"),
        "{refused:?}"
    );
    // and the ones it lists go
    assert!(start(&app, &alice(), "go", Some(Edits)).await.is_ok());
}

#[tokio::test]
async fn nothing_is_written_when_the_card_refuses() {
    let w = World::new();
    let app = w.app();
    for _ in 0..3 {
        start(&app, &alice(), "go", Some(Plan)).await.unwrap_err();
    }
    let listed = app
        .list_threads(&alice(), orch_ports::ThreadListing::recent(None, 50, false))
        .await
        .unwrap();
    assert!(listed.is_empty(), "{listed:?}");
}

#[tokio::test]
async fn a_card_that_cannot_be_read_is_a_503_and_nothing_is_written() {
    let w = tiered();
    w.agent.set_card_down("plain", true);
    let app = w.app();
    let error = start(&app, &alice(), "go", Some(Plan)).await.unwrap_err();
    assert!(
        matches!(&error, AppError::CardUnavailable { agent } if agent.as_str() == "plain"),
        "{error:?}"
    );
    assert!(error.class().is_retryable(), "the caller can try again");
    let listed = app
        .list_threads(&alice(), orch_ports::ThreadListing::recent(None, 50, false))
        .await
        .unwrap();
    assert!(listed.is_empty());
    // a message with no tier does not read the card, so it is not held back by it
    assert!(start(&app, &alice(), "go", None).await.is_ok());
}

fn capped(cap: Option<AccessTier>) -> RoleGrant {
    RoleGrant {
        permissions: BTreeSet::from([
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
            Permission::ThreadWrite,
        ]),
        agents: AgentScope::All,
        max_tier: cap,
    }
}

fn who(roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(alice())
    }
}

fn under(w: &World, roles: &[(&str, RoleGrant)]) -> Arc<TestApp> {
    w.app_with(AppConfig {
        policy: Policy::new(
            roles
                .iter()
                .map(|(name, grant)| (Role::new(*name), grant.clone()))
                .collect::<BTreeMap<_, _>>(),
            None,
        )
        .unwrap(),
        stream_poll: Duration::from_millis(100),
        ..AppConfig::default()
    })
}

#[tokio::test]
async fn a_tier_above_what_the_role_allows_is_a_422_before_the_card_is_read() {
    let w = tiered();
    let app = under(&w, &[("careful", capped(Some(Auto)))]);
    let careful = who(&["careful"]);
    // the card is down, and the answer is still the cap's: it is asked first
    w.agent.set_card_down("plain", true);
    let error = start(&app, &careful, "go", Some(Bypass)).await.unwrap_err();
    assert!(
        matches!(&error, AppError::Unprocessable(m) if m == "the tier 'bypass' is above what your role allows"),
        "{error:?}"
    );
    assert_eq!(card_reads(&w), 0);
    w.agent.set_card_down("plain", false);
    // up to the cap, inclusive
    for tier in [Plan, Edits, Auto] {
        assert!(
            start(&app, &careful, "go", Some(tier)).await.is_ok(),
            "{tier}"
        );
    }
    assert!(start(&app, &careful, "go", Some(Bypass)).await.is_err());
}

#[tokio::test]
async fn the_cap_of_a_person_with_several_roles_is_the_highest_and_a_role_without_one_has_none() {
    let w = tiered();
    let app = under(
        &w,
        &[
            ("low", capped(Some(Plan))),
            ("mid", capped(Some(Edits))),
            ("open", capped(None)),
        ],
    );
    let access = |roles: &[&str]| app.access(&who(roles)).max_tier();
    assert_eq!(access(&["low"]), Some(Plan));
    assert_eq!(access(&["low", "mid"]), Some(Edits));
    assert_eq!(
        access(&["low", "open"]),
        None,
        "one role without a cap has none"
    );
    assert_eq!(access(&["open"]), None);
    // so the higher one picks bypass or not as the roles say
    assert!(
        start(&app, &who(&["low", "mid"]), "go", Some(Edits))
            .await
            .is_ok()
    );
    assert!(
        start(&app, &who(&["low", "mid"]), "go", Some(Auto))
            .await
            .is_err()
    );
    assert!(
        start(&app, &who(&["low", "open"]), "go", Some(Bypass))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn the_built_in_roles_have_no_cap() {
    let w = tiered();
    let app = w.app();
    assert_eq!(app.access(&who(&["user"])).max_tier(), None);
    assert_eq!(app.access(&who(&["admin"])).max_tier(), None);
    assert!(
        start(&app, &who(&["user"]), "go", Some(Bypass))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_message_to_a_thread_is_checked_as_the_first_one_is_and_leaves_the_log_alone_when_refused()
 {
    let w = tiered();
    let app = w.app();
    let thread = start(&app, &alice(), "go", Some(Auto)).await.unwrap();
    let before = events(&app, &alice(), thread).await.len();
    // the card stops honouring bypass
    w.agent
        .set_access_tier("plain", Some(offer(&["plan", "edits", "auto"])));
    for input in [message("more", Some(Bypass)), stop("now", Some(Bypass))] {
        let error = app.submit(&alice(), thread, input, None).await.unwrap_err();
        assert!(
            matches!(&error, AppError::Unprocessable(m) if m == "this agent does not take the tier 'bypass'"),
            "{error:?}"
        );
    }
    assert_eq!(events(&app, &alice(), thread).await.len(), before);
    // and one it honours is written, as sent
    app.submit(&alice(), thread, message("more", Some(Plan)), None)
        .await
        .unwrap();
    assert_eq!(tiers_logged(&app, thread).await, [Some(Auto), Some(Plan)]);
}

// ---- when the dispatcher sends -------------------------------------------------------------

#[tokio::test]
async fn the_message_goes_with_its_tier_and_the_echo_is_written_on_the_status() {
    let w = tiered();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "echo go", Some(Plan)).await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    assert_eq!(sent(&w), [("echo go".to_owned(), false, Some(Plan))]);
    // what the agent said it worked under is on its first status, never compared with the ask
    let said = echoes(&app, thread).await;
    assert_eq!(said.first(), Some(&Some(Plan)), "{said:?}");
    assert_eq!(said.iter().filter(|t| t.is_some()).count(), 1, "once");
    run.shutdown().await;
}

#[tokio::test]
async fn a_message_with_no_tier_goes_with_none_and_its_statuses_say_none() {
    let w = tiered();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "echo go", None).await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    assert_eq!(sent(&w), [("echo go".to_owned(), false, None)]);
    assert!(echoes(&app, thread).await.iter().all(Option::is_none));
    run.shutdown().await;
}

#[tokio::test]
async fn a_card_that_stopped_honouring_the_tier_fails_the_job_and_nothing_is_sent() {
    let w = tiered();
    let app = w.app();
    let thread = start(&app, &alice(), "echo go", Some(Bypass))
        .await
        .unwrap();
    let rows: Vec<_> = w
        .store
        .list_open_outbox(thread)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    // between the check and the send the agent stops listing it (the card is read again)
    w.agent.set_access_tier("plain", None);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let record = wait_state(&app, &alice(), thread, ThreadState::Failed).await;
    assert_eq!(record.state, ThreadState::Failed);
    assert!(sent(&w).is_empty(), "never the message without its tier");
    let log = events(&app, &alice(), thread).await;
    let said = log
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Error(m) => Some(m.message.clone()),
            EventBody::AgentStatus(s) => s.detail.clone(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        said.contains("the agent no longer takes the access tier 'bypass'"),
        "{said}"
    );
    for id in rows {
        let row = w.store.get_outbox(id).await.unwrap().unwrap();
        assert_eq!(row.status, OutboxStatus::Dead, "{row:?}");
    }
    run.shutdown().await;
}

#[tokio::test]
async fn a_card_that_cannot_be_read_at_the_send_is_retried_and_the_tier_goes_when_it_is_back() {
    let w = tiered();
    let app = w.app();
    let thread = start(&app, &alice(), "echo go", Some(Edits)).await.unwrap();
    w.agent.set_card_down("plain", true);
    let run = spawn_dispatcher(&app, fast(), "d1");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        sent(&w).is_empty(),
        "nothing is sent while the card is down"
    );
    assert_ne!(
        app.get_thread(&alice(), thread).await.unwrap().state,
        ThreadState::Failed,
        "a card that is down is a retry, not a failure"
    );
    w.agent.set_card_down("plain", false);
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    assert_eq!(sent(&w), [("echo go".to_owned(), false, Some(Edits))]);
    run.shutdown().await;
}

#[tokio::test]
async fn a_message_sent_while_the_job_works_is_steered_with_its_own_tier() {
    let w = tiered();
    w.agent
        .set_extensions("plain", &[orch_core::KnownExtension::Steer]);
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "gate go", Some(Auto)).await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Working).await;
    app.submit(&alice(), thread, message("faster", Some(Plan)), None)
        .await
        .unwrap();
    eventually("the steer reached the agent", || async {
        sent(&w).iter().any(|(_, steer, _)| *steer).then_some(())
    })
    .await;
    assert_eq!(
        sent(&w),
        [
            ("gate go".to_owned(), false, Some(Auto)),
            ("faster".to_owned(), true, Some(Plan)),
        ]
    );
    // one that names none steers with none: the person picks again for each message
    app.submit(&alice(), thread, message("and again", None), None)
        .await
        .unwrap();
    eventually("the second steer reached the agent", || async {
        (sent(&w).len() == 3).then_some(())
    })
    .await;
    assert_eq!(sent(&w)[2], ("and again".to_owned(), true, None));
    w.agent.release_gate();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn a_stop_and_send_starts_the_next_job_with_the_tier_of_its_messages() {
    let w = tiered();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "slow job", Some(Auto)).await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Working).await;
    app.submit(&alice(), thread, stop("echo instead", Some(Plan)), None)
        .await
        .unwrap();
    // a message that names none leaves it: the newest that names one wins
    app.submit(&alice(), thread, stop("and more", None), None)
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    let tiers: Vec<_> = sent(&w)
        .into_iter()
        .map(|(_, steer, tier)| (steer, tier))
        .collect();
    assert_eq!(tiers, [(false, Some(Auto)), (false, Some(Plan))]);
    assert_eq!(
        tiers_logged(&app, thread).await,
        [Some(Auto), Some(Plan), None],
        "each message is logged as it was sent"
    );
    run.shutdown().await;
}
