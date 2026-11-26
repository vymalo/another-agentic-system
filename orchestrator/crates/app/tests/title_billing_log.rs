//! The account id of a title call is in no log line (card ANR-670): the injector stamps the thread
//! owner's `UserId` as `x-account-id`, the worker names the lease and the thread and never the
//! person. In a test binary of its own, because it captures the logs of the process: another
//! test's callsite caches would otherwise decide what is recorded.
//!
//! The subscriber is written here rather than taken from `tracing-subscriber` so that this crate
//! gains no dependency for one test: it keeps the message of every event and nothing else.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::{Arc, Mutex, PoisonError};

use orch_core::ThreadState;
use orch_ports::ToolSecret;
use orch_ports::memory::MemoryRunLeases;
use support::billing::{
    Broker, ENDPOINT, Injector, LEASE_TOKEN, app_with, create, quiet, spawn, wait_state,
};
use support::{World, alice, eventually};

/// Every log line's message, as the process emits it.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<String>>);

impl Captured {
    fn text(&self) -> String {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The event's fields as `name=value`, which includes its `message`.
struct Fields(String);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write as _;
        let _ = write!(self.0, " {}={value:?}", field.name());
    }
}

impl tracing::Subscriber for Captured {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        let mut log = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        log.push_str(event.metadata().name());
        log.push_str(&fields.0);
        log.push('\n');
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

#[tokio::test]
async fn no_line_of_a_title_call_holds_the_account_id() {
    let captured = Captured::default();
    tracing::subscriber::set_global_default(captured.clone()).unwrap();

    let alice = alice();
    let w = World::new();
    let leases = Arc::new(MemoryRunLeases::with_token(
        ENDPOINT,
        ToolSecret::new(LEASE_TOKEN),
    ));
    let injector = Injector::new(w.model.clone(), Arc::clone(&leases));
    let app = app_with(&w, injector.clone());
    let run = spawn(&app, Broker::new(Arc::clone(&leases)));

    w.model.then_answer("Which branch to use");
    let t = create(&app, &alice, "plain", "ask me about the build").await;
    wait_state(&app, &alice, t.id, ThreadState::Blocked).await;
    eventually("the title", || async {
        (app.get_thread(&alice, t.id).await.unwrap().title != "ask me about the build")
            .then_some(())
    })
    .await;
    quiet(&w, t.id).await;
    run.shutdown().await;

    // The call happened, and the injector read the person's account: the log is not empty.
    assert_eq!(injector.stamped().as_slice(), std::slice::from_ref(&alice));
    let log = captured.text();
    assert!(
        log.contains("the utility call's lease is bound"),
        "the title never bound its lease, so this proves nothing: {log}"
    );
    assert!(
        !log.contains(&alice.to_string()),
        "the account id is in a log line: {log}"
    );
}
