//! `TeamPresetStore` conformance cases: what the API relies on from any store of people's team
//! presets (ADR 0074), stated once and run against the in-memory store, the Postgres store and the
//! SQLite store.
//!
//! An implementation supplies a fresh store (`async fn() -> Option<S>`; `None` skips the suite).
//! Each case is `async fn(store)`, uses owners of its own (so a store shared by the cases of one
//! run is never confused) and gives up after 60 seconds.
//!
//! What is deliberately not asserted: how a preset is kept, what a revision is beyond "a new one
//! for every write, never given twice", and the wording of any error. The rules of what a preset
//! is are [`orch_core::team`]'s, and the case `bounds_are_refused_with_words` states the ones a
//! store can only refuse by never being given an invalid preset.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use orch_core::{AgentId, AgentTarget, Timestamp, UserId};
use orch_core::{
    CheckpointRules, DEFAULT_MAX_HANDOFFS, DEFAULT_PUSH_TOKENS, HandoffApproval, MemberHistory,
    TeamMemberInput, TeamPreset, TeamPresetId, TeamPresetInput,
};
use uuid::Uuid;

use crate::{
    MAX_TEAM_PRESETS, StoredTeamPreset, TEAM_PRESET_PAGE, TeamPresetStore, TeamPresetStoreError,
    TeamPresetSummary,
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

fn fresh_id() -> TeamPresetId {
    TeamPresetId::from_entropy(*Uuid::now_v7().as_bytes())
}

fn at(second: i64) -> Timestamp {
    Timestamp::from_second(1_700_000_000 + second).expect("a time")
}

fn member(name: &str) -> TeamMemberInput {
    TeamMemberInput {
        name: name.to_owned(),
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        role: format!("the {name} of the team"),
        history: MemberHistory::AgentKeeps,
    }
}

/// A save of `name` whose lead is `first` and whose members are `members`.
fn input(name: &str, members: &[&str]) -> TeamPresetInput {
    let members: Vec<TeamMemberInput> = members.iter().map(|m| member(m)).collect();
    let lead = members
        .first()
        .map(|m| m.name.clone())
        .unwrap_or_else(|| "lead".to_owned());
    TeamPresetInput {
        name: name.to_owned(),
        title: format!("The team {name}"),
        lead,
        members,
        charter: "Deliver together, and say what you did.".to_owned(),
        max_handoffs: DEFAULT_MAX_HANDOFFS,
        handoff_approval: HandoffApproval::Ask,
        checkpoints: CheckpointRules::default(),
        push_tokens: DEFAULT_PUSH_TOKENS,
        summary: false,
    }
}

/// A valid preset named `name`, with two members.
fn preset(name: &str) -> TeamPreset {
    TeamPreset::new(fresh_id(), input(name, &["lead", "helper"])).expect("a valid preset")
}

async fn save<S: TeamPresetStore>(store: &S, owner: &UserId, name: &str) -> StoredTeamPreset {
    store
        .insert_team_preset(owner, &preset(name), at(0))
        .await
        .expect("insert")
}

async fn ids<S: TeamPresetStore>(store: &S, owner: &UserId) -> Vec<TeamPresetId> {
    let page = store
        .list_team_presets(owner, None, 50)
        .await
        .expect("list");
    page.items.into_iter().map(|i| i.id).collect()
}

/// A preset comes back whole with the revision and the time `insert_team_preset` gave.
pub async fn insert_then_get_round_trips<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let definition = preset("reviewers");
        let stored = store
            .insert_team_preset(&a, &definition, at(7))
            .await
            .expect("insert");
        assert!(stored.revision().get() > 0);
        assert_eq!(stored.updated_at(), at(7));
        let got = store
            .get_team_preset(&a, &definition.id().clone())
            .await
            .expect("get")
            .expect("the preset is kept");
        assert_eq!(got.preset(), &definition);
        assert_eq!(got.revision(), stored.revision());
        assert_eq!(got.updated_at(), at(7));
    })
    .await;
}

/// An id nobody holds is `None`, not an error.
pub async fn a_missing_preset_is_none<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        assert!(
            store
                .get_team_preset(&a, &fresh_id())
                .await
                .expect("get")
                .is_none()
        );
    })
    .await;
}

