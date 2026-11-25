//! The mission core (ADR 0095, slice 3a): the transitions of its diagram, and three properties
//! over arbitrary sequences of events.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;

use orch_core::*;
use proptest::prelude::*;

const PERSON: MissionActor = MissionActor::Person;

fn drafted() -> MissionEvent {
    MissionEvent::Drafted {
        goal: "ship the export".into(),
        limit: MissionLimit {
            tokens: 1_000,
            wall_clock_secs: 600,
            tasks_at_once: 2,
        },
        team: MissionTeam {
            members: vec![
                MissionMember {
                    name: "boss".into(),
                    agent: "lead-agent".into(),
                    role: "lead".into(),
                },
                MissionMember {
                    name: "dev".into(),
                    agent: "coder".into(),
                    role: "writer".into(),
                },
            ],
            lead: "boss".into(),
        },
        done: vec![
            DoneCheck::Ci {
                repo: "app".into(),
                checks: vec!["build".into()],
            },
            DoneCheck::Verdict {
                agent: "reviewer".into(),
            },
        ],
    }
}

fn proposed(version: u32, digest: &str) -> MissionEvent {
    MissionEvent::PlanProposed {
        version,
        digest: digest.into(),
    }
}

fn approved(version: u32, digest: &str, by: MissionActor) -> MissionEvent {
    MissionEvent::PlanApproved {
        version,
        digest: digest.into(),
        by,
    }
}

fn report(check: usize, tip: &str, passed: bool) -> MissionEvent {
    MissionEvent::CheckReported {
        check,
        tip: tip.into(),
        passed,
    }
}

fn stop_by_person() -> MissionEvent {
    MissionEvent::Stopped {
        reason: StopReason::Person,
        by: PERSON,
    }
}

fn running_prefix() -> Vec<MissionEvent> {
    vec![
        drafted(),
        proposed(1, "d1"),
        approved(1, "d1", PERSON),
        MissionEvent::Started { by: PERSON },
    ]
}

fn at(state: MissionState) -> Mission {
    let mut events = match state {
        MissionState::Drafted => vec![drafted(), proposed(1, "d1")],
        MissionState::Planned => running_prefix()[..3].to_vec(),
        _ => running_prefix(),
    };
    match state {
        MissionState::Waiting => events.push(MissionEvent::Waiting {
            reason: WaitReason::Question,
        }),
        MissionState::Paused => events.push(MissionEvent::Paused { by: PERSON }),
        MissionState::Stopped => events.push(stop_by_person()),
        MissionState::Done => {
            events.push(report(0, "t1", true));
            events.push(report(1, "t1", true));
            events.push(MissionEvent::Done { tip: "t1".into() });
        }
        _ => {}
    }
    let m = Mission::replay(&events).unwrap();
    assert_eq!(m.state(), state);
    m
}

/// Every event the properties draw from: right and wrong actors, versions, tips and checks.
fn alphabet() -> Vec<MissionEvent> {
    use MissionActor::{Core, Lead};
    let mut v = vec![
        drafted(),
        proposed(1, "d1"),
        proposed(2, "d2"),
        approved(1, "d1", PERSON),
        approved(1, "d1", Lead),
        approved(1, "d1", Core),
        approved(2, "d2", PERSON),
        approved(1, "other", PERSON),
        MissionEvent::SpendObserved {
            tokens: 1,
            wall_clock_secs: 1,
        },
        MissionEvent::SpendObserved {
            tokens: 1_000,
            wall_clock_secs: 1,
        },
        MissionEvent::Waiting {
            reason: WaitReason::LimitParked,
        },
        MissionEvent::Stopped {
            reason: StopReason::LimitSpent,
            by: Core,
        },
        MissionEvent::Stopped {
            reason: StopReason::LimitSpent,
            by: PERSON,
        },
        MissionEvent::Stopped {
            reason: StopReason::Person,
            by: Lead,
        },
        stop_by_person(),
    ];
    for by in [PERSON, Lead, Core] {
        v.push(MissionEvent::Started { by });
        v.push(MissionEvent::Resumed { by });
        v.push(MissionEvent::Paused { by });
    }
    for check in [0, 1, 5] {
        for tip in ["t1", "t2"] {
            for passed in [true, false] {
                v.push(report(check, tip, passed));
            }
        }
    }
    v.push(MissionEvent::Done { tip: "t1".into() });
    v.push(MissionEvent::Done { tip: "t2".into() });
    v
}

fn arb_event() -> impl Strategy<Value = MissionEvent> {
    proptest::sample::select(alphabet())
}

fn arb_events() -> impl Strategy<Value = Vec<MissionEvent>> {
    proptest::collection::vec(arb_event(), 0..40)
}

/// What a run through a sequence showed, read from the accepted events only.
struct Trace {
    mission: Mission,
    person_approved: bool,
    person_started: bool,
    latest: BTreeMap<usize, (String, bool)>,
}

