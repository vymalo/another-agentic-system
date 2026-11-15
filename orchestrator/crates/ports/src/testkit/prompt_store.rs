//! `PromptStore` conformance cases: what the API relies on from any store of people's prompt
//! libraries (ADR 0070), stated once and run against the in-memory store and the Postgres store.
//!
//! An implementation supplies a fresh store (`async fn() -> Option<S>`; `None` skips the suite).
//! Each case is `async fn(store)`, uses owners of its own (so a store shared by the cases of one
//! run is never confused) and gives up after 60 seconds.
//!
//! What is deliberately not asserted: how a prompt is kept, what a revision is beyond "a new one for
//! every write, never given twice", and the wording of any error.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use orch_core::prompt::{PromptAudience, PromptDefinition, PromptId, PromptInput};
use orch_core::{Timestamp, UserId};
use serde_json::json;
use uuid::Uuid;

use crate::{
    MAX_PROMPTS, PromptPage, PromptRevision, PromptScope, PromptStore, PromptStoreError,
    PromptViewer, StoredPrompt,
};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

/// How many times the cases that race writers are run, each time with an owner of its own.
const RACE_ROUNDS: usize = 5;

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, f)
        .await
        .expect("the case took too long")
}

/// A person no other case uses.
fn owner() -> UserId {
    UserId::new(&format!("{}@conformance.test", Uuid::now_v7()))
}

fn viewer(user: &UserId, roles: &[&str]) -> PromptViewer {
    PromptViewer {
        user: user.clone(),
        roles: roles.iter().map(|r| (*r).to_owned()).collect(),
    }
}

/// A role value no other case uses.
fn role() -> String {
    format!("role-{}", Uuid::now_v7().simple())
}

fn fresh_id() -> PromptId {
    let mut entropy = [0_u8; 16];
    getrandom::fill(&mut entropy).expect("the operating system's random source");
    PromptId::from_entropy(entropy)
}

fn at(second: i64) -> Timestamp {
    Timestamp::from_second(1_700_000_000 + second).expect("a time")
}

/// A prompt of one variable, shared with `audience`.
fn prompt(id: &PromptId, name: &str, audience: &[&str]) -> PromptDefinition {
    PromptDefinition::new(
        id.clone(),
        PromptInput {
            name: name.to_owned(),
            title: format!("Title of {name}"),
            description: "About it".to_owned(),
            body: "Hello {{who}}".to_owned(),
            variables: vec![json!({"id": "who", "question": "Who?", "type": "text"})],
            audience: audience.iter().map(|s| (*s).to_owned()).collect(),
        },
    )
    .expect("a valid prompt")
}

/// `n`: the name `p<n>`.
fn named(n: usize) -> String {
    format!("p{n}")
}

async fn save<S: PromptStore>(
    store: &S,
    owner: &UserId,
    name: &str,
    audience: &[&str],
) -> StoredPrompt {
    store
        .insert_prompt(owner, &prompt(&fresh_id(), name, audience), at(0))
        .await
        .expect("insert")
}

async fn ids<S: PromptStore>(store: &S, who: &PromptViewer, scope: PromptScope) -> Vec<PromptId> {
    store
        .list_prompts(who, scope, None, 100)
        .await
        .expect("list")
        .items
        .into_iter()
        .map(|i| i.id)
        .collect()
}

/// A prompt comes back whole with the revision and the time `insert_prompt` gave, as its owner's.
pub async fn insert_then_get_round_trips<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let id = fresh_id();
        let definition = prompt(&id, "review", &["team"]);
        let stored = store
            .insert_prompt(&a, &definition, at(7))
            .await
            .expect("insert");
        assert!(stored.revision().get() > 0);
        assert_eq!(stored.updated_at(), at(7));
        assert!(stored.own());
        let got = store
            .get_prompt(&viewer(&a, &[]), &id)
            .await
            .expect("get")
            .expect("the prompt is kept");
        assert_eq!(got.prompt(), &definition);
        assert_eq!(got.revision(), stored.revision());
        assert_eq!(got.updated_at(), at(7));
        assert!(got.own());
        assert_eq!(got.prompt().variables().len(), 1);
    })
    .await;
}