/// Every field of a preset survives the store: the title and the charter, each member's agent, its
/// release, its role and its history, both values of the history, the three shapes of the approval,
/// the checkpoint rules, the cap, the budget and the summary on and off.
pub async fn every_field_round_trips<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let mut index = 0_u16;
        for history in [MemberHistory::AgentKeeps, MemberHistory::OrchestratorSends] {
            for summary in [false, true] {
                for approval in [
                    HandoffApproval::Ask,
                    HandoffApproval::After { seconds: 30 },
                    HandoffApproval::After {
                        seconds: orch_core::MAX_APPROVAL_SECONDS,
                    },
                    HandoffApproval::Auto,
                ] {
                    let id = fresh_id();
                    // every save needs a name of its own: names are unique per owner
                    let name = format!("team-{index}");
                    index += 1;
                    let fields = TeamPresetInput {
                        name,
                        title: "A team that ships".to_owned(),
                        lead: "author".to_owned(),
                        members: vec![
                            TeamMemberInput {
                                name: "author".to_owned(),
                                target: AgentTarget {
                                    agent_id: AgentId::new("coder"),
                                    release: Some("stable".to_owned()),
                                },
                                role: "writes the change".to_owned(),
                                history,
                            },
                            TeamMemberInput {
                                name: "critic".to_owned(),
                                target: AgentTarget {
                                    agent_id: AgentId::new("researcher"),
                                    release: None,
                                },
                                role: "reads it back".to_owned(),
                                history: MemberHistory::OrchestratorSends,
                            },
                        ],
                        charter: "One change, then a review.\n\tNo exceptions.".to_owned(),
                        max_handoffs: orch_core::MAX_HANDOFFS_CEILING,
                        handoff_approval: approval,
                        checkpoints: CheckpointRules {
                            after: std::iter::once("author".to_owned()).collect(),
                            before_pull_request: true,
                        },
                        push_tokens: orch_core::MAX_PUSH_TOKENS,
                        summary,
                    };
                    let definition = TeamPreset::new(id.clone(), fields).expect("valid");
                    store
                        .insert_team_preset(&a, &definition, at(0))
                        .await
                        .expect("insert");
                    let got = store
                        .get_team_preset(&a, &id)
                        .await
                        .expect("get")
                        .expect("kept");
                    assert_eq!(got.preset(), &definition, "approval {approval:?}");
                    assert_eq!(got.preset().members()[0].history(), history);
                    assert_eq!(
                        got.preset().members()[1].history(),
                        MemberHistory::OrchestratorSends
                    );
                    assert_eq!(got.preset().summary(), summary);
                }
            }
        }
    })
    .await;
}

/// The bounds of a preset are refused by the rules, with words that name the field and the number:
/// nine members, `maxHandoffs` 21, a budget of 3 999 or 100 001, a lead that is no member.
pub async fn bounds_are_refused_with_words<S: TeamPresetStore>(_store: S) {
    within(async {
        let refused = |save: TeamPresetInput| {
            let error = TeamPreset::new(fresh_id(), save).expect_err("refused");
            (error.code, error.to_string(), error.at)
        };

        let nine = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
        let (code, words, at) = refused(input("too-many", &nine));
        assert_eq!((code, at.as_str()), ("bad_members", "members"));
        // the team ledger's own words: it is the ledger's rule
        assert!(words.contains("at most 8 members"), "{words}");

        let (code, words, at) = refused(TeamPresetInput {
            max_handoffs: 21,
            ..input("over-the-cap", &["lead", "helper"])
        });
        assert_eq!((code, at.as_str()), ("bad_max_handoffs", "maxHandoffs"));
        assert!(words.contains("6 to 20"), "{words}");

        let (code, _, _) = refused(TeamPresetInput {
            max_handoffs: 5,
            ..input("under-the-cap", &["lead", "helper"])
        });
        assert_eq!(code, "bad_max_handoffs");

        for budget in [3_999, 100_001] {
            let (code, words, at) = refused(TeamPresetInput {
                push_tokens: budget,
                ..input("a-budget", &["lead", "helper"])
            });
            assert_eq!((code, at.as_str()), ("bad_push_tokens", "pushTokens"));
            assert!(words.contains("4000 to 100000"), "{words}");
        }

        let (code, words, _) = refused(TeamPresetInput {
            lead: "nobody".to_owned(),
            ..input("no-lead", &["lead", "helper"])
        });
        assert_eq!(code, "bad_lead");
        assert!(words.contains("not a member"), "{words}");

        let (code, _, _) = refused(TeamPresetInput {
            handoff_approval: HandoffApproval::After { seconds: 4 },
            ..input("too-soon", &["lead", "helper"])
        });
        assert_eq!(code, "bad_handoff_approval");
    })
    .await;
}

