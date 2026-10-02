//! Property tests of the verification gate (ADR 0018): over random sequences of every input, the
//! rules that must never break. They only look at what `transition` returns (states, jobs and
//! the events it appends), not at how it decides.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;

use orch_core::*;
use proptest::prelude::*;
use serde_json::json;

const SHAS: [&str; 2] = [
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
];
const NAMES: [&str; 2] = ["build", "lint"];
const REPOS: [&str; 2] = ["https://github.com/o/r.git", "https://github.com/other/r"];

fn agent(update: AgentUpdate) -> Input {
    Input::Agent {
        agent: AgentId::new("a"),
        revision: None,
        update,
    }
}
fn artifact(name: &str, data: serde_json::Value) -> Input {
    agent(AgentUpdate::Artifact {
        name: name.into(),
        mime_type: None,
        uri: None,
        text: Some(data.to_string()),
    })
}

fn arb_task_state() -> impl Strategy<Value = AgentTaskState> {
    prop_oneof![
        1 => Just(AgentTaskState::Submitted),
        1 => Just(AgentTaskState::Working),
        1 => Just(AgentTaskState::InputRequired),
        1 => Just(AgentTaskState::AuthRequired),
        // Weighted up: the interesting moves start at `completed`.
        4 => Just(AgentTaskState::Completed),
        1 => Just(AgentTaskState::Failed),
        1 => Just(AgentTaskState::Canceled),
        1 => Just(AgentTaskState::Rejected),
    ]
}

fn arb_conclusion() -> impl Strategy<Value = CiConclusion> {
    prop_oneof![
        3 => Just(CiConclusion::Success),
        1 => Just(CiConclusion::Neutral),
        1 => Just(CiConclusion::Skipped),
        2 => Just(CiConclusion::Failure),
        1 => Just(CiConclusion::Cancelled),
        1 => Just(CiConclusion::TimedOut),
    ]
}

fn arb_input() -> impl Strategy<Value = Input> {
    let sha = (0..SHAS.len()).prop_map(|i| SHAS[i]);
    let small = 0_u32..6;
    prop_oneof![
        2 => "[a-z]{1,6}".prop_map(|text| Input::UserMessage {
            user: UserId::new("u@x.io"),
            text,
            message_id: None,
            run_id: None,
            origin: orch_core::Origin::Agui,
            catalog: None,
        }),
        1 => Just(Input::Cancel {
            user: UserId::new("u@x.io")
        }),
        6 => arb_task_state().prop_map(|state| agent(AgentUpdate::Status { state, detail: None })),
        4 => (sha.clone(), 0..REPOS.len()).prop_map(|(sha, r)| artifact(
            "branch",
            json!({"repository": REPOS[r], "branch": "agent/x", "commit": sha})
        )),
        4 => (any::<bool>(), sha.clone(), prop::collection::vec("[a-z]{1,5}", 0..3)).prop_map(
            |(passed, sha, findings)| artifact(
                "checks",
                json!({"passed": passed, "commit": sha, "findings": findings})
            )
        ),
        1 => Just(artifact("checks", json!({"passed": "maybe"}))),
        // a `branch` the gate cannot use: a short hash, and a repository that is no address
        1 => Just(artifact("branch", json!({"repository": REPOS[0], "branch": "agent/x", "commit": "abc"}))),
        1 => Just(artifact("branch", json!({"repository": "not a repo", "branch": "agent/x", "commit": SHAS[0]}))),
        1 => Just(artifact("other", json!({}))),
        1 => (any::<bool>(), "[a-z]{1,5}").prop_map(|(retryable, reason)| Input::DeliveryFailed {
            reason,
            retryable
        }),
        1 => Just(Input::CancelledBeforeStart),
        1 => "[a-z]{1,6}".prop_map(|text| Input::Redeliver { text }),
        1 => (any::<bool>(), "[a-z]{1,5}")
            .prop_map(|(retryable, reason)| Input::CancelRejected { reason, retryable }),
        8 => (0..NAMES.len(), sha.clone(), 0..REPOS.len(), arb_conclusion()).prop_map(
            |(n, sha, r, conclusion)| Input::CiReported(CiReport {
                provider: CiProvider::Generic,
                repository: REPOS[r].to_owned(),
                sha: sha.to_owned(),
                branch: None,
                name: NAMES[n].to_owned(),
                conclusion,
                url: None,
                summary: None,
            })
        ),
        6 => (small.clone(), small.clone(), any::<bool>(), prop::collection::vec("[a-z]{1,5}", 0..3))
            .prop_map(|(attempt, verification, passed, findings)| Input::VerifierReported {
                attempt,
                verification,
                verdict: Verdict { passed, findings }
            }),
        3 => (small.clone(), small.clone(), "[a-z]{0,5}").prop_map(
            |(attempt, verification, reason)| Input::VerifierFailed {
                attempt,
                verification,
                reason
            }
        ),
        4 => (small.clone(), small, any::<bool>()).prop_map(|(attempt, verification, ci)| {
            Input::TimerFired(if ci {
                Timer::CiDeadline { attempt, verification }
            } else {
                Timer::VerifierDeadline { attempt, verification }
            })
        }),
    ]
}