/// An id nobody holds is `None`, not an error.
pub async fn a_missing_prompt_is_none<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        assert!(
            store
                .get_prompt(&viewer(&a, &[]), &fresh_id())
                .await
                .expect("get")
                .is_none()
        );
    })
    .await;
}

/// A private prompt is not another person's to read, list, replace or delete, and it stays as it was.
pub async fn a_private_prompt_is_nobody_elses<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        let mine = save(&store, &a, "mine", &[]).await;
        let id = mine.prompt().id().clone();
        let who = viewer(&b, &["team"]);
        assert!(store.get_prompt(&who, &id).await.expect("get").is_none());
        assert!(ids(&store, &who, PromptScope::All).await.is_empty());
        let theirs = prompt(&id, "mine", &[]);
        assert!(matches!(
            store
                .replace_prompt(&b, &theirs, mine.revision(), at(1))
                .await,
            Err(PromptStoreError::NotFound)
        ));
        assert!(matches!(
            store.delete_prompt(&b, &id, None).await,
            Err(PromptStoreError::NotFound)
        ));
        let after = store
            .get_prompt(&viewer(&a, &[]), &id)
            .await
            .expect("get")
            .expect("still there");
        assert_eq!(after, mine);
    })
    .await;
}

/// The id is global: it names one prompt for everybody, so a second person cannot take it.
pub async fn ids_are_global<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        let held = save(&store, &a, "held", &[]).await;
        let id = held.prompt().id().clone();
        let result = store
            .insert_prompt(&b, &prompt(&id, "other", &[]), at(0))
            .await;
        assert!(
            matches!(result, Err(PromptStoreError::Exists)),
            "{result:?}"
        );
        assert_eq!(
            ids(&store, &viewer(&b, &[]), PromptScope::All).await,
            Vec::new()
        );
        assert_eq!(
            store
                .get_prompt(&viewer(&a, &[]), &id)
                .await
                .expect("get")
                .expect("kept"),
            held
        );
    })
    .await;
}

/// A name is unique for an owner, and another person may use it.
pub async fn names_are_unique_per_owner<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        save(&store, &a, "review", &[]).await;
        let again = store
            .insert_prompt(&a, &prompt(&fresh_id(), "review", &[]), at(0))
            .await;
        assert!(
            matches!(again, Err(PromptStoreError::NameTaken)),
            "{again:?}"
        );
        assert!(
            store
                .insert_prompt(&b, &prompt(&fresh_id(), "review", &[]), at(0))
                .await
                .is_ok()
        );
    })
    .await;
}

/// A rename onto another of the owner's names is refused and changes nothing.
pub async fn a_rename_to_a_taken_name_is_name_taken_and_changes_nothing<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let first = save(&store, &a, "a", &[]).await;
        let second = save(&store, &a, "b", &[]).await;
        let renamed = prompt(second.prompt().id(), "a", &[]);
        let result = store
            .replace_prompt(&a, &renamed, second.revision(), at(5))
            .await;
        assert!(
            matches!(result, Err(PromptStoreError::NameTaken)),
            "{result:?}"
        );
        let who = viewer(&a, &[]);
        for before in [&first, &second] {
            let now = store
                .get_prompt(&who, before.prompt().id())
                .await
                .expect("get")
                .expect("there");
            assert_eq!(&now, before);
        }
    })
    .await;
}

/// `Mine` is the viewer's own prompts in id order, and says how many they hold.
pub async fn mine_lists_only_own_ordered_by_id<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        let team = role();
        let mut mine = Vec::new();
        for n in 0..3 {
            mine.push(save(&store, &a, &named(n), &[]).await.prompt().id().clone());
        }
        save(&store, &b, "theirs", &[team.as_str()]).await;
        let who = viewer(&a, &[team.as_str()]);
        let page = store
            .list_prompts(&who, PromptScope::Mine, None, 100)
            .await
            .expect("list");
        let mut want = mine.clone();
        want.sort();
        assert_eq!(
            page.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
            want
        );
        assert_eq!(page.held, 3);
        assert!(!page.more);
        assert!(page.items.iter().all(|i| i.own && i.audience.is_some()));
    })
    .await;
}

