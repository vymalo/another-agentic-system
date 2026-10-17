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
    AgentScope, AppConfig, AppError, Creation, FirstMessage, ForkAt, ForkRequest, Inbound,
    NewThread, Permission, Policy, RoleGrant,
};
use orch_core::{
    AccessTier, Classify, ErrorClass, EventBody, EventKind, Input, Origin, ThreadId, ThreadState,
    TierOffer, UiActionData, UiVersion,
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
        tier_cap: None,
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
        tier_cap: None,
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

// ---- the orchestrator's own messages on a thread's behalf (ADR 0061) ----------------------

fn mcp_message(text: &str) -> Input {
    Input::UserMessage {
        tier_cap: None,
        user: alice(),
        text: text.to_owned(),
        message_id: None,
        run_id: None,
        origin: Origin::Mcp,
        catalog: None,
        mentions: Vec::new(),
        tier: None,
    }
}

#[tokio::test]
async fn the_rework_of_a_gate_is_sent_to_the_agent_under_the_tier_of_the_job() {
    let w = tiered();
    let app = w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        gate: orch_core::GatePolicy::requiring([orch_core::CheckSource::AgentChecks]),
        ..AppConfig::default()
    });
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "gate-push one", Some(Edits))
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Working).await;
    // the agent pushes and finishes without checks: the gate sends it back
    w.agent.release_gate();
    eventually("the rework reaches the agent", || async {
        (sent(&w).len() >= 2).then_some(())
    })
    .await;
    let sends = sent(&w);
    assert_eq!(sends[0], ("gate-push one".to_owned(), false, Some(Edits)));
    assert!(sends[1].0.contains("attempt 2"), "{:?}", sends[1].0);
    assert_eq!(
        sends[1].2,
        Some(Edits),
        "the rework is sent under the tier the person's message was, not the agent's default"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_rework_of_a_job_that_chose_no_tier_goes_with_none() {
    let w = tiered();
    let app = w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        gate: orch_core::GatePolicy::requiring([orch_core::CheckSource::AgentChecks]),
        ..AppConfig::default()
    });
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "gate-push one", None).await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Working).await;
    w.agent.release_gate();
    eventually("the rework reaches the agent", || async {
        (sent(&w).len() >= 2).then_some(())
    })
    .await;
    assert_eq!(sent(&w)[1].2, None);
    run.shutdown().await;
}

#[tokio::test]
async fn an_answer_from_mcp_continues_the_tier_of_the_job_and_a_message_that_starts_a_new_job_does_not()
 {
    let w = tiered();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "ask which branch", Some(Edits))
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Blocked).await;
    // the answer names no tier (the MCP surface cannot): it continues the task under the last choice
    app.submit(&alice(), thread, mcp_message("main"), None)
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    // a message to the finished thread is a new job, however it comes in: the choice of the job
    // before is not its choice, and a person with no cap gets the agent as it always was
    app.submit(&alice(), thread, mcp_message("echo again"), None)
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    // and a person's own message with no pick is sent as it is: none (the web sends the pick every time)
    app.submit(&alice(), thread, message("echo plain", None), None)
        .await
        .unwrap();
    eventually("the last message reaches the agent", || async {
        (sent(&w).len() == 4).then_some(())
    })
    .await;
    let tiers: Vec<_> = sent(&w).into_iter().map(|(text, _, t)| (text, t)).collect();
    assert_eq!(
        tiers,
        [
            ("ask which branch".to_owned(), Some(Edits)),
            ("main".to_owned(), Some(Edits)),
            ("echo again".to_owned(), None),
            ("echo plain".to_owned(), None),
        ]
    );
    assert_eq!(
        tiers_logged(&app, thread).await,
        [Some(Edits), Some(Edits), None, None],
        "the log says what each message was sent under"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_message_from_mcp_to_a_working_job_steers_under_the_tier_of_the_job() {
    let w = tiered();
    w.agent
        .set_extensions("plain", &[orch_core::KnownExtension::Steer]);
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "gate go", Some(Auto)).await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Working).await;
    app.submit(&alice(), thread, mcp_message("faster"), None)
        .await
        .unwrap();
    eventually("the steer reached the agent", || async {
        sent(&w).iter().any(|(_, steer, _)| *steer).then_some(())
    })
    .await;
    assert_eq!(sent(&w)[1], ("faster".to_owned(), true, Some(Auto)));
    w.agent.release_gate();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn an_answer_is_sent_under_the_last_choice_capped_by_the_role_of_whoever_gives_it() {
    let w = tiered();
    // a person with no role of their own is `free`: no cap
    let app = w.app_with(AppConfig {
        policy: Policy::new(
            BTreeMap::from([
                (Role::new("free"), capped(None)),
                (Role::new("careful"), capped(Some(Plan))),
            ]),
            Some(Role::new("free")),
        )
        .unwrap(),
        stream_poll: Duration::from_millis(100),
        ..AppConfig::default()
    });
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "ask which branch", Some(Edits))
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Blocked).await;
    // the person's role now caps at plan: the answer goes under the lower of the two
    app.submit(&who(&["careful"]), thread, mcp_message("main"), None)
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    assert_eq!(sent(&w)[1], ("main".to_owned(), false, Some(Plan)));
    run.shutdown().await;
}