fn arb_gate() -> impl Strategy<Value = GatePolicy> {
    (
        prop::collection::btree_set(
            prop_oneof![
                Just(CheckSource::Ci),
                Just(CheckSource::AgentChecks),
                Just(CheckSource::Verifier)
            ],
            1..=3,
        ),
        1_u32..=4,
        prop::collection::btree_set(0..NAMES.len(), 0..=2),
        any::<bool>(),
    )
        .prop_map(|(require, max_attempts, names, verifier)| {
            let mut gate = GatePolicy::requiring(require);
            gate.max_attempts = max_attempts;
            gate.ci.required = names.into_iter().map(|n| NAMES[n].to_owned()).collect();
            gate.verifier = verifier.then(|| AgentId::new("reviewer"));
            gate
        })
}

fn appended(cmds: &[Command]) -> impl Iterator<Item = &EventBody> {
    cmds.iter().filter_map(|c| match c {
        Command::Append(d) => Some(&d.body),
        Command::Delegate { .. }
        | Command::DelegateAction { .. }
        | Command::RequestCancel { .. }
        | Command::Watch { .. }
        | Command::Schedule { .. }
        | Command::SetTitle(_)
        | Command::RequestTitle { .. }
        | Command::SetDescription(_)
        | Command::RequestDescription { .. }
        | Command::RequestVerification { .. } => None,
    })
}