/// Pages follow the id: no repeat, no gap, and `more` is true until the last.
pub async fn pages_follow_the_id<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        for n in 0..5 {
            save(&store, &a, &named(n), &[]).await;
        }
        let who = viewer(&a, &[]);
        let all = ids(&store, &who, PromptScope::Mine).await;
        assert_eq!(all.len(), 5);
        let mut seen = Vec::new();
        let mut after: Option<PromptId> = None;
        let mut flags = Vec::new();
        loop {
            let page: PromptPage = store
                .list_prompts(&who, PromptScope::Mine, after.as_ref(), 2)
                .await
                .expect("list");
            flags.push(page.more);
            seen.extend(page.items.iter().map(|i| i.id.clone()));
            match page.items.last() {
                Some(last) if page.more => after = Some(last.id.clone()),
                _ => break,
            }
        }
        assert_eq!(seen, all);
        assert_eq!(flags, vec![true, true, false]);
    })
    .await;
}

/// A prompt shared with a role is seen by a viewer whose roles hold it, listed and read, as not their own
/// and with no audience; a viewer with other roles, or none, sees nothing.
pub async fn a_shared_prompt_is_seen_by_its_audience_only<S: PromptStore>(store: S) {
    within(async {
        let b = owner();
        let team = role();
        let shared = save(&store, &b, "shared", &[team.as_str()]).await;
        let id = shared.prompt().id().clone();
        let a = owner();
        let member = viewer(&a, &[team.as_str(), "user"]);
        let got = store
            .get_prompt(&member, &id)
            .await
            .expect("get")
            .expect("the audience sees it");
        assert!(!got.own());
        assert!(
            got.prompt().audience().is_private(),
            "the roles are not theirs to know"
        );
        assert_eq!(got.prompt().body(), shared.prompt().body());
        for scope in [PromptScope::Shared, PromptScope::All] {
            let page = store
                .list_prompts(&member, scope, None, 100)
                .await
                .expect("list");
            assert_eq!(page.items.len(), 1);
            assert!(!page.items[0].own);
            assert!(page.items[0].audience.is_none());
            assert_eq!(page.held, 0);
        }
        assert!(ids(&store, &member, PromptScope::Mine).await.is_empty());
        for outsider in [viewer(&owner(), &[role().as_str()]), viewer(&owner(), &[])] {
            assert!(
                store
                    .get_prompt(&outsider, &id)
                    .await
                    .expect("get")
                    .is_none()
            );
            assert!(ids(&store, &outsider, PromptScope::All).await.is_empty());
        }
    })
    .await;
}

/// `*` is everybody, a viewer with no role at all included.
pub async fn everyone_is_everyone<S: PromptStore>(store: S) {
    within(async {
        let b = owner();
        let shared = save(&store, &b, "all", &["*"]).await;
        let nobody = viewer(&owner(), &[]);
        let got = store
            .get_prompt(&nobody, shared.prompt().id())
            .await
            .expect("get")
            .expect("everybody sees it");
        assert!(!got.own());
        assert!(
            ids(&store, &nobody, PromptScope::Shared)
                .await
                .contains(shared.prompt().id())
        );
    })
    .await;
}

/// Roles are compared byte for byte: `Team` is not `team`.
pub async fn roles_match_byte_for_byte<S: PromptStore>(store: S) {
    within(async {
        let b = owner();
        let tag = Uuid::now_v7().simple().to_string();
        let shared = save(&store, &b, "cased", &[format!("Team-{tag}").as_str()]).await;
        let lower = viewer(&owner(), &[format!("team-{tag}").as_str()]);
        assert!(
            store
                .get_prompt(&lower, shared.prompt().id())
                .await
                .expect("get")
                .is_none()
        );
        let exact = viewer(&owner(), &[format!("Team-{tag}").as_str()]);
        assert!(
            store
                .get_prompt(&exact, shared.prompt().id())
                .await
                .expect("get")
                .is_some()
        );
    })
    .await;
}