#[tokio::test]
async fn an_answer_the_card_no_longer_allows_the_choice_for_goes_under_the_highest_tier_it_does() {
    let w = tiered();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "ask which branch", Some(Bypass))
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Blocked).await;
    // the agent stops listing bypass: the answer is never raised, and goes under the highest below
    w.agent
        .set_access_tier("plain", Some(offer(&["plan", "edits"])));
    app.submit(&alice(), thread, mcp_message("main"), None)
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    assert_eq!(sent(&w)[1], ("main".to_owned(), false, Some(Edits)));
    assert_eq!(
        tiers_logged(&app, thread).await,
        [Some(Bypass), Some(Edits)],
        "the log says the tier it was sent under"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn an_answer_to_an_agent_that_no_longer_lists_tiers_is_refused_and_writes_nothing() {
    let w = tiered();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "ask which branch", Some(Edits))
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Blocked).await;
    let before = tiers_logged(&app, thread).await;
    w.agent.set_access_tier("plain", None);
    let error = app
        .submit(&alice(), thread, mcp_message("main"), None)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, AppError::Unprocessable(m) if m.contains("no longer lists access tiers") && m.contains("start a new chat")),
        "{error:?}"
    );
    assert_eq!(tiers_logged(&app, thread).await, before, "nothing written");
    assert_eq!(sent(&w).len(), 1, "and nothing sent");
    // a card that is down is not an answer either
    w.agent
        .set_access_tier("plain", Some(offer(&["plan", "edits"])));
    w.agent.set_card_down("plain", true);
    let error = app
        .submit(&alice(), thread, mcp_message("main"), None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, AppError::CardUnavailable { .. }),
        "{error:?}"
    );
    run.shutdown().await;
}

// ---- a capped role that names no tier (ADR 0061, the resolver of `App::resolve_tier`) -------

fn offer_defaulting(tiers: &[&str], default: &str) -> TierOffer {
    TierOffer::from_params(&json!({"tiers": tiers, "default": default})).unwrap()
}

/// A world whose agent `plain` lists all four tiers and works under `auto` when none is named.
fn defaulting_to_auto() -> World {
    let w = World::new();
    w.agent.set_access_tier(
        "plain",
        Some(offer_defaulting(
            &["plan", "edits", "auto", "bypass"],
            "auto",
        )),
    );
    w
}

async fn delegated_tier(w: &World, thread: ThreadId) -> Option<AccessTier> {
    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let [row] = rows.as_slice() else {
        panic!("{rows:?}")
    };
    let OutboxPayload::Delegate { tier, .. } = &row.payload else {
        panic!("{:?}", row.payload)
    };
    *tier
}