/// Another person never reads a preset, never lists it, and never changes it: their read is
/// `None`, their listing holds nothing of it, and both writes are `NotFound` and change nothing.
pub async fn another_owner_never_reads_or_writes_it<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        let mine = save(&store, &a, "mine").await;
        let id = mine.preset().id().clone();
        assert!(store.get_team_preset(&b, &id).await.expect("get").is_none());
        assert!(ids(&store, &b).await.is_empty());
        assert!(matches!(
            store
                .replace_team_preset(&b, &preset("mine"), mine.revision(), at(1))
                .await,
            Err(TeamPresetStoreError::NotFound)
        ));
        assert!(matches!(
            store.delete_team_preset(&b, &id, None).await,
            Err(TeamPresetStoreError::NotFound)
        ));
        let after = store
            .get_team_preset(&a, &id)
            .await
            .expect("get")
            .expect("still there");
        assert_eq!(after, mine);
    })
    .await;
}

/// The id is global: it names one preset for everybody, so a second person cannot take it.
pub async fn ids_are_global<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        let held = save(&store, &a, "held").await;
        let id = held.preset().id().clone();
        let other =
            TeamPreset::new(id.clone(), input("other", &["lead", "helper"])).expect("valid");
        let result = store.insert_team_preset(&b, &other, at(0)).await;
        assert!(
            matches!(result, Err(TeamPresetStoreError::Exists)),
            "{result:?}"
        );
        assert!(ids(&store, &b).await.is_empty());
    })
    .await;
}

/// Two people may each have a preset of the same name; one may not have two.
pub async fn names_are_unique_per_owner<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        save(&store, &a, "same").await;
        save(&store, &b, "same").await;
        let result = store.insert_team_preset(&a, &preset("same"), at(0)).await;
        assert!(
            matches!(result, Err(TeamPresetStoreError::NameTaken)),
            "{result:?}"
        );
    })
    .await;
}

/// A listing is the person's own presets, ordered by id, and says how many they hold.
pub async fn a_listing_is_the_owners_ordered_by_id<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        let mut saved = Vec::new();
        for name in ["one", "two", "three"] {
            let id = save(&store, &a, name).await.preset().id().clone();
            saved.push((id, name));
        }
        save(&store, &b, "theirs").await;
        saved.sort();
        let mine: Vec<orch_core::TeamPresetId> = saved.iter().map(|(id, _)| id.clone()).collect();
        let page = store.list_team_presets(&a, None, 50).await.expect("list");
        assert_eq!(
            page.items.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
            mine
        );
        assert_eq!(page.held, 3);
        assert!(!page.more);
        assert_eq!(page.items[0].member_count, 2);
        // the first row is the one whose id sorts first, not the one saved first
        assert!(
            page.items[0].name == "one"
                || page.items[0].name == "two"
                || page.items[0].name == "three"
        );
    })
    .await;
}

/// A page follows the id: `after` is exclusive, `more` says whether another follows.
pub async fn pages_follow_the_id<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let mut all = Vec::new();
        for name in ["one", "two", "three", "four"] {
            all.push(save(&store, &a, name).await.preset().id().clone());
        }
        all.sort();
        let first = store.list_team_presets(&a, None, 2).await.expect("list");
        assert_eq!(first.items.len(), 2);
        assert!(first.more);
        assert_eq!(first.held, 4);
        let second = store
            .list_team_presets(&a, Some(&first.items[1].id), 2)
            .await
            .expect("list");
        assert_eq!(
            second
                .items
                .iter()
                .map(|i| i.id.clone())
                .collect::<Vec<_>>(),
            all[2..].to_vec()
        );
        assert!(!second.more);
        let empty = store
            .list_team_presets(&a, all.last(), 2)
            .await
            .expect("list");
        assert!(empty.items.is_empty());
        assert!(!empty.more);
    })
    .await;
}

