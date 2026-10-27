//! The usage record of a person (ADR 0065): what a usage event changes in the ledger and what a row
//! becomes when it is applied. A total replaces, a call adds only while the task has no total, an
//! event applied twice is applied once, the month is the UTC month of the latest change.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use jiff::Timestamp;
use orch_core::*;
use uuid::Uuid;

fn owner() -> UserId {
    UserId::new("alice@example.com")
}

fn thread() -> ThreadId {
    ThreadId(Uuid::from_u128(0x0190_0000_0000_7000_8000_0000_0000_0001))
}

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn counts(input: u64, output: u64) -> TokenCounts {
    TokenCounts {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        ..TokenCounts::default()
    }
}

fn line(provider: Option<&str>, model: &str, tokens: TokenCounts) -> ModelTokens {
    ModelTokens {
        provider: provider.map(Into::into),
        model: model.into(),
        tokens,
    }
}

fn call_event(
    seq: i64,
    time: &str,
    agent: &str,
    task: &str,
    id: &str,
    tokens: TokenCounts,
) -> Event {
    Event {
        seq,
        thread_id: thread(),
        at: at(time),
        actor: Actor::agent(&AgentId::new(agent), None),
        body: EventBody::ModelUsage(Box::new(ModelUsageData {
            job: 1,
            agent: agent.into(),
            task: task.into(),
            call: id.into(),
            path: Vec::new(),
            provider: Some("openai".into()),
            model: "glm".into(),
            tokens,
            context_window: None,
        })),
    }
}

fn total_event(
    seq: i64,
    time: &str,
    agent: &str,
    task: &str,
    totals: Vec<ModelTokens>,
    sandbox: Option<SandboxUsage>,
) -> Event {
    Event {
        seq,
        thread_id: thread(),
        at: at(time),
        actor: Actor::agent(&AgentId::new(agent), None),
        body: EventBody::ModelUsageTotal(ModelUsageTotalData {
            job: 1,
            agent: agent.into(),
            task: task.into(),
            path: Vec::new(),
            totals,
            sandbox,
        }),
    }
}

fn change(event: &Event) -> UsageChange {
    usage_change(&owner(), event).expect("a usage event")
}

/// Applies `events` in order to one task's row, as a store does.
fn fold(events: &[Event]) -> Option<UsageRow> {
    let mut row: Option<UsageRow> = None;
    for event in events {
        if let Some(next) = UsageRow::fold(row.as_ref(), &change(event)) {
            row = Some(next);
        }
    }
    row
}

const JAN: &str = "2026-01-15T10:00:00Z";

#[test]
fn only_the_two_usage_events_change_the_ledger() {
    let message = Event {
        seq: 1,
        thread_id: thread(),
        at: at(JAN),
        actor: Actor::user(&owner()),
        body: EventBody::UserMessage(UserMessageData {
            text: "hi".into(),
            message_id: None,
            run_id: None,
            origin: Origin::Agui,
            delivery: None,
            mentions: Vec::new(),
            tier: None,
            tier_cap: None,
        }),
    };
    assert_eq!(usage_change(&owner(), &message), None);
    assert!(
        usage_change(
            &owner(),
            &call_event(2, JAN, "coder", "t1", "c1", counts(1, 1))
        )
        .is_some()
    );
    assert!(
        usage_change(
            &owner(),
            &total_event(3, JAN, "coder", "t1", Vec::new(), None)
        )
        .is_some()
    );
}

#[test]
fn a_change_names_the_person_the_task_the_event_and_the_month() {
    let event = call_event(
        7,
        "2026-02-01T00:00:00Z",
        "coder",
        "t1",
        "c1",
        counts(10, 2),
    );
    let change = change(&event);
    assert_eq!(change.owner, owner());
    assert_eq!(
        change.key,
        UsageTaskKey {
            thread: thread(),
            agent: "coder".into(),
            task: "t1".into()
        }
    );
    assert_eq!(change.seq, 7);
    assert_eq!(change.month.to_string(), "2026-02");
    assert_eq!(
        change.what,
        UsageWhat::Call(line(Some("openai"), "glm", counts(10, 2)))
    );
}

#[test]
fn calls_add_up_while_the_task_has_no_total() {
    let row = fold(&[
        call_event(1, JAN, "coder", "t1", "c1", counts(10, 2)),
        call_event(2, JAN, "coder", "t1", "c2", counts(20, 3)),
    ])
    .unwrap();
    assert_eq!(row.source, UsageSource::Calls);
    assert_eq!(row.seq, 2);
    assert_eq!(row.tokens, vec![line(Some("openai"), "glm", counts(30, 5))]);
    assert_eq!(row.sandbox, None);
}