/// An application whose every person (no role of their own: the default) is capped at `cap`.
fn capped_at(w: &World, cap: Option<AccessTier>) -> Arc<TestApp> {
    w.app_with(AppConfig {
        policy: Policy::new(
            BTreeMap::from([(Role::new("careful"), capped(cap))]),
            Some(Role::new("careful")),
        )
        .unwrap(),
        stream_poll: Duration::from_millis(100),
        ..AppConfig::default()
    })
}

async fn threads_of(app: &TestApp, who: &impl orch_app::Requester) -> usize {
    app.list_threads(who, orch_ports::ThreadListing::recent(None, 50, false))
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn a_capped_role_that_names_no_tier_is_run_at_its_cap_never_at_the_agents_default() {
    // the agent works under `auto`; the cap is the lower of the two
    for (cap, runs_at) in [(Plan, Plan), (Edits, Edits), (Auto, Auto), (Bypass, Auto)] {
        let w = defaulting_to_auto();
        let app = capped_at(&w, Some(cap));
        let careful = alice();
        let thread = start(&app, &careful, "go", None).await.unwrap();
        assert_eq!(
            tiers_logged(&app, thread).await,
            [Some(runs_at)],
            "cap {cap}: the log says what the message is sent under"
        );
        assert_eq!(
            delegated_tier(&w, thread).await,
            Some(runs_at),
            "cap {cap}: and so does the row the dispatcher sends"
        );
        let job = app.get_thread(&careful, thread).await.unwrap().job;
        assert_eq!(
            job.tier,
            Some(runs_at),
            "cap {cap}: the job is pinned to it"
        );
    }
}

#[tokio::test]
async fn the_tier_a_capped_role_is_given_is_the_highest_the_card_lists_within_the_limit() {
    // the card skips edits: the highest at or below edits is plan
    let w = World::new();
    w.agent
        .set_access_tier("plain", Some(offer_defaulting(&["plan", "auto"], "auto")));
    let app = capped_at(&w, Some(Edits));
    let thread = start(&app, &alice(), "go", None).await.unwrap();
    assert_eq!(tiers_logged(&app, thread).await, [Some(Plan)]);

    // the agent's default is below the cap: the default, not the cap
    let w = World::new();
    w.agent.set_access_tier(
        "plain",
        Some(offer_defaulting(&["plan", "edits", "auto"], "plan")),
    );
    let app = capped_at(&w, Some(Auto));
    let thread = start(&app, &alice(), "go", None).await.unwrap();
    assert_eq!(tiers_logged(&app, thread).await, [Some(Plan)]);
}

#[tokio::test]
async fn an_agent_with_no_tier_a_capped_role_may_use_is_a_422_and_nothing_is_written() {
    let w = World::new();
    w.agent
        .set_access_tier("plain", Some(offer_defaulting(&["auto", "bypass"], "auto")));
    let app = capped_at(&w, Some(Edits));
    let careful = alice();
    let error = start(&app, &careful, "go", None).await.unwrap_err();
    assert!(
        matches!(&error, AppError::Unprocessable(m) if m == "this agent has no access tier at or below 'edits', the most your role allows"),
        "{error:?}"
    );
    assert_eq!(error.class(), ErrorClass::Rejected);
    assert_eq!(threads_of(&app, &careful).await, 0);
}

#[tokio::test]
async fn a_capped_role_and_an_agent_that_lists_no_tiers_send_plain_a2a() {
    // the cap cannot be said to an agent that does not list the extension: the message is the one
    // it always was (the contract says so), and the cap stays on the job
    let w = World::new();
    let app = capped_at(&w, Some(Edits));
    let careful = alice();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &careful, "echo go", None).await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    assert_eq!(tiers_logged(&app, thread).await, [None]);
    assert_eq!(sent(&w), [("echo go".to_owned(), false, None)]);
    assert!(card_reads(&w) >= 1, "the card was read to know that");
    // but the cap is kept (it applies to the day the agent lists tiers): see `tier_caps.rs`
    let job = app.get_thread(&careful, thread).await.unwrap().job;
    assert_eq!((job.tier, job.tier_cap), (None, Some(Edits)));
    run.shutdown().await;
}