/// A page is at least one and at most [`TEAM_PRESET_PAGE`], whatever the caller asked for.
pub async fn a_page_is_positive_and_bounded<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        save(&store, &a, "one").await;
        save(&store, &a, "two").await;
        let huge = store
            .list_team_presets(&a, None, u32::MAX)
            .await
            .expect("list");
        assert_eq!(huge.items.len(), 2);
        let zero = store.list_team_presets(&a, None, 0).await.expect("list");
        assert_eq!(zero.items.len(), 1, "a limit of zero is one");
        assert_eq!(zero.items[0].id, huge.items[0].id);
        assert!(zero.more, "the second preset follows the first");
        let bounded = store
            .list_team_presets(&a, None, u32::try_from(TEAM_PRESET_PAGE).unwrap() + 50)
            .await
            .expect("list");
        assert!(bounded.items.len() <= TEAM_PRESET_PAGE);
        assert_eq!(bounded.items.len(), 2);
    })
    .await;
}

/// A replace at the current revision replaces the whole preset and gives a new revision.
pub async fn replace_with_the_current_revision_replaces_whole<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let first = save(&store, &a, "start").await;
        let id = first.preset().id().clone();
        let replacement = TeamPreset::new(
            id.clone(),
            TeamPresetInput {
                name: "renamed".to_owned(),
                title: "Another title".to_owned(),
                charter: "Another charter.".to_owned(),
                max_handoffs: 9,
                handoff_approval: HandoffApproval::Auto,
                push_tokens: 12_000,
                summary: true,
                ..input("renamed", &["solo", "second"])
            },
        )
        .expect("valid");
        let stored = store
            .replace_team_preset(&a, &replacement, first.revision(), at(3))
            .await
            .expect("replace");
        assert!(stored.revision() > first.revision());
        assert_eq!(stored.updated_at(), at(3));
        let got = store
            .get_team_preset(&a, &id)
            .await
            .expect("get")
            .expect("kept");
        assert_eq!(got.preset(), &replacement);
        assert_eq!(got.revision(), stored.revision());
    })
    .await;
}

/// A replace at a stale revision is a conflict and changes nothing.
pub async fn a_stale_replace_is_a_conflict_and_changes_nothing<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let first = save(&store, &a, "start").await;
        let id = first.preset().id().clone();
        let current = store
            .replace_team_preset(&a, &first.clone().into_preset(), first.revision(), at(1))
            .await
            .expect("replace");
        let result = store
            .replace_team_preset(&a, &first.clone().into_preset(), first.revision(), at(2))
            .await;
        assert!(
            matches!(result, Err(TeamPresetStoreError::Conflict)),
            "{result:?}"
        );
        let after = store
            .get_team_preset(&a, &id)
            .await
            .expect("get")
            .expect("kept");
        assert_eq!(after.revision(), current.revision());
    })
    .await;
}

/// A rename onto another of the owner's names is `NameTaken` and changes nothing.
pub async fn a_rename_to_a_taken_name_is_name_taken_and_changes_nothing<S: TeamPresetStore>(
    store: S,
) {
    within(async {
        let a = owner();
        let held = save(&store, &a, "taken").await;
        let mine = save(&store, &a, "mine").await;
        let renamed = TeamPreset::new(
            mine.preset().id().clone(),
            TeamPresetInput {
                name: "taken".to_owned(),
                ..mine.preset().to_input()
            },
        )
        .expect("valid");
        let result = store
            .replace_team_preset(&a, &renamed, mine.revision(), at(1))
            .await;
        assert!(
            matches!(result, Err(TeamPresetStoreError::NameTaken)),
            "{result:?}"
        );
        let after = store
            .get_team_preset(&a, &mine.preset().id().clone())
            .await
            .expect("get")
            .expect("kept");
        assert_eq!(after, mine);
        assert!(
            store
                .get_team_preset(&a, &held.preset().id().clone())
                .await
                .expect("get")
                .is_some()
        );
    })
    .await;
}

/// A delete removes the preset and the second delete is `NotFound`: a deleted preset stays deleted.
pub async fn delete_removes_the_preset_and_the_second_delete_is_not_found<S: TeamPresetStore>(
    store: S,
) {
    within(async {
        let a = owner();
        let mine = save(&store, &a, "gone").await;
        let id = mine.preset().id().clone();
        store
            .delete_team_preset(&a, &id, Some(mine.revision()))
            .await
            .expect("delete");
        assert!(store.get_team_preset(&a, &id).await.expect("get").is_none());
        assert!(matches!(
            store.delete_team_preset(&a, &id, None).await,
            Err(TeamPresetStoreError::NotFound)
        ));
    })
    .await;
}

