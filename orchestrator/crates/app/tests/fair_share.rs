//! A fair share of the dispatcher for each person (ADR 0066), end to end on the in-memory stack: when
//! every slot is held by one person's long jobs, the next free slot goes to the person who has none
//! running, ahead of the first person's queue; and `fifo` keeps the oldest-first order.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::DispatcherConfig;
use orch_core::{ThreadId, ThreadState};
use orch_ports::ClaimOrder;
use orch_ports::memory::Call;
use support::*;

/// The texts of the messages sent to the agent so far, in the order they were sent.
fn sent(w: &World) -> Vec<String> {
    w.agent
        .sends()
        .into_iter()
        .map(|c| match c {
            Call::Send { text, .. } => text,
            other => panic!("{other:?}"),
        })
        .collect()
}

/// Two slots, both held by alice's jobs that wait for the agent, a third job of alice queued, and
/// then a job of bob's. One of alice's jobs ends: the slot it frees goes to `order`'s choice.
/// Returns the text of the message sent next, and the threads of alice's queued job and of bob's.
async fn the_slot_that_frees(order: ClaimOrder) -> (String, ThreadId, ThreadId) {
    let w = World::new();
    let app = w.app();
    let cfg = DispatcherConfig {
        concurrency: 2,
        order,
        ..fast()
    };
    let run = spawn_dispatcher(&app, cfg, "d1");
    let a1 = create(&app, &alice(), "plain", "gate a1").await;
    let a2 = create(&app, &alice(), "plain", "gate a2").await;
    eventually("both slots to be held by alice's jobs", || async {
        (sent(&w).len() == 2).then_some(())
    })
    .await;
    let a3 = create(&app, &alice(), "plain", "gate a3").await;
    let b1 = create(&app, &bob(), "plain", "gate b1").await;
    // every slot is held, so neither queued job has started
    assert_eq!(state_of(&w, a3.id).await, ThreadState::Queued);
    assert_eq!(state_of(&w, b1.id).await, ThreadState::Queued);
    assert_eq!(sent(&w).len(), 2);

    // one of alice's jobs ends and frees a slot
    w.agent.release_gate();
    let third = eventually("the freed slot to be taken", || async {
        let texts = sent(&w);
        (texts.len() == 3).then(|| texts[2].clone())
    })
    .await;
    // the other slot is still alice's, so the queue that was not served waits for it
    assert_eq!(sent(&w).len(), 3);

    // let everything finish: a permit for each job still to end
    eventually("every job to end", || async {
        w.agent.release_gate();
        let mut done = true;
        for t in [a1.id, a2.id, a3.id, b1.id] {
            done &= state_of(&w, t).await == ThreadState::Done;
        }
        done.then_some(())
    })
    .await;
    run.shutdown().await;
    (third, a3.id, b1.id)
}

#[tokio::test]
async fn a_person_with_nothing_running_gets_the_next_free_slot_ahead_of_a_queue() {
    let (third, _, _) = the_slot_that_frees(ClaimOrder::Fair).await;
    assert_eq!(third, "gate b1", "bob's job, not alice's third");
}

#[tokio::test]
async fn fifo_gives_the_next_free_slot_to_the_oldest_row() {
    let (third, _, _) = the_slot_that_frees(ClaimOrder::Fifo).await;
    assert_eq!(third, "gate a3", "alice's third job is the older");
}

/// One person alone gets every slot: the order changes nothing for them.
#[tokio::test]
async fn one_person_alone_fills_every_slot() {
    let w = World::new();
    let app = w.app();
    let cfg = DispatcherConfig {
        concurrency: 3,
        ..fast()
    };
    let run = spawn_dispatcher(&app, cfg, "d1");
    let mut threads = Vec::new();
    for n in 1..=5 {
        threads.push(
            create(&app, &alice(), "plain", &format!("gate a{n}"))
                .await
                .id,
        );
    }
    eventually("three of alice's jobs to hold the three slots", || async {
        (sent(&w).len() == 3).then_some(())
    })
    .await;
    assert_eq!(sent(&w), ["gate a1", "gate a2", "gate a3"]);
    eventually("every job to end", || async {
        w.agent.release_gate();
        let mut done = true;
        for t in &threads {
            done &= state_of(&w, *t).await == ThreadState::Done;
        }
        done.then_some(())
    })
    .await;
    run.shutdown().await;
}