#[tokio::test]
async fn a_capped_role_and_a_card_that_cannot_be_read_is_a_503_and_nothing_is_written() {
    let w = defaulting_to_auto();
    w.agent.set_card_down("plain", true);
    let app = capped_at(&w, Some(Edits));
    let careful = alice();
    let error = start(&app, &careful, "go", None).await.unwrap_err();
    assert!(
        matches!(&error, AppError::CardUnavailable { agent } if agent.as_str() == "plain"),
        "{error:?}"
    );
    assert_eq!(threads_of(&app, &careful).await, 0);
    // a person with no cap does not read the card for a message that names none
    let app = w.app();
    assert!(start(&app, &alice(), "go", None).await.is_ok());
}

#[tokio::test]
async fn a_capped_role_is_sent_under_its_cap_for_every_message_that_names_no_tier() {
    let w = defaulting_to_auto();
    w.agent
        .set_extensions("plain", &[orch_core::KnownExtension::Steer]);
    let app = capped_at(&w, Some(Edits));
    let careful = alice();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &careful, "gate go", None).await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Working).await;
    // the person does not pick again: the next message does not fall back to the agent's `auto`
    app.submit(&careful, thread, message("faster", None), None)
        .await
        .unwrap();
    eventually("the steer reached the agent", || async {
        sent(&w).iter().any(|(_, steer, _)| *steer).then_some(())
    })
    .await;
    w.agent.release_gate();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    app.submit(&careful, thread, message("echo more", None), None)
        .await
        .unwrap();
    eventually("the follow-up reached the agent", || async {
        (sent(&w).len() == 3).then_some(())
    })
    .await;
    assert_eq!(
        sent(&w),
        [
            ("gate go".to_owned(), false, Some(Edits)),
            ("faster".to_owned(), true, Some(Edits)),
            ("echo more".to_owned(), false, Some(Edits)),
        ]
    );
    assert_eq!(
        tiers_logged(&app, thread).await,
        [Some(Edits), Some(Edits), Some(Edits)]
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_message_from_mcp_that_starts_a_new_job_of_a_capped_role_is_resolved_as_a_new_one() {
    let w = defaulting_to_auto();
    let app = capped_at(&w, Some(Edits));
    let careful = alice();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &careful, "echo go", Some(Plan)).await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    // a new job: not the plan of the job before, and not above the cap
    app.submit(&careful, thread, mcp_message("echo again"), None)
        .await
        .unwrap();
    eventually("the second job reaches the agent", || async {
        (sent(&w).len() == 2).then_some(())
    })
    .await;
    assert_eq!(sent(&w)[1], ("echo again".to_owned(), false, Some(Edits)));
    run.shutdown().await;
}