#[test]
fn a_total_replaces_the_calls_and_never_sits_on_top_of_them() {
    let row = fold(&[
        call_event(1, JAN, "coder", "t1", "c1", counts(10, 2)),
        call_event(2, JAN, "coder", "t1", "c2", counts(20, 3)),
        total_event(
            3,
            JAN,
            "coder",
            "t1",
            vec![line(Some("openai"), "glm", counts(25, 4))],
            None,
        ),
    ])
    .unwrap();
    assert_eq!(row.source, UsageSource::Total);
    // lower than the calls' sum: the agent left out a retried attempt, and the total is the record
    assert_eq!(row.tokens, vec![line(Some("openai"), "glm", counts(25, 4))]);
}

#[test]
fn a_later_total_replaces_an_earlier_one_even_when_it_is_lower() {
    let row = fold(&[
        total_event(
            1,
            JAN,
            "coder",
            "t1",
            vec![line(None, "m", counts(100, 10))],
            None,
        ),
        total_event(
            2,
            JAN,
            "coder",
            "t1",
            vec![line(None, "m", counts(90, 9))],
            None,
        ),
    ])
    .unwrap();
    assert_eq!(row.tokens, vec![line(None, "m", counts(90, 9))]);
    assert_eq!(row.seq, 2);
}

#[test]
fn a_call_after_a_total_changes_nothing() {
    let events = [
        total_event(
            1,
            JAN,
            "coder",
            "t1",
            vec![line(None, "m", counts(100, 10))],
            None,
        ),
        call_event(
            2,
            "2026-03-01T00:00:00Z",
            "coder",
            "t1",
            "late",
            counts(5, 5),
        ),
    ];
    let row = fold(&events[..1]).unwrap();
    assert_eq!(UsageRow::fold(Some(&row), &change(&events[1])), None);
    // nor the month: the row stays in the month of the total
    assert_eq!(row.month.to_string(), "2026-01");
}

#[test]
fn an_event_applied_twice_is_applied_once() {
    let events = [
        call_event(1, JAN, "coder", "t1", "c1", counts(10, 2)),
        call_event(2, JAN, "coder", "t1", "c2", counts(20, 3)),
        total_event(
            3,
            JAN,
            "coder",
            "t1",
            vec![line(None, "m", counts(25, 4))],
            None,
        ),
    ];
    let once = fold(&events).unwrap();
    for event in &events {
        assert_eq!(
            UsageRow::fold(Some(&once), &change(event)),
            None,
            "seq {} is folded in already",
            event.seq
        );
    }
    // a whole replay, from the start, ends where the first pass did
    let mut row: Option<UsageRow> = None;
    for event in events.iter().chain(&events) {
        if let Some(next) = UsageRow::fold(row.as_ref(), &change(event)) {
            row = Some(next);
        }
    }
    assert_eq!(row.unwrap(), once);
}

#[test]
fn a_call_replayed_while_the_task_has_no_total_is_not_added_twice() {
    let first = call_event(1, JAN, "coder", "t1", "c1", counts(10, 2));
    let row = fold(std::slice::from_ref(&first)).unwrap();
    assert_eq!(UsageRow::fold(Some(&row), &change(&first)), None);
    let second = call_event(2, JAN, "coder", "t1", "c2", counts(20, 3));
    let row = UsageRow::fold(Some(&row), &change(&second)).unwrap();
    assert_eq!(UsageRow::fold(Some(&row), &change(&first)), None);
    assert_eq!(UsageRow::fold(Some(&row), &change(&second)), None);
    assert_eq!(row.tokens, vec![line(Some("openai"), "glm", counts(30, 5))]);
}

#[test]
fn the_agents_and_the_tasks_are_apart() {
    let a = change(&call_event(1, JAN, "coder", "t1", "c1", counts(1, 1))).key;
    let other_task = change(&call_event(1, JAN, "coder", "t2", "c1", counts(1, 1))).key;
    // an asked agent that numbers its tasks as the thread's agent does is another row
    let asked = change(&call_event(1, JAN, "researcher", "t1", "c1", counts(1, 1))).key;
    assert_ne!(a, other_task);
    assert_ne!(a, asked);
}