/// Applies each event, calling `after` on every step with the event and whether it was taken.
fn run(
    prefix: &[MissionEvent],
    suffix: &[MissionEvent],
    mut after: impl FnMut(&Trace, &MissionEvent, bool, &Mission),
) {
    let mut trace = Trace {
        mission: Mission::from_event(&prefix[0]).unwrap(),
        person_approved: false,
        person_started: false,
        latest: BTreeMap::new(),
    };
    for event in prefix[1..].iter().chain(suffix) {
        let before = trace.mission.clone();
        let taken = trace.mission.apply(event).is_ok();
        if !taken {
            assert_eq!(trace.mission, before, "a refused event changed the mission");
        } else {
            match event {
                MissionEvent::PlanApproved { by, .. } if *by == PERSON => {
                    trace.person_approved = true;
                }
                MissionEvent::Started { by } if *by == PERSON => trace.person_started = true,
                MissionEvent::CheckReported { check, tip, passed } => {
                    trace.latest.insert(*check, (tip.clone(), *passed));
                }
                _ => {}
            }
        }
        after(&trace, event, taken, &before);
    }
}

fn prefixes() -> Vec<Vec<MissionEvent>> {
    let p = running_prefix();
    vec![vec![drafted()], p[..2].to_vec(), p[..3].to_vec(), p]
}

proptest! {
    /// (1) Done only after a passing check of the definition of done, on the tip it names.
    ///
    /// Deleting `if !self.proven_on(tip) { return Err(MissionRefusal::DoneNotProven); }` from the
    /// `MissionEvent::Done` arm of `Mission::apply` makes this fail: the running prefix and a
    /// `Done` with no report is then taken.
    #[test]
    fn done_needs_every_check_passed_on_its_tip(
        which in 0usize..4,
        suffix in arb_events(),
    ) {
        let prefix = prefixes()[which].clone();
        run(&prefix, &suffix, |trace, event, taken, _| {
            if let (true, MissionEvent::Done { tip }) = (taken, event) {
                let n = trace.mission.done_checks().len();
                for i in 0..n {
                    let ok = matches!(trace.latest.get(&i), Some((t, true)) if t == tip);
                    assert!(ok, "check {i} had not passed on {tip}");
                }
            }
            if trace.mission.state() == MissionState::Done {
                assert!(trace.latest.values().any(|(_, passed)| *passed));
                assert!(trace.mission.done_tip().is_some());
            }
        });
    }

    /// (2) Stopped is terminal: nothing leaves it, and a refused event changes nothing.
    #[test]
    fn stopped_and_done_are_terminal(
        which in 0usize..4,
        suffix in arb_events(),
        tail in arb_events(),
    ) {
        let prefix = prefixes()[which].clone();
        let mut frozen: Option<MissionState> = None;
        let mut last: Option<Mission> = None;
        run(&prefix, &suffix, |trace, _, _, _| {
            let state = trace.mission.state();
            if let Some(f) = frozen {
                assert_eq!(state, f, "a terminal state was left");
            } else if state.is_terminal() {
                frozen = Some(state);
            }
            last = Some(trace.mission.clone());
        });
        if let Some(mut m) = last
            && m.state().is_terminal()
        {
            let state = m.state();
            for event in &tail {
                assert!(m.apply(event).is_err());
                assert_eq!(m.state(), state);
            }
        }
    }

    /// (3) A plan runs only after the person approved it: no Running without an accepted
    /// approval by the person of the proposed plan and a start by the person.
    #[test]
    fn running_needs_the_persons_approval(
        which in 0usize..4,
        suffix in arb_events(),
    ) {
        let prefix = prefixes()[which].clone();
        run(&prefix, &suffix, |trace, _, _, _| {
            let m = &trace.mission;
            if matches!(
                m.state(),
                MissionState::Planned
                    | MissionState::Running
                    | MissionState::Waiting
                    | MissionState::Paused
                    | MissionState::Done
            ) {
                assert!(trace.person_approved && m.approved_plan().is_some());
            }
            if matches!(
                m.state(),
                MissionState::Running | MissionState::Waiting | MissionState::Paused | MissionState::Done
            ) {
                assert!(trace.person_started);
            }
        });
    }
}

