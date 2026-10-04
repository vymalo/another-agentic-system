//! Whether a thread may be deleted (ADR 0043, decision 3): the pure check the application makes on
//! each thread it is about to erase.
//!
//! A thread that works cannot be: the delete cascades the unsent `cancel` row away, and the remote
//! task would go on working, perhaps pushing to a repository. Cancelling keeps the state (the thread
//! reaches `cancelled` only when the agent reports), so the person stops the thread, waits for it
//! to end, and then deletes it. A `blocked` thread can be deleted: its turn is over (ADR 0029).

use crate::error::{Classify, ErrorClass};
use crate::gate::Job;
use crate::thread::ThreadState;

/// Why a thread cannot be deleted now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NotDeletable {
    /// The thread is `queued`, `working` or `verifying`: an agent (or the verification gate) is at
    /// work for it.
    #[error("the thread is {} and its job is running", .0.as_str())]
    Active(ThreadState),
    /// An agent the job asked is still working, whatever the state of the thread says.
    #[error("an agent the job asked is still working")]
    AskRunning,
}

impl Classify for NotDeletable {
    fn class(&self) -> ErrorClass {
        ErrorClass::Rejected
    }
}

/// Whether the thread in `state`, with the ledger of its current `job`, may be deleted.
///
/// # Errors
/// [`NotDeletable::Active`] while the thread is `queued`, `working` or `verifying`;
/// [`NotDeletable::AskRunning`] while an ask of the job has not ended. `blocked` and the terminal
/// states pass.
pub fn deletable(state: ThreadState, job: &Job) -> Result<(), NotDeletable> {
    match state {
        ThreadState::Queued | ThreadState::Working | ThreadState::Verifying => {
            return Err(NotDeletable::Active(state));
        }
        ThreadState::Blocked | ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => {
        }
    }
    if job.asks.iter().any(crate::ask::Ask::is_running) {
        return Err(NotDeletable::AskRunning);
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::ask::{Ask, AskOutcome};
    use crate::ids::AgentId;
    use crate::thread_tools::Caller;

    fn ask(outcome: Option<AskOutcome>) -> Ask {
        Ask {
            n: 1,
            by: Caller::Main,
            agent: AgentId::new("researcher"),
            depth: 1,
            call_key: None,
            fingerprint: None,
            task_id: None,
            outcome,
        }
    }

    fn job_with(asks: Vec<Ask>) -> Job {
        Job {
            asks,
            ..Job::default()
        }
    }

    #[test]
    fn a_thread_whose_turn_is_over_can_be_deleted() {
        for state in [
            ThreadState::Blocked,
            ThreadState::Done,
            ThreadState::Failed,
            ThreadState::Cancelled,
        ] {
            assert_eq!(deletable(state, &Job::default()), Ok(()), "{state:?}");
        }
    }

    #[test]
    fn a_thread_that_works_cannot() {
        for state in [
            ThreadState::Queued,
            ThreadState::Working,
            ThreadState::Verifying,
        ] {
            assert_eq!(
                deletable(state, &Job::default()),
                Err(NotDeletable::Active(state)),
                "{state:?}"
            );
        }
    }

    #[test]
    fn a_running_ask_refuses_whatever_the_state_says_but_a_finished_one_does_not() {
        let running = job_with(vec![ask(None)]);
        assert_eq!(
            deletable(ThreadState::Blocked, &running),
            Err(NotDeletable::AskRunning)
        );
        assert_eq!(
            deletable(ThreadState::Done, &running),
            Err(NotDeletable::AskRunning)
        );
        let ended = job_with(vec![
            ask(Some(AskOutcome::Completed)),
            ask(Some(AskOutcome::TimedOut)),
        ]);
        assert_eq!(deletable(ThreadState::Done, &ended), Ok(()));
        // the state is looked at first
        assert_eq!(
            deletable(ThreadState::Working, &running),
            Err(NotDeletable::Active(ThreadState::Working))
        );
    }

    #[test]
    fn a_refusal_is_a_rejection_that_says_why() {
        let e = NotDeletable::Active(ThreadState::Working);
        assert_eq!(e.class(), ErrorClass::Rejected);
        assert_eq!(
            e.to_string(),
            "the thread is working and its job is running"
        );
        assert_eq!(NotDeletable::AskRunning.class(), ErrorClass::Rejected);
    }
}