#[test]
fn the_month_is_the_utc_month_of_the_latest_event_that_changed_the_row() {
    let row = fold(&[
        call_event(
            1,
            "2026-01-31T23:59:59Z",
            "coder",
            "t1",
            "c1",
            counts(10, 2),
        ),
        call_event(
            2,
            "2026-02-01T00:00:00Z",
            "coder",
            "t1",
            "c2",
            counts(20, 3),
        ),
    ])
    .unwrap();
    assert_eq!(row.month.to_string(), "2026-02");
    // the whole row moves with it: the earlier call is counted in February too
    assert_eq!(row.tokens, vec![line(Some("openai"), "glm", counts(30, 5))]);
    // an offset is read as the instant it names, in UTC
    let late = call_event(
        1,
        "2026-01-31T23:30:00-02:00",
        "coder",
        "t1",
        "c1",
        counts(1, 1),
    );
    assert_eq!(
        UsageRow::fold(None, &change(&late))
            .unwrap()
            .month
            .to_string(),
        "2026-02"
    );
}

#[test]
fn models_are_kept_apart_and_ordered_by_provider_then_model() {
    let mut a = call_event(1, JAN, "coder", "t1", "c1", counts(1, 1));
    let EventBody::ModelUsage(d) = &mut a.body else {
        unreachable!()
    };
    d.provider = None;
    d.model = "small".into();
    let b = call_event(2, JAN, "coder", "t1", "c2", counts(2, 2));
    let c = call_event(3, JAN, "coder", "t1", "c3", counts(4, 4));
    let row = fold(&[a, b, c]).unwrap();
    assert_eq!(
        row.tokens,
        vec![
            line(None, "small", counts(1, 1)),
            line(Some("openai"), "glm", counts(6, 6)),
        ]
    );
}

#[test]
fn the_parts_of_the_counts_are_summed_and_stay_unknown_until_one_says_them() {
    let mut with_parts = counts(100, 50);
    with_parts.reasoning_tokens = Some(20);
    with_parts.cached_input_tokens = Some(60);
    let row = fold(&[
        call_event(1, JAN, "coder", "t1", "c1", counts(10, 5)),
        call_event(2, JAN, "coder", "t1", "c2", with_parts),
        call_event(3, JAN, "coder", "t1", "c3", with_parts),
    ])
    .unwrap();
    let t = row.tokens[0].tokens;
    assert_eq!(t.input_tokens, 210);
    assert_eq!(t.output_tokens, 105);
    assert_eq!(t.total_tokens, 315);
    assert_eq!(t.reasoning_tokens, Some(40));
    assert_eq!(t.cached_input_tokens, Some(120));
    assert_eq!(t.cache_write_input_tokens, None);
}

#[test]
fn counts_stop_at_the_largest_bigint_and_a_row_never_fails() {
    let big = counts(MAX_TOKEN_COUNT, MAX_TOKEN_COUNT);
    let mut row = None;
    for seq in 1..=2_000 {
        let event = call_event(seq, JAN, "coder", "t1", &format!("c{seq}"), big);
        if let Some(next) = UsageRow::fold(row.as_ref(), &change(&event)) {
            row = Some(next);
        }
    }
    let t = row.unwrap().tokens[0].tokens;
    assert_eq!(t.input_tokens, MAX_LEDGER_COUNT);
    assert_eq!(t.output_tokens, MAX_LEDGER_COUNT);
    assert_eq!(t.total_tokens, MAX_LEDGER_COUNT);
}

#[test]
fn a_total_that_names_a_model_twice_is_summed_so_a_store_never_meets_two_lines_of_one_key() {
    // `UsageTotals::check` refuses it, so the log holds none; the ledger still must not fail on one
    let row = fold(&[total_event(
        1,
        JAN,
        "coder",
        "t1",
        vec![line(None, "m", counts(1, 1)), line(None, "m", counts(2, 2))],
        None,
    )])
    .unwrap();
    assert_eq!(row.tokens, vec![line(None, "m", counts(3, 3))]);
}

#[test]
fn the_sandbox_of_the_latest_total_is_the_rows_and_a_total_without_one_clears_it() {
    let sandbox = SandboxUsage {
        environment: SandboxEnvironment::Kubernetes,
        usage: Some(vec![SandboxEntry {
            cpu_millis: Some(250),
            memory_bytes: Some(536_870_912),
            commands: 12,
            milliseconds: 81_234,
        }]),
    };
    let with = total_event(1, JAN, "coder", "t1", Vec::new(), Some(sandbox.clone()));
    let without = total_event(2, JAN, "coder", "t1", Vec::new(), None);
    assert_eq!(
        fold(std::slice::from_ref(&with)).unwrap().sandbox,
        Some(sandbox)
    );
    assert_eq!(fold(&[with, without]).unwrap().sandbox, None);
}