#[tokio::test]
async fn the_rework_of_a_capped_role_that_named_no_tier_is_sent_under_the_cap() {
    let w = defaulting_to_auto();
    let app = w.app_with(AppConfig {
        policy: Policy::new(
            BTreeMap::from([(Role::new("careful"), capped(Some(Edits)))]),
            Some(Role::new("careful")),
        )
        .unwrap(),
        stream_poll: Duration::from_millis(100),
        gate: orch_core::GatePolicy::requiring([orch_core::CheckSource::AgentChecks]),
        ..AppConfig::default()
    });
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "gate-push one", None).await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Working).await;
    w.agent.release_gate();
    eventually("the rework reaches the agent", || async {
        (sent(&w).len() >= 2).then_some(())
    })
    .await;
    let sends = sent(&w);
    assert_eq!(sends[0].2, Some(Edits));
    assert_eq!(
        sends[1].2,
        Some(Edits),
        "the rework is under the pinned tier, not the agent's `auto`"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn the_cap_of_a_person_is_that_of_the_roles_that_let_them_use_the_agent() {
    let w = tiered();
    let invoke = |cap: Option<AccessTier>, agents: AgentScope| RoleGrant {
        agents,
        ..capped(cap)
    };
    let read_only = RoleGrant {
        permissions: BTreeSet::from([Permission::AgentRead, Permission::ThreadRead]),
        agents: AgentScope::All,
        max_tier: None,
    };
    let app = under(
        &w,
        &[
            (
                "planner",
                invoke(Some(Plan), AgentScope::from_patterns(["plain"])),
            ),
            (
                "coder",
                invoke(Some(Auto), AgentScope::from_patterns(["coder"])),
            ),
            ("free", invoke(None, AgentScope::from_patterns(["other"]))),
            ("viewer", read_only),
        ],
    );
    let cap = |roles: &[&str], agent: &str| {
        app.access(&who(roles))
            .max_tier_for(&orch_core::AgentId::new(agent))
    };
    // a role that cannot use the agent neither lifts nor sets the cap
    assert_eq!(cap(&["planner", "free"], "plain"), Some(Plan));
    assert_eq!(cap(&["planner", "viewer"], "plain"), Some(Plan));
    assert_eq!(cap(&["planner", "coder"], "plain"), Some(Plan));
    assert_eq!(cap(&["planner", "coder"], "coder"), Some(Auto));
    assert_eq!(cap(&["planner", "free"], "other"), None);
    // the hint the web shows is over the roles that let the person use any agent
    assert_eq!(
        app.access(&who(&["planner", "viewer"])).max_tier(),
        Some(Plan)
    );
    assert_eq!(app.access(&who(&["planner", "free"])).max_tier(), None);
    assert_eq!(app.access(&who(&["viewer"])).max_tier(), None);
}

// ---- a fork's first message and an edit (ADR 0061) ------------------------------------------

/// A finished thread whose one message was sent under `tier`; the dispatcher is gone, so what a
/// fork writes stays an open row.
async fn finished(app: &Arc<TestApp>, tier: Option<AccessTier>) -> (ThreadId, i64) {
    let run = spawn_dispatcher(app, fast(), "d1");
    let thread = start(app, &alice(), "echo go", tier).await.unwrap();
    wait_state(app, &alice(), thread, ThreadState::Done).await;
    run.shutdown().await;
    let seq = events(app, &alice(), thread)
        .await
        .into_iter()
        .find(|e| e.kind() == EventKind::UserMessage)
        .unwrap()
        .seq;
    (thread, seq)
}

fn edit(seq: i64, tier: Option<AccessTier>) -> ForkRequest {
    ForkRequest {
        at: ForkAt::Replace {
            seq,
            text: "echo instead".to_owned(),
            message_id: Some("m-edit".to_owned()),
            tier,
        },
        target: None,
        id: None,
    }
}

fn first_message_after(seq: i64, tier: Option<AccessTier>) -> ForkRequest {
    ForkRequest {
        at: ForkAt::AfterTurn {
            seq,
            first: Some(FirstMessage {
                text: "echo next".to_owned(),
                message_id: None,
                run_id: None,
                origin: Origin::Agui,
                ui_catalog: None,
                mentions: Vec::new(),
                tier,
            }),
        },
        target: None,
        id: None,
    }
}

/// The tier the fork's own message (its last) is logged with, and the one its delegation carries.
async fn fork_tiers(
    w: &World,
    app: &TestApp,
    fork: ThreadId,
) -> (Option<AccessTier>, Option<AccessTier>) {
    let logged = tiers_logged(app, fork).await.pop().unwrap();
    (logged, delegated_tier(w, fork).await)
}

#[tokio::test]
async fn an_edit_that_names_no_tier_keeps_the_tier_of_the_message_it_replaces() {
    let w = tiered();
    let app = w.app();
    let (thread, seq) = finished(&app, Some(Plan)).await;
    let forked = app
        .fork_thread(&alice(), thread, edit(seq, None))
        .await
        .unwrap();
    assert_eq!(
        fork_tiers(&w, &app, forked.thread.id).await,
        (Some(Plan), Some(Plan)),
        "the edit is run as the message was, not at the agent's default"
    );
    // one that names a tier uses it
    let forked = app
        .fork_thread(&alice(), thread, edit(seq, Some(Bypass)))
        .await
        .unwrap();
    assert_eq!(
        fork_tiers(&w, &app, forked.thread.id).await,
        (Some(Bypass), Some(Bypass))
    );
}

#[tokio::test]
async fn an_edit_of_a_message_that_named_no_tier_names_none() {
    let w = tiered();
    let app = w.app();
    let (thread, seq) = finished(&app, None).await;
    let before = card_reads(&w);
    let forked = app
        .fork_thread(&alice(), thread, edit(seq, None))
        .await
        .unwrap();
    assert_eq!(fork_tiers(&w, &app, forked.thread.id).await, (None, None));
    assert_eq!(card_reads(&w), before, "plain A2A reads no card for it");
}

#[tokio::test]
async fn an_edit_is_lowered_to_the_cap_and_to_what_the_card_still_honours_never_raised() {
    let w = tiered();
    let open = w.app();
    let (thread, seq) = finished(&open, Some(Bypass)).await;
    // the person's role is capped at edits now
    let capped = capped_at(&w, Some(Edits));
    let forked = capped
        .fork_thread(&alice(), thread, edit(seq, None))
        .await
        .unwrap();
    assert_eq!(
        fork_tiers(&w, &capped, forked.thread.id).await,
        (Some(Edits), Some(Edits))
    );
    // the card lists no edits any more: the highest it does, below the choice
    w.agent
        .set_access_tier("plain", Some(offer(&["plan", "auto"])));
    let forked = capped
        .fork_thread(&alice(), thread, edit(seq, None))
        .await
        .unwrap();
    assert_eq!(
        fork_tiers(&w, &capped, forked.thread.id).await,
        (Some(Plan), Some(Plan))
    );
    // a person who names no tier and has a cap, on a message that named none, is given the cap's
    let (thread, seq) = finished(&open, None).await;
    w.agent.set_access_tier(
        "plain",
        Some(offer_defaulting(&["plan", "edits", "auto"], "auto")),
    );
    let forked = capped
        .fork_thread(&alice(), thread, edit(seq, None))
        .await
        .unwrap();
    assert_eq!(
        fork_tiers(&w, &capped, forked.thread.id).await,
        (Some(Edits), Some(Edits))
    );
}

#[tokio::test]
async fn an_edit_whose_tier_cannot_be_kept_is_refused_and_writes_nothing() {
    let w = tiered();
    let app = w.app();
    let (thread, seq) = finished(&app, Some(Edits)).await;
    let threads = threads_of(&app, &alice()).await;
    // the agent no longer lists the extension
    w.agent.set_access_tier("plain", None);
    let error = app
        .fork_thread(&alice(), thread, edit(seq, None))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, AppError::Unprocessable(m) if m.contains("no longer lists access tiers") && m.contains("start a new chat")),
        "{error:?}"
    );
    // or has none low enough
    w.agent
        .set_access_tier("plain", Some(offer(&["auto", "bypass"])));
    let error = app
        .fork_thread(&alice(), thread, edit(seq, None))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, AppError::Unprocessable(m) if m.contains("no tier at or below 'edits'")),
        "{error:?}"
    );
    // a card that is down is a 503
    w.agent.set_card_down("plain", true);
    let error = app
        .fork_thread(&alice(), thread, edit(seq, None))
        .await
        .unwrap_err();
    assert!(
        matches!(error, AppError::CardUnavailable { .. }),
        "{error:?}"
    );
    assert_eq!(
        threads_of(&app, &alice()).await,
        threads,
        "nothing was made"
    );
}