/// A prompt the viewer owns is listed once, as theirs, and not among the shared.
pub async fn own_shared_prompts_are_not_in_shared<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let team = role();
        let mine = save(&store, &a, "mine", &[team.as_str()]).await;
        let who = viewer(&a, &[team.as_str()]);
        assert!(ids(&store, &who, PromptScope::Shared).await.is_empty());
        let all = store
            .list_prompts(&who, PromptScope::All, None, 100)
            .await
            .expect("list");
        assert_eq!(all.items.len(), 1);
        assert_eq!(&all.items[0].id, mine.prompt().id());
        assert!(all.items[0].own);
        assert_eq!(
            all.items[0].audience,
            Some(PromptAudience::from_values(&[team]).expect("audience"))
        );
    })
    .await;
}

/// Nothing a viewer is given names the author or the roles of the share.
pub async fn nothing_of_the_author_reaches_another_viewer<S: PromptStore>(store: S) {
    within(async {
        let b = owner();
        let team = role();
        let shared = save(&store, &b, "shared", &[team.as_str()]).await;
        let a = viewer(&owner(), &[team.as_str()]);
        let got = store
            .get_prompt(&a, shared.prompt().id())
            .await
            .expect("get");
        let page = store
            .list_prompts(&a, PromptScope::All, None, 100)
            .await
            .expect("list");
        let seen = format!("{got:?} {page:?}");
        assert!(
            !seen.contains(b.as_str()),
            "the author's key is in what a viewer got"
        );
        assert!(
            !seen.contains(&team),
            "the roles of the share are in what a viewer got"
        );
    })
    .await;
}

/// An empty audience hides the prompt from the audience at once.
pub async fn unsharing_hides_at_once<S: PromptStore>(store: S) {
    within(async {
        let b = owner();
        let team = role();
        let shared = save(&store, &b, "shared", &[team.as_str()]).await;
        let id = shared.prompt().id().clone();
        let a = viewer(&owner(), &[team.as_str()]);
        assert!(store.get_prompt(&a, &id).await.expect("get").is_some());
        let private = prompt(&id, "shared", &[]);
        store
            .replace_prompt(&b, &private, shared.revision(), at(1))
            .await
            .expect("replace");
        assert!(store.get_prompt(&a, &id).await.expect("get").is_none());
        assert!(ids(&store, &a, PromptScope::All).await.is_empty());
    })
    .await;
}

/// A replace with the current revision replaces every member, gives a new revision and keeps the time it was given.
pub async fn replace_with_the_current_revision_replaces_whole<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let first = save(&store, &a, "before", &[]).await;
        let id = first.prompt().id().clone();
        let next = PromptDefinition::new(
            id.clone(),
            PromptInput {
                name: "after".into(),
                title: "Another title".into(),
                description: String::new(),
                body: "Nothing to fill".into(),
                variables: Vec::new(),
                audience: vec!["*".into()],
            },
        )
        .expect("valid");
        let done = store
            .replace_prompt(&a, &next, first.revision(), at(9))
            .await
            .expect("replace");
        assert_ne!(done.revision(), first.revision());
        assert_eq!(done.updated_at(), at(9));
        let got = store
            .get_prompt(&viewer(&a, &[]), &id)
            .await
            .expect("get")
            .expect("there");
        assert_eq!(got.prompt(), &next);
        assert_eq!(got.revision(), done.revision());
        assert_eq!(got.updated_at(), at(9));
    })
    .await;
}