/// Inputs that name a verification or attempt other than the current one.
fn stale_probes(job: &Job) -> Vec<Input> {
    let mut probes = Vec::new();
    let others = [
        (job.attempt + 1, job.verification),
        (job.attempt, job.verification + 1),
        (job.attempt + 1, job.verification + 1),
    ]
    .into_iter()
    .chain((job.verification > 0).then(|| (job.attempt, job.verification - 1)))
    .chain((job.attempt > 1).then(|| (job.attempt - 1, job.verification)));
    for (attempt, verification) in others {
        probes.push(Input::VerifierReported {
            attempt,
            verification,
            verdict: Verdict {
                passed: true,
                findings: vec![],
            },
        });
        probes.push(Input::VerifierReported {
            attempt,
            verification,
            verdict: Verdict {
                passed: false,
                findings: vec!["x".into()],
            },
        });
        probes.push(Input::VerifierFailed {
            attempt,
            verification,
            reason: "down".into(),
        });
        probes.push(Input::TimerFired(Timer::CiDeadline {
            attempt,
            verification,
        }));
        probes.push(Input::TimerFired(Timer::VerifierDeadline {
            attempt,
            verification,
        }));
    }
    probes
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn the_gate_holds_over_random_sequences(
        gate in arb_gate(),
        inputs in prop::collection::vec(arb_input(), 0..60),
    ) {
        let start = Snapshot::queued(gate.clone());
        let mut snap = start.clone();
        // The last answer of each source in the current attempt, read from the log alone.
        let mut answers: BTreeMap<CheckSource, CheckStatus> = BTreeMap::new();
        let mut trace = Vec::new();
        for input in &inputs {
            let before = snap.clone();
            let outcome = transition(&before, input);
            trace.push(outcome.clone());
            let (next, cmds) = match outcome {
                Ok(ok) => ok,
                Err(TransitionError::Finished { state }) | Err(TransitionError::InvalidInState { state, .. }) => {
                    // Only a finished thread refuses, and it changes nothing.
                    prop_assert!(state.is_terminal() && state == before.state);
                    continue;
                }
                Err(other) => return Err(TestCaseError::fail(format!("unexpected error {other}"))),
            };

            // (1) A finished job absorbs everything but a message, which starts the next job
            // (ADR 0020): the state and the job stay as they were otherwise.
            let next_job = next.job.number != before.job.number;
            if before.state.is_terminal() {
                let message = matches!(input, Input::UserMessage { .. } | Input::Redeliver { .. });
                if message && !(before.state == ThreadState::Cancelled && matches!(input, Input::Redeliver { .. })) {
                    prop_assert!(next_job);
                    prop_assert_eq!(next.state, ThreadState::Queued);
                    prop_assert_eq!(next.job.number, before.job.number + 1);
                    let (Input::UserMessage { text, .. } | Input::Redeliver { text }) = input else {
                        unreachable!()
                    };
                    let mut expected = before.job.next();
                    if gate.is_active() {
                        expected.task = Some(text.clone());
                    }
                    prop_assert_eq!(&next.job, &expected);
                } else {
                    prop_assert_eq!(&next, &before);
                }
            } else {
                prop_assert!(!next_job);
            }
            // (2) The attempt is between 1 and the maximum, and moves only with a rework.
            prop_assert!(next.job.attempt >= 1 && next.job.attempt <= gate.max());
            let reworks = appended(&cmds).filter(|b| matches!(b, EventBody::Rework(_))).count();
            let delegations = cmds.iter().filter(|c| matches!(c, Command::Delegate { .. })).count();
            if next_job {
                prop_assert_eq!(next.job.attempt, 1);
                prop_assert_eq!(next.job.verification, before.job.verification);
            } else if next.job.attempt != before.job.attempt {
                prop_assert_eq!(next.job.attempt, before.job.attempt + 1);
                prop_assert_eq!(reworks, 1);
                prop_assert_eq!(next.state, ThreadState::Queued);
                prop_assert!(next.job.results.is_empty() && next.job.pushed.is_none());
            } else {
                prop_assert_eq!(reworks, 0);
            }
            prop_assert!(delegations <= 1);
            // What the person wrote is kept within its cap, and only ever grows by a message.
            prop_assert!(next.job.task.as_ref().is_none_or(|t| t.len() <= MAX_TASK_BYTES));
            // The gate policy of a running job never changes.
            prop_assert_eq!(&next.job.gate, &gate);
            // (3) The ledger only holds this attempt's answers.
            prop_assert!(next.job.results.iter().all(|r| r.attempt == next.job.attempt));

            // Read the log: what each source last said in this attempt.
            for body in appended(&cmds) {
                match body {
                    EventBody::CheckResult(r) if !r.stale => {
                        answers.insert(r.source, r.status);
                    }
                    EventBody::Rework(_) | EventBody::JobStarted(_) => answers.clear(),
                    _ => {}
                }
            }

            // (4) Never done while a required source is failed or pending.
            if next.state == ThreadState::Done && before.state != ThreadState::Done {
                for source in &gate.require {
                    prop_assert_eq!(
                        answers.get(source),
                        Some(&CheckStatus::Passed),
                        "done while {:?} is not passed", source
                    );
                }
            }
            // (4b) Git is the artifact: a job gated on the agent's checks is done only with a
            // pushed commit, and the checks of the agent name exactly that commit and passed.
            if next.state == ThreadState::Done
                && before.state != ThreadState::Done
                && gate.requires(CheckSource::AgentChecks)
            {
                let pushed = next.job.pushed.as_ref();
                prop_assert!(pushed.is_some(), "done with no pushed commit");
                let entry = next
                    .job
                    .results
                    .iter()
                    .find(|r| r.source == CheckSource::AgentChecks);
                prop_assert!(entry.is_some(), "done with no agent_checks entry");
                let entry = entry.unwrap();
                prop_assert_eq!(entry.status, CheckStatus::Passed);
                prop_assert_eq!(entry.commit.as_deref(), pushed.map(|p| p.commit.as_str()));
            }
            // A failed source is never left waiting: the thread reworks, fails or is done.
            if next.state == ThreadState::Verifying {
                prop_assert!(answers.values().all(|s| *s != CheckStatus::Failed));
                prop_assert!(next.job.hold.is_none());
            }
            // (5) Only a finished agent starts a verification.
            if next.state == ThreadState::Verifying && before.state != ThreadState::Verifying {
                let finished = matches!(input, Input::Agent { update: AgentUpdate::Status { state: AgentTaskState::Completed, .. }, .. });
                prop_assert!(finished);
                prop_assert_eq!(next.job.verification, before.job.verification + 1);
            } else if before.state == ThreadState::Verifying && next.state != ThreadState::Verifying {
                prop_assert_eq!(next.job.verification, before.job.verification);
            }
            // (6) A timeout blocks without spending an attempt.
            if next.job.hold.is_some() {
                prop_assert_eq!(next.state, ThreadState::Blocked);
                prop_assert_eq!(next.job.attempt, before.job.attempt);
            }
            // (7) At most one thread_state event, naming the state entered, never `verifying`.
            for body in appended(&cmds) {
                if let EventBody::ThreadState(t) = body {
                    prop_assert_eq!(t.state, next.state);
                    prop_assert!(t.state != ThreadState::Verifying);
                }
            }

            // (8) An input for another attempt or verification never changes state or job.
            for probe in stale_probes(&next.job) {
                let (probed, _) = transition(&next, &probe).unwrap();
                prop_assert_eq!(&probed, &next, "{:?}", probe);
            }

            // (9) What is stored is what is read back.
            let stored = serde_json::to_value(&next.job).unwrap();
            prop_assert_eq!(&serde_json::from_value::<Job>(stored).unwrap(), &next.job);

            snap = next;
        }

        // (10) Replay is deterministic.
        let mut again = start;
        for (input, expected) in inputs.iter().zip(&trace) {
            let outcome = transition(&again, input);
            prop_assert_eq!(&outcome, expected);
            if let Ok((next, _)) = outcome {
                again = next;
            }
        }
        prop_assert_eq!(again, snap);
    }

    /// With no gate the job is never touched, whatever arrives, and the thread never verifies.
    #[test]
    fn no_gate_no_change_to_the_job(inputs in prop::collection::vec(arb_input(), 0..60)) {
        let mut snap = Snapshot::queued(GatePolicy::default());
        for input in &inputs {
            if let Ok((next, cmds)) = transition(&snap, input) {
                // Only the number of the job moves (a message on a finished thread), and the
                // description's ledger, which the end of a job asks for.
                prop_assert_eq!(
                    &next.job,
                    &Job { number: next.job.number, description: next.job.description, ..Job::default() }
                );
                prop_assert!(next.job.number >= snap.job.number);
                prop_assert!(next.state != ThreadState::Verifying);
                let machine_commands = cmds.iter().any(|c| {
                    matches!(
                        c,
                        Command::Watch { .. }
                            | Command::Schedule { .. }
                            | Command::RequestVerification { .. }
                    )
                });
                prop_assert!(!machine_commands);
                prop_assert!(appended(&cmds).all(|b| !matches!(b, EventBody::Rework(_))
                    && !matches!(b, EventBody::CheckResult(r) if !r.stale)));
                snap = next;
            }
        }
    }
}