#[tokio::test]
async fn the_first_message_of_a_fork_names_its_tier_or_gets_the_cap_of_the_person() {
    let w = defaulting_to_auto();
    let open = w.app();
    let (thread, seq) = finished(&open, None).await;
    // named: checked as for a new thread
    let forked = open
        .fork_thread(&alice(), thread, first_message_after(seq, Some(Plan)))
        .await
        .unwrap();
    assert_eq!(
        fork_tiers(&w, &open, forked.thread.id).await,
        (Some(Plan), Some(Plan))
    );
    // none, no cap: plain A2A
    let forked = open
        .fork_thread(&alice(), thread, first_message_after(seq, None))
        .await
        .unwrap();
    assert_eq!(fork_tiers(&w, &open, forked.thread.id).await, (None, None));
    // none, capped: the cap, never the agent's `auto`
    let capped = capped_at(&w, Some(Edits));
    let forked = capped
        .fork_thread(&alice(), thread, first_message_after(seq, None))
        .await
        .unwrap();
    assert_eq!(
        fork_tiers(&w, &capped, forked.thread.id).await,
        (Some(Edits), Some(Edits))
    );
    // named above the cap: 422, nothing made
    let threads = threads_of(&capped, &alice()).await;
    let error = capped
        .fork_thread(&alice(), thread, first_message_after(seq, Some(Bypass)))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, AppError::Unprocessable(m) if m == "the tier 'bypass' is above what your role allows"),
        "{error:?}"
    );
    assert_eq!(threads_of(&capped, &alice()).await, threads);
}