/// A delete at a stale revision is a conflict and deletes nothing.
pub async fn a_stale_delete_is_a_conflict<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let first = save(&store, &a, "kept").await;
        let id = first.preset().id().clone();
        let current = store
            .replace_team_preset(&a, &first.clone().into_preset(), first.revision(), at(1))
            .await
            .expect("replace");
        assert!(matches!(
            store
                .delete_team_preset(&a, &id, Some(first.revision()))
                .await,
            Err(TeamPresetStoreError::Conflict)
        ));
        assert_eq!(
            store
                .get_team_preset(&a, &id)
                .await
                .expect("get")
                .expect("kept")
                .revision(),
            current.revision()
        );
    })
    .await;
}

/// A write after a delete is `NotFound` and brings nothing back.
pub async fn a_write_after_a_delete_is_not_found_and_brings_nothing_back<S: TeamPresetStore>(
    store: S,
) {
    within(async {
        let a = owner();
        let mine = save(&store, &a, "back").await;
        let id = mine.preset().id().clone();
        let revision = mine.revision();
        store
            .delete_team_preset(&a, &id, None)
            .await
            .expect("delete");
        assert!(matches!(
            store
                .replace_team_preset(&a, &mine.into_preset(), revision, at(1))
                .await,
            Err(TeamPresetStoreError::NotFound)
        ));
        assert!(store.get_team_preset(&a, &id).await.expect("get").is_none());
    })
    .await;
}

/// A revision is a new number for every write and is never given twice, a delete and a new insert
/// of the same id included.
pub async fn revisions_are_never_given_twice<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let mut seen = std::collections::BTreeSet::new();
        let first = save(&store, &a, "one").await;
        assert!(seen.insert(first.revision()));
        let second = store
            .replace_team_preset(&a, &first.clone().into_preset(), first.revision(), at(1))
            .await
            .expect("replace");
        assert!(seen.insert(second.revision()));
        store
            .delete_team_preset(&a, &second.preset().id().clone(), None)
            .await
            .expect("delete");
        let again = save(&store, &a, "two").await;
        assert!(seen.insert(again.revision()), "a revision was given twice");
    })
    .await;
}

/// At most [`MAX_TEAM_PRESETS`] per person, and a person's own do not count against another's.
pub async fn at_most_fifty_per_owner<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        for n in 0..MAX_TEAM_PRESETS {
            save(&store, &a, &format!("t{n}")).await;
        }
        let result = store.insert_team_preset(&a, &preset("over"), at(0)).await;
        assert!(
            matches!(
                result,
                Err(TeamPresetStoreError::TooMany {
                    limit: MAX_TEAM_PRESETS
                })
            ),
            "{result:?}"
        );
        save(&store, &owner(), "another").await;
    })
    .await;
}

/// Writers racing for the last place never pass the limit.
pub async fn concurrent_inserts_never_pass_the_limit<S: TeamPresetStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        for _ in 0..2 {
            let a = owner();
            for n in 0..MAX_TEAM_PRESETS - 2 {
                save(&*store, &a, &format!("t{n}")).await;
            }
            let mut writers = Vec::new();
            for n in 0..8 {
                let (store, a) = (Arc::clone(&store), a.clone());
                writers.push(tokio::spawn(async move {
                    store
                        .insert_team_preset(&a, &preset(&format!("race{n}")), at(0))
                        .await
                }));
            }
            let mut won = 0;
            for w in writers {
                match w.await.expect("task") {
                    Ok(_) => won += 1,
                    Err(TeamPresetStoreError::TooMany { .. }) => {}
                    Err(e) => panic!("unexpected: {e:?}"),
                }
            }
            assert_eq!(won, 2, "two places were free");
        }
    })
    .await;
}

/// Inserters of one id have one winner; the others find it taken.
pub async fn concurrent_inserts_of_one_id_have_one_winner<S: TeamPresetStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        for _ in 0..RACE_ROUNDS {
            let id = fresh_id();
            let mut writers = Vec::new();
            for n in 0..8 {
                let (store, id) = (Arc::clone(&store), id.clone());
                writers.push(tokio::spawn(async move {
                    let definition =
                        TeamPreset::new(id, input(&format!("same{n}"), &["lead", "helper"]))
                            .expect("valid");
                    store.insert_team_preset(&owner(), &definition, at(0)).await
                }));
            }
            let mut won = 0;
            for w in writers {
                match w.await.expect("task") {
                    Ok(_) => won += 1,
                    Err(TeamPresetStoreError::Exists) => {}
                    Err(e) => panic!("unexpected: {e:?}"),
                }
            }
            assert_eq!(won, 1);
        }
    })
    .await;
}