/// A replace that is not based on the current revision is a `Conflict` and writes nothing.
pub async fn a_stale_replace_is_a_conflict_and_changes_nothing<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let first = save(&store, &a, "one", &[]).await;
        let id = first.prompt().id().clone();
        let newer = store
            .replace_prompt(&a, &prompt(&id, "one", &["*"]), first.revision(), at(1))
            .await
            .expect("replace");
        let stale = store
            .replace_prompt(&a, &prompt(&id, "two", &[]), first.revision(), at(2))
            .await;
        assert!(
            matches!(stale, Err(PromptStoreError::Conflict)),
            "{stale:?}"
        );
        let now = store
            .get_prompt(&viewer(&a, &[]), &id)
            .await
            .expect("get")
            .expect("there");
        assert_eq!(now, newer);
    })
    .await;
}

/// A deleted prompt stays deleted: a write based on it is `NotFound` and brings nothing back.
pub async fn a_write_after_a_delete_is_not_found_and_brings_nothing_back<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let first = save(&store, &a, "gone", &[]).await;
        let id = first.prompt().id().clone();
        store.delete_prompt(&a, &id, None).await.expect("delete");
        let again = store
            .replace_prompt(&a, &prompt(&id, "gone", &[]), first.revision(), at(1))
            .await;
        assert!(
            matches!(again, Err(PromptStoreError::NotFound)),
            "{again:?}"
        );
        assert!(
            store
                .get_prompt(&viewer(&a, &[]), &id)
                .await
                .expect("get")
                .is_none()
        );
        assert!(matches!(
            store.delete_prompt(&a, &id, None).await,
            Err(PromptStoreError::NotFound)
        ));
    })
    .await;
}

/// A delete that names a stale revision is a `Conflict`; the current one deletes; the next is `NotFound`.
pub async fn a_stale_delete_is_a_conflict<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let first = save(&store, &a, "x", &[]).await;
        let id = first.prompt().id().clone();
        let newer = store
            .replace_prompt(&a, &prompt(&id, "x", &["*"]), first.revision(), at(1))
            .await
            .expect("replace");
        let stale = store.delete_prompt(&a, &id, Some(first.revision())).await;
        assert!(
            matches!(stale, Err(PromptStoreError::Conflict)),
            "{stale:?}"
        );
        store
            .delete_prompt(&a, &id, Some(newer.revision()))
            .await
            .expect("delete at the current revision");
        let gone = store.delete_prompt(&a, &id, Some(newer.revision())).await;
        assert!(matches!(gone, Err(PromptStoreError::NotFound)), "{gone:?}");
    })
    .await;
}

/// Every write gets a revision nobody had, a delete and a save of the same id included.
pub async fn revisions_are_never_given_twice<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let mut seen: BTreeSet<PromptRevision> = BTreeSet::new();
        let first = save(&store, &a, "one", &[]).await;
        assert!(seen.insert(first.revision()));
        let id = first.prompt().id().clone();
        let mut revision = first.revision();
        for round in 0..3 {
            let next = store
                .replace_prompt(&a, &prompt(&id, "one", &[]), revision, at(round))
                .await
                .expect("replace");
            assert!(seen.insert(next.revision()), "a revision was given twice");
            revision = next.revision();
        }
        store.delete_prompt(&a, &id, None).await.expect("delete");
        let back = store
            .insert_prompt(&a, &prompt(&id, "one", &[]), at(9))
            .await
            .expect("insert again");
        assert!(
            seen.insert(back.revision()),
            "a deleted id was given an old revision"
        );
        let other = save(&store, &owner(), "other", &[]).await;
        assert!(seen.insert(other.revision()));
    })
    .await;
}

/// An owner holds at most 100, the limit is theirs alone, and one deletion makes room.
pub async fn at_most_a_hundred_per_owner<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let mut first = None;
        for n in 0..MAX_PROMPTS {
            let saved = save(&store, &a, &named(n), &[]).await;
            first.get_or_insert(saved);
        }
        let over = store
            .insert_prompt(&a, &prompt(&fresh_id(), "extra", &[]), at(0))
            .await;
        assert!(
            matches!(over, Err(PromptStoreError::TooMany { limit }) if limit == MAX_PROMPTS),
            "{over:?}"
        );
        assert!(
            store
                .insert_prompt(&owner(), &prompt(&fresh_id(), "extra", &[]), at(0))
                .await
                .is_ok(),
            "another person is not limited by it"
        );
        let held = store
            .list_prompts(&viewer(&a, &[]), PromptScope::Mine, None, 1)
            .await
            .expect("list");
        assert_eq!(held.held, 100);
        let first = first.expect("one");
        store
            .delete_prompt(&a, first.prompt().id(), None)
            .await
            .expect("delete");
        assert!(
            store
                .insert_prompt(&a, &prompt(&fresh_id(), "extra", &[]), at(0))
                .await
                .is_ok()
        );
    })
    .await;
}