#[test]
fn a_month_is_written_yyyy_mm_and_nothing_else_is_one() {
    for (text, year, month) in [
        ("2026-10", 2026, 10),
        ("0001-01", 1, 1),
        ("9999-12", 9999, 12),
    ] {
        let parsed: UsageMonth = text.parse().unwrap();
        assert_eq!((parsed.year(), parsed.month()), (year, month));
        assert_eq!(parsed.to_string(), text);
    }
    for bad in [
        "",
        "2026",
        "2026-1",
        "2026-13",
        "2026-00",
        "0000-01",
        "2026/10",
        "+026-10",
        "2026-10 ",
        " 2026-10",
        "2026-1x",
        "２０２６-10",
        "2026-10-01",
    ] {
        assert!(bad.parse::<UsageMonth>().is_err(), "{bad:?}");
    }
    assert!(UsageMonth::new(2026, 12).unwrap() > UsageMonth::new(2026, 2).unwrap());
    assert_eq!(
        serde_json::to_string(&UsageMonth::new(2026, 3).unwrap()).unwrap(),
        "\"2026-03\""
    );
    assert!(serde_json::from_str::<UsageMonth>("\"2026-3\"").is_err());
}

#[test]
fn the_totals_of_a_month_sum_the_rows_per_model_and_per_sandbox_shape() {
    let shape = |cpu, memory, commands, milliseconds| SandboxEntry {
        cpu_millis: cpu,
        memory_bytes: memory,
        commands,
        milliseconds,
    };
    let kube = |entries| SandboxUsage {
        environment: SandboxEnvironment::Kubernetes,
        usage: Some(entries),
    };
    let local = SandboxUsage {
        environment: SandboxEnvironment::Local,
        usage: None,
    };
    let row = |task: &str, tokens: Vec<ModelTokens>, sandbox: Option<SandboxUsage>| {
        fold(&[total_event(1, JAN, "coder", task, tokens, sandbox)]).unwrap()
    };
    let rows = [
        row(
            "t1",
            vec![
                line(Some("openai"), "glm", counts(100, 10)),
                line(None, "small", counts(1, 1)),
            ],
            Some(kube(vec![
                shape(Some(250), Some(1024), 2, 1000),
                shape(None, None, 1, 5),
            ])),
        ),
        row(
            "t2",
            vec![line(Some("openai"), "glm", counts(50, 5))],
            Some(kube(vec![shape(Some(250), Some(1024), 3, 2000)])),
        ),
        row("t3", Vec::new(), Some(local.clone())),
        row("t4", Vec::new(), Some(local)),
        // a measured zero: a pod was asked for and no command ran
        row("t5", Vec::new(), Some(kube(Vec::new()))),
        row("t6", vec![line(None, "small", counts(2, 2))], None),
    ];
    let totals = UsageMonthTotals::of(UsageMonth::new(2026, 1).unwrap(), &rows);
    assert_eq!(totals.tasks, 6);
    assert_eq!(
        totals.models,
        vec![
            line(None, "small", counts(3, 3)),
            line(Some("openai"), "glm", counts(150, 15)),
        ]
    );
    assert_eq!(
        totals.sandbox,
        vec![
            SandboxShape {
                environment: SandboxEnvironment::Kubernetes,
                cpu_millis: None,
                memory_bytes: None,
                commands: 1,
                milliseconds: 5,
            },
            SandboxShape {
                environment: SandboxEnvironment::Kubernetes,
                cpu_millis: Some(250),
                memory_bytes: Some(1024),
                commands: 5,
                milliseconds: 3000,
            },
        ]
    );
    // not measured is not zero: it is counted apart, and a measured zero is neither
    assert_eq!(
        totals.unmeasured,
        vec![UnmeasuredSandbox {
            environment: SandboxEnvironment::Local,
            tasks: 2,
        }]
    );
    assert_eq!(
        UsageMonthTotals::of(UsageMonth::new(2026, 1).unwrap(), &[]),
        UsageMonthTotals::empty(UsageMonth::new(2026, 1).unwrap())
    );
}