#[test]
fn the_diagram_and_nothing_else() {
    use MissionState::*;
    let events = alphabet();
    let states = [Drafted, Planned, Running, Waiting, Paused, Done, Stopped];
    // The state each (state, event) leads to, written from the ADR's diagram; the rest is refused.
    let expect = |s: MissionState, e: &MissionEvent| -> Option<MissionState> {
        use MissionActor::{Core, Person};
        match (s, e) {
            (Drafted, MissionEvent::PlanProposed { .. }) => Some(Drafted),
            (
                Drafted,
                MissionEvent::PlanApproved {
                    version: 1,
                    digest,
                    by: Person,
                },
            ) if digest == "d1" => Some(Planned),
            (Planned, MissionEvent::Started { by: Person }) => Some(Running),
            (Running, MissionEvent::Waiting { .. }) => Some(Waiting),
            (Waiting, MissionEvent::Resumed { by: Person | Core }) => Some(Running),
            (Paused, MissionEvent::Resumed { by: Person }) => Some(Running),
            (Running | Waiting, MissionEvent::Paused { by: Person }) => Some(Paused),
            (
                Drafted | Planned | Running | Waiting | Paused,
                MissionEvent::Stopped {
                    reason: StopReason::Person,
                    by: Person,
                },
            ) => Some(Stopped),
            (_, MissionEvent::SpendObserved { .. }) if !matches!(s, Done | Stopped) => Some(s),
            (Running, MissionEvent::CheckReported { check: 0 | 1, .. }) => Some(Running),
            _ => None,
        }
    };
    for s in states {
        for e in &events {
            let mut m = at(s);
            let want = expect(s, e);
            let got = m.apply(e).ok().map(|()| m.state());
            assert_eq!(got, want, "{s:?} then {}", e.name());
        }
    }
}

#[test]
fn the_unproven_exits_are_refused() {
    let mut m = at(MissionState::Running);
    assert_eq!(
        m.apply(&MissionEvent::Done { tip: "t1".into() }),
        Err(MissionRefusal::DoneNotProven)
    );
    m.apply(&report(0, "t1", true)).unwrap();
    m.apply(&report(1, "t2", true)).unwrap();
    assert_eq!(
        m.apply(&MissionEvent::Done { tip: "t1".into() }),
        Err(MissionRefusal::DoneNotProven)
    );
    m.apply(&report(1, "t1", true)).unwrap();
    m.apply(&MissionEvent::Done { tip: "t1".into() }).unwrap();
    assert_eq!(m.done_tip(), Some("t1"));

    let mut m = at(MissionState::Running);
    let spent = MissionEvent::Stopped {
        reason: StopReason::LimitSpent,
        by: MissionActor::Core,
    };
    assert_eq!(m.apply(&spent), Err(MissionRefusal::LimitNotSpent));
    m.apply(&MissionEvent::SpendObserved {
        tokens: 1_000,
        wall_clock_secs: 0,
    })
    .unwrap();
    m.apply(&spent).unwrap();
    assert_eq!(m.stopped_reason(), Some(StopReason::LimitSpent));
}

#[test]
fn a_plan_is_not_approved_unproposed_or_by_an_agent() {
    let mut m = Mission::from_event(&drafted()).unwrap();
    assert_eq!(
        m.apply(&approved(1, "d1", PERSON)),
        Err(MissionRefusal::PlanMismatch)
    );
    m.apply(&proposed(1, "d1")).unwrap();
    assert_eq!(
        m.apply(&approved(1, "d1", MissionActor::Lead)),
        Err(MissionRefusal::WrongActor {
            event: "mission_plan_approved"
        })
    );
    assert!(m.apply(&MissionEvent::Started { by: PERSON }).is_err());
    assert_eq!(m.state(), MissionState::Drafted);
}

#[test]
fn a_draft_is_checked() {
    let MissionEvent::Drafted {
        goal, limit, team, ..
    } = drafted()
    else {
        unreachable!()
    };
    let bad = |goal: &str, limit: MissionLimit, team: MissionTeam, done: Vec<DoneCheck>| {
        Mission::from_event(&MissionEvent::Drafted {
            goal: goal.into(),
            limit,
            team,
            done,
        })
        .unwrap_err()
    };
    let done = vec![DoneCheck::Verdict { agent: "r".into() }];
    assert_eq!(
        bad(&goal, limit, team.clone(), vec![]),
        MissionRefusal::Draft(DraftError::Done("must name at least one check"))
    );
    let no_tokens = MissionLimit { tokens: 0, ..limit };
    assert_eq!(
        bad(&goal, no_tokens, team.clone(), done.clone()),
        MissionRefusal::Draft(DraftError::Limit("tokens"))
    );
    let mut lone = team.clone();
    lone.members.truncate(1);
    assert!(matches!(
        bad(&goal, limit, lone, done.clone()),
        MissionRefusal::Draft(DraftError::Team(_))
    ));
    let mut no_lead = team;
    no_lead.lead = "nobody".into();
    assert!(matches!(
        bad(&goal, limit, no_lead, done),
        MissionRefusal::Draft(DraftError::Team(_))
    ));
    assert_eq!(
        Mission::replay(&[proposed(1, "d")]),
        Err(MissionRefusal::NotDrafted)
    );
    assert_eq!(Mission::replay(&[]), Err(MissionRefusal::NotDrafted));
}

#[test]
fn events_round_trip_as_json() {
    for e in alphabet() {
        let text = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<MissionEvent>(&text).unwrap(), e);
    }
}