/// The refusals of an insert come in one order: an id somebody holds, then a name the owner has, then the limit.
pub async fn the_order_of_refusals<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        let someone_elses = save(&store, &b, "theirs", &[]).await;
        for n in 0..MAX_PROMPTS {
            save(&store, &a, &named(n), &[]).await;
        }
        // at the limit, with a name the owner has: the name
        let taken = store
            .insert_prompt(&a, &prompt(&fresh_id(), &named(0), &[]), at(0))
            .await;
        assert!(
            matches!(taken, Err(PromptStoreError::NameTaken)),
            "{taken:?}"
        );
        // at the limit, with a name the owner has and an id somebody holds: the id
        let exists = store
            .insert_prompt(
                &a,
                &prompt(someone_elses.prompt().id(), &named(0), &[]),
                at(0),
            )
            .await;
        assert!(
            matches!(exists, Err(PromptStoreError::Exists)),
            "{exists:?}"
        );
        // at the limit, with a free name: the limit
        let over = store
            .insert_prompt(&a, &prompt(&fresh_id(), "free", &[]), at(0))
            .await;
        assert!(
            matches!(over, Err(PromptStoreError::TooMany { .. })),
            "{over:?}"
        );
    })
    .await;
}

/// Writers racing for the last places never pass the limit.
pub async fn concurrent_inserts_never_pass_the_limit<S: PromptStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        for _ in 0..RACE_ROUNDS {
            let a = owner();
            for n in 0..(MAX_PROMPTS - 8) {
                save(&*store, &a, &named(n), &[]).await;
            }
            let mut writers = Vec::new();
            for n in 0..16 {
                let (store, a) = (Arc::clone(&store), a.clone());
                writers.push(tokio::spawn(async move {
                    store
                        .insert_prompt(&a, &prompt(&fresh_id(), &format!("race{n}"), &[]), at(0))
                        .await
                }));
            }
            let mut won = 0;
            for w in writers {
                match w.await.expect("task") {
                    Ok(_) => won += 1,
                    Err(PromptStoreError::TooMany { .. }) => {}
                    Err(e) => panic!("unexpected: {e:?}"),
                }
            }
            assert_eq!(won, 8, "exactly the places that were left");
            let held = store
                .list_prompts(&viewer(&a, &[]), PromptScope::Mine, None, 1)
                .await
                .expect("list");
            assert_eq!(held.held, 100);
        }
    })
    .await;
}

/// Writers racing to save one id have one winner.
pub async fn concurrent_inserts_of_one_id_have_one_winner<S: PromptStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        for _ in 0..RACE_ROUNDS {
            let id = fresh_id();
            let mut writers = Vec::new();
            for n in 0..8 {
                let (store, id) = (Arc::clone(&store), id.clone());
                writers.push(tokio::spawn(async move {
                    store
                        .insert_prompt(&owner(), &prompt(&id, &format!("same{n}"), &[]), at(0))
                        .await
                }));
            }
            let mut won = 0;
            for w in writers {
                match w.await.expect("task") {
                    Ok(_) => won += 1,
                    Err(PromptStoreError::Exists) => {}
                    Err(e) => panic!("unexpected: {e:?}"),
                }
            }
            assert_eq!(won, 1);
        }
    })
    .await;
}