// ---- the action of a person on a surface (ADR 0061) ----------------------------------------

fn ui_action() -> Input {
    let mut context = serde_json::Map::new();
    context.insert("choice".into(), json!("a"));
    Input::UiAction {
        tier_cap: None,
        user: alice(),
        action: UiActionData {
            tier: None,
            tier_cap: None,
            surface_id: "s1".into(),
            name: "go".into(),
            source_component_id: "go".into(),
            context,
            version: UiVersion::V0_9_1,
            run_id: Some("run-2".into()),
        },
        catalog: None,
        tier: None,
    }
}

#[tokio::test]
async fn an_action_continues_the_tier_of_the_job_lowered_to_the_cap_and_the_card() {
    let w = tiered();
    let open = w.app();
    let run = spawn_dispatcher(&open, fast(), "d1");
    let thread = start(&open, &alice(), "ui pick one", Some(Bypass))
        .await
        .unwrap();
    wait_state(&open, &alice(), thread, ThreadState::Blocked).await;
    // the person's role is capped at edits since the choice was made: the action does not go above
    let capped = capped_at(&w, Some(Edits));
    capped
        .submit(&alice(), thread, ui_action(), None)
        .await
        .unwrap();
    wait_state(&open, &alice(), thread, ThreadState::Done).await;
    let actions: Vec<_> = w
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send {
                action: Some(_),
                tier,
                ..
            } => Some(tier),
            _ => None,
        })
        .collect();
    assert_eq!(
        actions,
        [Some(Edits)],
        "the action goes under the lower of the two"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn an_action_the_cap_or_the_card_cannot_keep_the_tier_for_is_refused_and_writes_nothing() {
    let w = tiered();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, &alice(), "ui pick one", Some(Edits))
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Blocked).await;
    let before = events(&app, &alice(), thread).await.len();
    w.agent.set_access_tier("plain", None);
    let error = app
        .submit(&alice(), thread, ui_action(), None)
        .await
        .unwrap_err();
    assert!(matches!(error, AppError::Unprocessable(_)), "{error:?}");
    w.agent.set_card_down("plain", true);
    let error = app
        .submit(&alice(), thread, ui_action(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(error, AppError::CardUnavailable { .. }),
        "{error:?}"
    );
    assert_eq!(events(&app, &alice(), thread).await.len(), before);
    run.shutdown().await;
}