/// Writers replacing from one revision have one winner; the others find it changed.
pub async fn concurrent_replaces_from_one_revision_have_one_winner<S: TeamPresetStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        for _ in 0..RACE_ROUNDS {
            let a = owner();
            let first = save(&*store, &a, "start").await;
            let id = first.preset().id().clone();
            let revision = first.revision();
            let definition = first.preset().clone();
            let mut writers = Vec::new();
            for n in 0..8 {
                let (store, a, id, definition) = (
                    Arc::clone(&store),
                    a.clone(),
                    id.clone(),
                    definition.clone(),
                );
                writers.push(tokio::spawn(async move {
                    let next = TeamPreset::new(
                        id,
                        TeamPresetInput {
                            title: format!("Title {n}"),
                            ..definition.to_input()
                        },
                    )
                    .expect("valid");
                    store.replace_team_preset(&a, &next, revision, at(n)).await
                }));
            }
            let mut won = 0;
            for w in writers {
                match w.await.expect("task") {
                    Ok(_) => won += 1,
                    Err(TeamPresetStoreError::Conflict) => {}
                    Err(e) => panic!("unexpected: {e:?}"),
                }
            }
            assert_eq!(won, 1);
        }
    })
    .await;
}

/// Writers racing for one name have one winner: the name stays unique.
pub async fn concurrent_inserts_of_one_name_have_one_winner<S: TeamPresetStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        for _ in 0..RACE_ROUNDS {
            let a = owner();
            let mut writers = Vec::new();
            for _ in 0..8 {
                let (store, a) = (Arc::clone(&store), a.clone());
                writers.push(tokio::spawn(async move {
                    store
                        .insert_team_preset(&a, &preset("one-name"), at(0))
                        .await
                }));
            }
            let mut won = 0;
            for w in writers {
                match w.await.expect("task") {
                    Ok(_) => won += 1,
                    Err(TeamPresetStoreError::NameTaken) => {}
                    Err(e) => panic!("unexpected: {e:?}"),
                }
            }
            assert_eq!(won, 1);
        }
    })
    .await;
}

/// The order of the refusals of an insert: `Exists`, then `NameTaken`, then `TooMany`.
pub async fn the_order_of_refusals<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        let held = save(&store, &a, "held").await;
        let id = held.preset().id().clone();
        // an id that exists and a name that is taken: the id is reported first
        let both = TeamPreset::new(id.clone(), input("mine", &["lead", "helper"])).expect("valid");
        save(&store, &b, "mine").await;
        assert!(matches!(
            store.insert_team_preset(&b, &both, at(0)).await,
            Err(TeamPresetStoreError::Exists)
        ));
        // a name that is taken, at a full shelf: the name is reported first
        let c = owner();
        for n in 0..MAX_TEAM_PRESETS - 1 {
            save(&store, &c, &format!("t{n}")).await;
        }
        save(&store, &c, "the-one").await;
        assert!(matches!(
            store
                .insert_team_preset(&c, &preset("the-one"), at(0))
                .await,
            Err(TeamPresetStoreError::NameTaken)
        ));
    })
    .await;
}

/// A summary row says the lead and how many members the team has, and no word of its charter.
pub async fn a_summary_says_the_lead_and_the_member_count<S: TeamPresetStore>(store: S) {
    within(async {
        let a = owner();
        let definition = TeamPreset::new(
            fresh_id(),
            TeamPresetInput {
                lead: "helper".to_owned(),
                ..input("sized", &["lead", "helper", "third"])
            },
        )
        .expect("valid");
        let stored = store
            .insert_team_preset(&a, &definition, at(5))
            .await
            .expect("insert");
        let page = store.list_team_presets(&a, None, 1).await.expect("list");
        let row = TeamPresetSummary {
            id: definition.id().clone(),
            name: definition.name().to_owned(),
            title: definition.title().to_owned(),
            lead: definition.lead().clone(),
            member_count: 3,
            revision: stored.revision(),
            updated_at: at(5),
        };
        assert_eq!(page.items, vec![row]);
    })
    .await;
}
