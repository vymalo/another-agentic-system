//! `EntitlementSource` conformance cases: what the orchestrator relies on to suspend a person's
//! agents when their plan lapses and to leave them alone (or resume them) when it does not.
//!
//! An implementation supplies an [`EntitlementFixture`]: the source under test, the means to make a
//! person active or lapsed since a moment, to make it forget one, to take it down, and the people
//! that were asked.

use jiff::Timestamp;
use orch_core::{Classify, ErrorClass, UserId};

use crate::{Entitlement, EntitlementSource};

/// A source under test, with the knobs a case writes a person's plan with.
pub trait EntitlementFixture: Send + Sync + 'static {
    /// The source under test.
    type Source: EntitlementSource;

    /// The source.
    fn source(&self) -> &Self::Source;

    /// `person`'s plan covers them from now on.
    fn set_active(&self, person: &UserId);

    /// `person`'s plan ended at `since`.
    fn set_lapsed(&self, person: &UserId, since: Timestamp);

    /// The source has no record of `person` from now on.
    fn forget(&self, person: &UserId);

    /// Takes the source down (`true`): every read is
    /// [`EntitlementError::Unavailable`](crate::EntitlementError::Unavailable) until `false`.
    fn set_unavailable(&self, unavailable: bool);

    /// Who was asked, in the order asked, every read since the test began.
    fn asked(&self) -> Vec<UserId>;
}

fn person(key: &str) -> UserId {
    UserId::new(key)
}

fn at(second: i64) -> Timestamp {
    Timestamp::from_second(second).unwrap()
}

/// A person whose plan is running is [`Entitlement::Active`].
pub async fn an_active_person_is_active<F: EntitlementFixture>(f: F) {
    let source = f.source();
    let alice = person("alice@example.test");
    f.set_active(&alice);
    let answer = source.entitled(&alice).await.unwrap();
    assert_eq!(answer, Entitlement::Active);
    assert!(answer.is_active());
    assert_eq!(answer.lapsed_since(), None);
}

/// A person whose plan ended is [`Entitlement::Lapsed`] with the moment it ended, and never active.
pub async fn a_lapsed_person_is_lapsed_since_the_moment<F: EntitlementFixture>(f: F) {
    let source = f.source();
    let bob = person("bob@example.test");
    let since = at(1_790_000_000);
    f.set_lapsed(&bob, since);
    let answer = source.entitled(&bob).await.unwrap();
    assert_eq!(answer, Entitlement::Lapsed { since });
    assert_eq!(answer.lapsed_since(), Some(since));
    assert!(!answer.is_active(), "a lapsed person is never entitled");
}

/// A person the source has no record of is [`Entitlement::Unknown`], and an unknown person is never
/// entitled: the port fails closed.
pub async fn a_person_the_source_never_heard_of_is_unknown<F: EntitlementFixture>(f: F) {
    let source = f.source();
    let carol = person("carol@example.test");
    let answer = source.entitled(&carol).await.unwrap();
    assert_eq!(answer, Entitlement::Unknown);
    assert!(
        !answer.is_active(),
        "a person with no plan record must never read as entitled"
    );
}

/// A source that stops knowing a person answers [`Entitlement::Unknown`] again.
pub async fn a_forgotten_person_is_unknown_again<F: EntitlementFixture>(f: F) {
    let source = f.source();
    let dave = person("dave@example.test");
    f.set_active(&dave);
    assert_eq!(source.entitled(&dave).await.unwrap(), Entitlement::Active);
    f.forget(&dave);
    assert_eq!(source.entitled(&dave).await.unwrap(), Entitlement::Unknown);
}

/// A lapse and a renewal are both seen on the very next read: `Active` is never served from a cache.
pub async fn a_lapse_and_a_renewal_show_on_the_next_read<F: EntitlementFixture>(f: F) {
    let source = f.source();
    let erin = person("erin@example.test");
    f.set_active(&erin);
    assert_eq!(source.entitled(&erin).await.unwrap(), Entitlement::Active);
    let since = at(1_790_100_000);
    f.set_lapsed(&erin, since);
    assert_eq!(
        source.entitled(&erin).await.unwrap(),
        Entitlement::Lapsed { since },
        "the lapse was not seen on the next read"
    );
    f.set_active(&erin);
    assert_eq!(
        source.entitled(&erin).await.unwrap(),
        Entitlement::Active,
        "the renewal was not seen on the next read"
    );
}

/// A read asks about that person and no other, and no answer's `Debug` names a person: one person's
/// entitlement is never the answer for the next.
pub async fn a_read_asks_about_that_person_and_no_other<F: EntitlementFixture>(f: F) {
    let source = f.source();
    let frank = person("frank@example.test");
    let grace = person("grace@example.test");
    f.set_active(&frank);
    f.set_lapsed(&grace, at(1_790_200_000));

    let first = source.entitled(&frank).await.unwrap();
    let second = source.entitled(&grace).await.unwrap();
    assert_eq!(first, Entitlement::Active);
    assert_eq!(
        second,
        Entitlement::Lapsed {
            since: at(1_790_200_000)
        }
    );
    assert_eq!(f.asked(), vec![frank.clone(), grace.clone()]);

    let stranger = person("heidi@example.test");
    let third = source.entitled(&stranger).await.unwrap();
    assert_eq!(
        third,
        Entitlement::Unknown,
        "an unknown person took another person's answer"
    );
    assert_eq!(
        f.asked(),
        vec![frank.clone(), grace.clone(), stranger.clone()]
    );

    for answer in [first, second, third] {
        let shown = format!("{answer:?}");
        for who in [&frank, &grace, &stranger] {
            assert!(
                !shown.contains(who.as_str()),
                "an entitlement's Debug names a person: {shown}"
            );
        }
    }
}

/// A source that is down is
/// [`EntitlementError::Unavailable`](crate::EntitlementError::Unavailable) (class `Transient`), and
/// neither the error nor its `Debug` names a person.
pub async fn an_unavailable_source_is_transient_and_names_nobody<F: EntitlementFixture>(f: F) {
    let source = f.source();
    let ivan = person("ivan@example.test");
    let judy = person("judy@example.test");
    f.set_active(&ivan);
    f.set_unavailable(true);

    let error = source.entitled(&judy).await.unwrap_err();
    assert_eq!(error.class(), ErrorClass::Transient);
    assert!(error.is_retryable());
    let shown = format!("{error} {error:?}");
    for who in [&ivan, &judy] {
        assert!(
            !shown.contains(who.as_str()),
            "the error names a person: {shown}"
        );
    }
}
