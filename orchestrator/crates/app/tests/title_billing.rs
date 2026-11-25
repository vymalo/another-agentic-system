//! The orchestrator's own title call takes the path of an agent's: through the deployment's
//! model-credential injector, for the person whose thread it is (sys#338 slice 4, card ANR-670).
//!
//! A title is asked outside any run, by the outbox worker, so the worker binds the call's lease
//! itself — its id is `thread:<thread id>:title`, the thread's agent, the thread's **owner** as the
//! person, and the title task's endpoint as the model — and sends that lease id with the request.
//! The injector resolves the lease by it and stamps `x-account-id` with the lease's account, which
//! is the person's `UserId` (ADR 0065, ADR 0084): the account of a title is the thread's owner,
//! never a constant and never the person who asked for the title.
//!
//! See [`support::billing`] for the injector and the broker the worker meets.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;
use std::sync::Arc;

use orch_core::{AgentId, ThreadState};
use orch_ports::memory::MemoryRunLeases;
use orch_ports::{OutboxKind, OutboxStatus, RunCaller, RunId, RunLeases, ToolSecret};
use support::billing::{
    Broker, ENDPOINT, Injector, LEASE_TOKEN, TestApp, app_with, create, quiet, spawn, wait_state,
};
use support::{World, alice, bob, eventually};

/// The lease id a title call of `thread` is made under.
fn title_lease(thread: orch_core::ThreadId) -> String {
    format!("thread:{thread}:title")
}

async fn title_of(app: &TestApp, id: orch_core::ThreadId) -> String {
    app.get_thread(&alice(), id).await.unwrap().title
}

/// The status of the thread's one `title` outbox row: the fact that *that* row was worked.
fn title_row(w: &World, id: orch_core::ThreadId) -> OutboxStatus {
    let rows: Vec<_> = w
        .store
        .outbox_of(id)
        .into_iter()
        .filter(|row| row.kind == OutboxKind::Title)
        .collect();
    assert_eq!(rows.len(), 1, "one title row: {rows:?}");
    rows[0].status
}

#[tokio::test]
async fn a_title_call_names_the_thread_owners_account_through_the_injector() {
    let w = World::new();
    let leases = Arc::new(MemoryRunLeases::with_token(
        ENDPOINT,
        ToolSecret::new(LEASE_TOKEN),
    ));
    let injector = Injector::new(w.model.clone(), Arc::clone(&leases));
    let app = app_with(&w, injector.clone());
    let broker = Broker::new(Arc::clone(&leases));
    let run = spawn(&app, broker.clone());

    // Alice's thread: the model answers, and the title lands in her thread.
    w.model.then_answer("Which branch to use");
    let a = create(&app, &alice(), "plain", "ask me about the build").await;
    wait_state(&app, &alice(), a.id, ThreadState::Blocked).await;
    eventually("alice's title", || async {
        (title_of(&app, a.id).await != "ask me about the build").then_some(())
    })
    .await;
    quiet(&w, a.id).await;
    assert_eq!(title_of(&app, a.id).await, "Which branch to use");

    // Bob's thread through the same model: a second person, a second lease.
    w.model.then_answer("Bob's build");
    let b = create(&app, &bob(), "plain", "bob asks about the build").await;
    wait_state(&app, &bob(), b.id, ThreadState::Blocked).await;
    eventually("bob's title", || async {
        (app.get_thread(&bob(), b.id).await.unwrap().title != "bob asks about the build")
            .then_some(())
    })
    .await;
    quiet(&w, b.id).await;
    run.shutdown().await;

    // Two questions reached the model, each with the lease the worker bound for its thread.
    let calls = w.model.calls();
    assert_eq!(calls.len(), 2, "one title question each: {calls:?}");
    let binds = broker.binds();
    assert_eq!(binds.len(), 2, "one binding per title call");
    let mut leases_seen = BTreeSet::new();
    for (call, (owner, thread)) in calls.iter().zip([(alice(), a.id), (bob(), b.id)]) {
        let lease = call
            .run
            .clone()
            .expect("the title call names the lease the injector resolves");
        assert_eq!(
            lease.as_str(),
            title_lease(thread),
            "the lease id names the thread and what is asked of it"
        );
        leases_seen.insert(lease.clone());
        let binding = binds
            .iter()
            .find(|binding| binding.run == lease)
            .unwrap_or_else(|| panic!("the lease {lease} was never bound: {binds:?}"));
        assert_eq!(
            binding.person, owner,
            "the binding spends for the thread's owner, not a constant"
        );
        assert_eq!(
            binding.agent,
            AgentId::new("plain"),
            "the thread's own agent"
        );
        assert_eq!(
            binding.model.as_deref(),
            Some(ENDPOINT),
            "the lease is for the title task's endpoint"
        );
        // What the injector stamps as `x-account-id`: the lease's account, the person's UserId.
        let lease = leases
            .lease(&lease, &RunCaller::Injector)
            .await
            .expect("the injector can read the lease of the call");
        assert_eq!(lease.account(), &owner, "the account is the thread owner's");
        assert_eq!(lease.token().expose(), LEASE_TOKEN, "the injected key");
    }
    assert_eq!(leases_seen.len(), 2, "two threads, two leases");
    assert_eq!(injector.stamped(), [alice(), bob()]);
    let mut unbound = broker.unbinds();
    unbound.sort();
    let mut want: Vec<RunId> = leases_seen.into_iter().collect();
    want.sort();
    assert_eq!(unbound, want, "every title lease is unbound after its call");
}

#[tokio::test]
async fn a_person_at_zero_gets_no_title_call_and_the_default_title_stands() {
    let w = World::new();
    let leases = Arc::new(MemoryRunLeases::with_token(
        ENDPOINT,
        ToolSecret::new(LEASE_TOKEN),
    ));
    // Alice has nothing left to spend: the injector answers 402 before the model is reached.
    let injector = Injector::new(w.model.clone(), Arc::clone(&leases)).with_zero(alice());
    let app = app_with(&w, injector.clone());
    let run = spawn(&app, Broker::new(Arc::clone(&leases)));

    w.model.then_answer("Never asked");
    let t = create(&app, &alice(), "plain", "stream hello").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    quiet(&w, t.id).await;
    run.shutdown().await;

    // 1. The work happened: the thread's own title row was worked and ended (`TitleDeclined`),
    //    so `0` below cannot pass because the worker never reached the row.
    assert_eq!(
        title_row(&w, t.id),
        OutboxStatus::Delivered,
        "the title row was never worked"
    );
    // 2. The title call counter is `0`: the 402 is the injector's, before the model endpoint.
    assert!(
        w.model.calls().is_empty(),
        "a person at zero reached the model: {:?}",
        w.model.calls()
    );
    // ... and the attempt reached the injector, for this person's account, once and no more
    // (a 402 is refused for good, not retried).
    assert_eq!(injector.asked(), 1, "one attempt, refused for good");
    assert_eq!(
        injector.stamped(),
        [alice()],
        "the balance was read for the thread owner"
    );
    // 3. The surviving value is the default the thread was created with.
    assert_eq!(
        title_of(&app, t.id).await,
        "stream hello",
        "the default title is the first line of the first message"
    );
    let got = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(got.job.title.source(), orch_core::TitleSource::FirstMessage);
    // 4. And the thread stays usable: the person renames it.
    let renamed = app
        .rename_thread(&alice(), t.id, "Mine, at zero")
        .await
        .unwrap();
    assert_eq!(renamed.title, "Mine, at zero");
}