/// Writers replacing from one revision have one winner; the others find it changed.
pub async fn concurrent_replaces_from_one_revision_have_one_winner<S: PromptStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        for _ in 0..RACE_ROUNDS {
            let a = owner();
            let first = save(&*store, &a, "start", &[]).await;
            let id = first.prompt().id().clone();
            let mut writers = Vec::new();
            for n in 0..8 {
                let (store, a, id) = (Arc::clone(&store), a.clone(), id.clone());
                let revision = first.revision();
                writers.push(tokio::spawn(async move {
                    store
                        .replace_prompt(&a, &prompt(&id, "start", &[]), revision, at(n))
                        .await
                }));
            }
            let mut won = 0;
            for w in writers {
                match w.await.expect("task") {
                    Ok(_) => won += 1,
                    Err(PromptStoreError::Conflict) => {}
                    Err(e) => panic!("unexpected: {e:?}"),
                }
            }
            assert_eq!(won, 1);
        }
    })
    .await;
}

/// A page is never larger than `PAGE`, whatever limit is asked for, and `more` says the rest is there.
pub async fn a_page_is_never_larger_than_the_page_size<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        let team = role();
        for n in 0..MAX_PROMPTS {
            save(&store, &a, &named(n), &[]).await;
        }
        save(&store, &b, "shared", &[team.as_str()]).await;
        let who = viewer(&a, &[team.as_str()]);
        let page = store
            .list_prompts(&who, PromptScope::All, None, 1000)
            .await
            .expect("list");
        assert_eq!(page.items.len(), orch_core::prompt::PAGE);
        assert!(page.more, "101 are visible and a page is 100");
        let last = page.items.last().expect("a page").id.clone();
        let rest = store
            .list_prompts(&who, PromptScope::All, Some(&last), 1000)
            .await
            .expect("list");
        assert_eq!(rest.items.len(), 1);
        assert!(!rest.more);
        // a page that holds exactly `PAGE` and no more says so
        let own = store
            .list_prompts(&who, PromptScope::Mine, None, 1000)
            .await
            .expect("list");
        assert_eq!(own.items.len(), orch_core::prompt::PAGE);
        assert!(!own.more);
    })
    .await;
}

/// Every text a variable can hold (and the title and the description) comes back as it was saved:
/// accents, emoji, joiners, direction marks, quotes, backslashes, a tab in a description, and an
/// integer written as a float.
pub async fn every_allowed_text_of_a_variable_is_read_back<S: PromptStore>(store: S) {
    within(async {
        let a = owner();
        let words = "é😀 \u{200d}\u{200e} \"quoted\" back\\slash {braces} <b>&amp;</b> 'single'";
        // the labels and the unit are held to 20 to 60 characters
        let short = "é😀\u{200d}\u{200e} \"q\" \\ {x}";
        let variables = vec![
            json!({"id": "t", "question": words, "help": words, "type": "text", "default": words}),
            json!({"id": "c", "question": "C?", "options": [
                {"value": "a", "label": words, "description": words, "icon": "git", "recommended": true},
                {"value": "b", "label": "B"}]}),
            json!({"id": "y", "question": "Y?", "type": "confirm", "yesLabel": short, "noLabel": "No"}),
            json!({"id": "n", "question": "N?", "type": "number", "unit": short, "minValue": "0", "step": "0.5"}),
            json!({"id": "s", "question": "S?", "type": "scale", "to": 5.0, "fromLabel": short, "toLabel": short}),
        ];
        let definition = PromptDefinition::new(
            fresh_id(),
            PromptInput {
                name: "texts".to_owned(),
                title: words.to_owned(),
                description: format!("{words}\n\tmore"),
                body: "{{t}} {{c}} {{y}} {{n}} {{s}}".to_owned(),
                variables,
                audience: Vec::new(),
            },
        )
        .expect("every one of these is text a prompt takes");
        let saved = store
            .insert_prompt(&a, &definition, at(0))
            .await
            .expect("insert");
        let who = viewer(&a, &[]);
        let got = store
            .get_prompt(&who, definition.id())
            .await
            .expect("get")
            .expect("there");
        assert_eq!(got.prompt(), &definition);
        assert_eq!(got, saved);
        assert_eq!(ids(&store, &who, PromptScope::Mine).await.len(), 1);
    })
    .await;
}
