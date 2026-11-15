//! `ThreadSearch` conformance cases (ADR 0068). Each is `async fn(store)` on a fresh, isolated store
//! that is also a [`ThreadStore`]: the index is written by the commits of the store it is read from.
//! The cases that play a log older than the index take a second argument, an `async fn(&S)` that makes
//! the store forget its index (it deletes the rows and marks no thread as gone through).

use jiff::{SignedDuration, Timestamp};
use orch_core::search::{MAX_MESSAGE_HITS, SearchRole, SearchTerms};
use orch_core::{
    Actor, AgentId, AgentMessageData, AgentTarget, EventBody, ForkKind, MessagePurpose, ThreadId,
    ThreadState, UserId, UserMessageData,
};
use uuid::Uuid;

use crate::{
    ArchivedFilter, Arrangement, Commit, CommitOutcome, ForkOrigin, NewEvent, NewThreadRecord,
    SearchPosition, SearchRank, SearchRequest, SearchRows, ThreadSearch, ThreadStore,
};

/// A store that is both the log and the index.
pub trait LogAndIndex: ThreadStore + ThreadSearch {}
impl<S: ThreadStore + ThreadSearch> LogAndIndex for S {}

fn t0() -> Timestamp {
    "2026-01-15T10:00:00Z".parse().unwrap()
}

fn at(secs: i64) -> Timestamp {
    t0().checked_add(SignedDuration::from_secs(secs)).unwrap()
}

fn thread_id(n: u128) -> ThreadId {
    ThreadId(Uuid::from_u128(
        0x0190_0000_0000_7000_8000_0000_0000_0000 + n,
    ))
}

fn alice() -> UserId {
    UserId::new("alice@example.com")
}

fn bob() -> UserId {
    UserId::new("bob@example.com")
}

fn new_thread(owner: &UserId, n: u128, title: &str, description: Option<&str>) -> NewThreadRecord {
    NewThreadRecord {
        id: thread_id(n),
        owner: owner.clone(),
        title: title.to_owned(),
        description: description.map(Into::into),
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        context_id: None,
        rail_parent: None,
        now: t0(),
    }
}

fn say(owner: &UserId, text: &str) -> NewEvent {
    NewEvent {
        at: t0(),
        actor: Actor::user(owner),
        body: EventBody::UserMessage(UserMessageData::new(text)),
        idempotency_key: None,
    }
}

fn reply(id: &str, text: &str, is_final: bool, purpose: Option<MessagePurpose>) -> NewEvent {
    NewEvent {
        at: t0(),
        actor: Actor::agent(&AgentId::new("coder"), None),
        body: EventBody::AgentMessage(AgentMessageData {
            text: text.to_owned(),
            message_id: id.to_owned(),
            is_final,
            purpose,
            via: None,
        }),
        idempotency_key: None,
    }
}

fn commit(events: Vec<NewEvent>) -> Commit {
    Commit {
        new_state: ThreadState::Working,
        job: None,
        events,
        outbox: Vec::new(),
        binding: None,
        now: at(10),
        lease: None,
        watches: Vec::new(),
        timers: Vec::new(),
        inbox: None,
        finishes_outbox: None,
        title: None,
        description: None,
        sharing: None,
        skip_unsent_delegates: false,
    }
}

/// Creates thread `n` of `owner` with `events` and returns its version.
async fn open<S: LogAndIndex>(
    store: &S,
    owner: &UserId,
    n: u128,
    title: &str,
    description: Option<&str>,
    events: Vec<NewEvent>,
) -> i64 {
    store
        .create_thread(new_thread(owner, n, title, description), commit(events))
        .await
        .unwrap()
        .0
        .version
}

async fn append<S: LogAndIndex>(store: &S, n: u128, version: i64, events: Vec<NewEvent>) -> i64 {
    match store
        .commit(thread_id(n), version, commit(events))
        .await
        .unwrap()
    {
        CommitOutcome::Applied { thread, .. } => thread.version,
        other => panic!("expected the commit to apply, got {other:?}"),
    }
}

fn request(q: &str) -> SearchRequest {
    SearchRequest {
        terms: SearchTerms::parse(q).unwrap(),
        archived: ArchivedFilter::Include,
        include_edits: false,
        after: None,
        limit: 50,
    }
}

async fn find<S: LogAndIndex>(store: &S, owner: &UserId, q: &str) -> SearchRows {
    store.search_threads(owner, &request(q)).await.unwrap()
}

fn ids(rows: &SearchRows) -> Vec<ThreadId> {
    rows.rows.iter().map(|r| r.thread.id).collect()
}

// ---- cases ------------------------------------------------------------------------------------

/// A title is rank 2, a description rank 1, a message rank 0; the messages carry their seq, role,
/// time and words.
pub async fn search_finds_title_description_and_message<S: LogAndIndex>(store: S) {
    open(
        &store,
        &alice(),
        1,
        "Quarterly report",
        None,
        vec![say(&alice(), "hello")],
    )
    .await;
    open(
        &store,
        &alice(),
        2,
        "Other",
        Some("the report for the board"),
        vec![say(&alice(), "hello")],
    )
    .await;
    open(
        &store,
        &alice(),
        3,
        "Third",
        None,
        vec![
            say(&alice(), "is the report ready?"),
            reply(
                "m1",
                "The report is ready",
                true,
                Some(MessagePurpose::Answer),
            ),
        ],
    )
    .await;
    open(
        &store,
        &alice(),
        4,
        "Nothing",
        None,
        vec![say(&alice(), "no match")],
    )
    .await;

    let rows = find(&store, &alice(), "report").await;
    assert_eq!(ids(&rows), vec![thread_id(1), thread_id(2), thread_id(3)]);
    assert!(!rows.more);
    let ranks: Vec<SearchRank> = rows.rows.iter().map(|r| r.rank).collect();
    assert_eq!(
        ranks,
        vec![
            SearchRank::Title,
            SearchRank::Description,
            SearchRank::Messages
        ]
    );
    assert_eq!(rows.rows[0].thread.title, "Quarterly report");
    assert_eq!(
        rows.rows[1].thread.description.as_deref(),
        Some("the report for the board")
    );
    let messages = &rows.rows[2].messages;
    assert_eq!(rows.rows[2].message_hits, 2);
    assert_eq!(
        messages
            .iter()
            .map(|m| (m.seq, m.role, m.text.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (2, SearchRole::Agent, "The report is ready"),
            (1, SearchRole::Person, "is the report ready?"),
        ]
    );
    assert_eq!(messages[0].at, t0());
    // a prefix finds the word, the middle of a word does not
    assert_eq!(ids(&find(&store, &alice(), "repo").await).len(), 3);
    assert!(find(&store, &alice(), "port").await.rows.is_empty());
    // every term must be in one document
    assert_eq!(
        ids(&find(&store, &alice(), "report board").await),
        vec![thread_id(2)]
    );
    assert!(find(&store, &alice(), "report zebra").await.rows.is_empty());
    // no term, nothing
    assert!(find(&store, &alice(), "a").await.rows.is_empty());
}

/// Only the owner's own words: another person's threads are never found, by title or by content.
pub async fn search_is_the_owners<S: LogAndIndex>(store: S) {
    open(
        &store,
        &alice(),
        1,
        "Pineapple plans",
        None,
        vec![say(&alice(), "pineapple one")],
    )
    .await;
    open(
        &store,
        &bob(),
        2,
        "Pineapple notes",
        Some("pineapple"),
        vec![say(&bob(), "pineapple two")],
    )
    .await;
    let a = find(&store, &alice(), "pineapple").await;
    assert_eq!(ids(&a), vec![thread_id(1)]);
    let b = find(&store, &bob(), "pineapple").await;
    assert_eq!(ids(&b), vec![thread_id(2)]);
    let nobody = find(&store, &UserId::new("carol@example.com"), "pineapple").await;
    assert!(nobody.rows.is_empty());
}

/// A thread that was erased is gone from the results at once, with its words.
pub async fn deleted_thread_leaves_no_hit<S: LogAndIndex>(store: S) {
    let v = open(
        &store,
        &alice(),
        1,
        "Walrus",
        Some("walrus"),
        vec![say(&alice(), "walrus")],
    )
    .await;
    open(
        &store,
        &alice(),
        2,
        "Keeper",
        None,
        vec![say(&alice(), "walrus too")],
    )
    .await;
    assert_eq!(find(&store, &alice(), "walrus").await.rows.len(), 2);
    store
        .delete_threads(&alice(), &[(thread_id(1), v)], at(60))
        .await
        .unwrap();
    assert_eq!(
        ids(&find(&store, &alice(), "walrus").await),
        vec![thread_id(2)]
    );
    // a thread made with the same id afterwards starts with nothing of the old one
    open(
        &store,
        &alice(),
        1,
        "Fresh",
        None,
        vec![say(&alice(), "nothing")],
    )
    .await;
    assert_eq!(
        ids(&find(&store, &alice(), "walrus").await),
        vec![thread_id(2)]
    );
}

/// The events a fork copied are its parent's words while the parent exists, and its own once it is
/// gone; what it said itself always counts.
pub async fn fork_copied_events_do_not_hit_until_parent_deleted<S: LogAndIndex>(store: S) {
    let v = open(
        &store,
        &alice(),
        1,
        "Parent",
        None,
        vec![say(&alice(), "zebra one"), say(&alice(), "zebra two")],
    )
    .await;
    let (fork, _) = store
        .fork_thread(
            new_thread(&alice(), 2, "Fork", None),
            ForkOrigin {
                parent: thread_id(1),
                cut: 2,
                kind: ForkKind::Fork,
            },
            commit(vec![say(&alice(), "a new zebra")]),
        )
        .await
        .unwrap();
    assert_eq!(fork.last_seq, 3);
    let rows = find(&store, &alice(), "zebra").await;
    assert_eq!(ids(&rows), vec![thread_id(2), thread_id(1)]);
    let fork_row = &rows.rows[0];
    assert_eq!(fork_row.message_hits, 1);
    assert_eq!(
        fork_row.messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
        vec![3]
    );
    assert_eq!(rows.rows[1].message_hits, 2);
    // the fork goes on, and its own words go on counting
    append(&store, 2, fork.version, vec![say(&alice(), "more zebra")]).await;
    let rows = find(&store, &alice(), "zebra").await;
    assert_eq!(rows.rows[0].message_hits, 2);
    // the parent is erased: the fork stands alone and its copied words are its own
    store
        .delete_threads(&alice(), &[(thread_id(1), v)], at(60))
        .await
        .unwrap();
    let rows = find(&store, &alice(), "zebra").await;
    assert_eq!(ids(&rows), vec![thread_id(2)]);
    assert_eq!(rows.rows[0].message_hits, 4);
}

/// A fork cut before the parent replaced a message has the message as it was: the copy is indexed
/// from the fork's own log, not from what the parent's index holds now.
pub async fn a_fork_has_the_message_the_parent_later_replaced<S: LogAndIndex>(store: S) {
    let v = open(
        &store,
        &alice(),
        1,
        "Parent",
        None,
        vec![
            say(&alice(), "question"),
            reply("m1", "first draft says walrus", true, None),
        ],
    )
    .await;
    let v = append(
        &store,
        1,
        v,
        vec![reply("m1", "second draft says otter", true, None)],
    )
    .await;
    assert!(find(&store, &alice(), "walrus").await.rows.is_empty());
    store
        .fork_thread(
            new_thread(&alice(), 2, "Fork", None),
            ForkOrigin {
                parent: thread_id(1),
                cut: 2,
                kind: ForkKind::Fork,
            },
            commit(vec![say(&alice(), "go on")]),
        )
        .await
        .unwrap();
    // the parent exists: the copy is its words, not the fork's
    assert!(find(&store, &alice(), "walrus").await.rows.is_empty());
    store
        .delete_threads(&alice(), &[(thread_id(1), v)], at(60))
        .await
        .unwrap();
    let rows = find(&store, &alice(), "walrus").await;
    assert_eq!(ids(&rows), vec![thread_id(2)]);
    assert_eq!(rows.rows[0].messages[0].seq, 2);
    assert!(find(&store, &alice(), "otter").await.rows.is_empty());
}

/// A thread made by an edit of a message is a branch: searched only when asked for.
pub async fn edits_only_with_branches<S: LogAndIndex>(store: S) {
    open(
        &store,
        &alice(),
        1,
        "Parent",
        None,
        vec![say(&alice(), "zebra original")],
    )
    .await;
    store
        .fork_thread(
            new_thread(&alice(), 2, "Edited", None),
            ForkOrigin {
                parent: thread_id(1),
                cut: 0,
                kind: ForkKind::Edit,
            },
            commit(vec![say(&alice(), "zebra edited")]),
        )
        .await
        .unwrap();
    assert_eq!(
        ids(&find(&store, &alice(), "zebra").await),
        vec![thread_id(1)]
    );
    let mut asked = request("zebra");
    asked.include_edits = true;
    let rows = store.search_threads(&alice(), &asked).await.unwrap();
    assert_eq!(ids(&rows), vec![thread_id(2), thread_id(1)]);
}

/// `archived`: exclude, only, include.
pub async fn archived_filters<S: LogAndIndex>(store: S) {
    open(
        &store,
        &alice(),
        1,
        "Zebra live",
        None,
        vec![say(&alice(), "hello")],
    )
    .await;
    open(
        &store,
        &alice(),
        2,
        "Zebra old",
        None,
        vec![say(&alice(), "hello")],
    )
    .await;
    store
        .arrange_thread(
            &alice(),
            thread_id(2),
            Arrangement {
                archived: Some(true),
                ..Arrangement::default()
            },
            at(20),
        )
        .await
        .unwrap();
    let with = |archived| SearchRequest {
        archived,
        ..request("zebra")
    };
    let all = store
        .search_threads(&alice(), &with(ArchivedFilter::Include))
        .await
        .unwrap();
    assert_eq!(ids(&all), vec![thread_id(2), thread_id(1)]);
    let live = store
        .search_threads(&alice(), &with(ArchivedFilter::Exclude))
        .await
        .unwrap();
    assert_eq!(ids(&live), vec![thread_id(1)]);
    let old = store
        .search_threads(&alice(), &with(ArchivedFilter::Only))
        .await
        .unwrap();
    assert_eq!(ids(&old), vec![thread_id(2)]);
}

/// A later final message with the same id replaces the earlier one: it is found once, as itself.
pub async fn replaced_final_message_hits_once<S: LogAndIndex>(store: S) {
    let v = open(
        &store,
        &alice(),
        1,
        "Drafts",
        None,
        vec![
            say(&alice(), "question"),
            reply("m1", "first draft mentions walrus", true, None),
        ],
    )
    .await;
    assert_eq!(find(&store, &alice(), "walrus").await.rows.len(), 1);
    let v = append(
        &store,
        1,
        v,
        vec![reply("m1", "second draft mentions otter", true, None)],
    )
    .await;
    assert!(find(&store, &alice(), "walrus").await.rows.is_empty());
    let rows = find(&store, &alice(), "otter").await;
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(
        (rows.rows[0].message_hits, rows.rows[0].messages[0].seq),
        (1, 3)
    );
    // a later final that is working text takes the message out: it is not the agent's answer
    append(
        &store,
        1,
        v,
        vec![reply(
            "m1",
            "now only working words",
            true,
            Some(MessagePurpose::Working),
        )],
    )
    .await;
    assert!(find(&store, &alice(), "otter").await.rows.is_empty());
    assert!(find(&store, &alice(), "working").await.rows.is_empty());
}

/// Partial messages and working text are not the agent's words; other events carry no words.
pub async fn partial_and_working_messages_never_hit<S: LogAndIndex>(store: S) {
    open(
        &store,
        &alice(),
        1,
        "Fix",
        None,
        vec![
            say(&alice(), "please look"),
            reply("a", "a partial walrus", false, None),
            reply("b", "working walrus", true, Some(MessagePurpose::Working)),
            reply("c", "the final answer", true, Some(MessagePurpose::Answer)),
        ],
    )
    .await;
    assert!(find(&store, &alice(), "walrus").await.rows.is_empty());
    assert_eq!(find(&store, &alice(), "answer").await.rows.len(), 1);
}

/// More than three messages match: three are shown, newest first, and all are counted.
pub async fn the_newest_three_messages_are_shown_and_all_are_counted<S: LogAndIndex>(store: S) {
    let events: Vec<NewEvent> = (1..=5)
        .map(|n| say(&alice(), &format!("zebra number {n}")))
        .collect();
    open(&store, &alice(), 1, "Notes", None, events).await;
    let rows = find(&store, &alice(), "zebra").await;
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(rows.rows[0].message_hits, 5);
    assert_eq!(MAX_MESSAGE_HITS, 3);
    assert_eq!(
        rows.rows[0]
            .messages
            .iter()
            .map(|m| m.seq)
            .collect::<Vec<_>>(),
        vec![5, 4, 3]
    );
}

/// A new title or description is searched from the commit that wrote it, and the old words are gone.
pub async fn a_rename_is_searched_from_its_commit<S: LogAndIndex>(store: S) {
    let v = open(
        &store,
        &alice(),
        1,
        "Walrus",
        Some("about walrus"),
        vec![say(&alice(), "hello")],
    )
    .await;
    assert_eq!(
        find(&store, &alice(), "walrus").await.rows[0].rank,
        SearchRank::Title
    );
    let mut renamed = commit(Vec::new());
    renamed.title = Some("Otter".to_owned());
    renamed.description = Some("about otters".to_owned());
    store.commit(thread_id(1), v, renamed).await.unwrap();
    assert!(find(&store, &alice(), "walrus").await.rows.is_empty());
    let rows = find(&store, &alice(), "otter").await;
    assert_eq!(rows.rows[0].rank, SearchRank::Title);
    let mut cleared = commit(Vec::new());
    cleared.description = Some(String::new());
    store.commit(thread_id(1), 2, cleared).await.unwrap();
    assert!(find(&store, &alice(), "otters").await.rows.is_empty());
}

/// Accents, ligatures, Cyrillic, CJK and digits survive the round trip through the index.
pub async fn keys_roundtrip_cyrillic_cjk_digits<S: LogAndIndex>(store: S) {
    open(
        &store,
        &alice(),
        1,
        "Caf\u{e9} menu",
        None,
        vec![say(&alice(), "Le c\u{153}ur de la carte")],
    )
    .await;
    open(
        &store,
        &alice(),
        2,
        "\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442} \u{43c}\u{438}\u{440}",
        None,
        vec![say(&alice(), "\u{1f389} party \u{6771}\u{4eac} today")],
    )
    .await;
    open(
        &store,
        &alice(),
        3,
        "Invoice 2026-11",
        Some("v1.2 release"),
        vec![say(&alice(), "total 120")],
    )
    .await;
    let cases: [(&str, u128, SearchRank); 8] = [
        ("cafe", 1, SearchRank::Title),
        ("C\u{152}UR", 1, SearchRank::Messages),
        ("\u{43f}\u{440}\u{438}\u{432}", 2, SearchRank::Title),
        ("\u{6771}\u{4eac}", 2, SearchRank::Messages),
        ("party", 2, SearchRank::Messages),
        ("2026", 3, SearchRank::Title),
        ("v1", 3, SearchRank::Description),
        ("120", 3, SearchRank::Messages),
    ];
    for (q, n, rank) in cases {
        let rows = find(&store, &alice(), q).await;
        assert_eq!(ids(&rows), vec![thread_id(n)], "{q}");
        assert_eq!(rows.rows[0].rank, rank, "{q}");
    }
}

/// Pages are stable: seven threads, pages of three, each once, in order; and a later page starts
/// after the position of the last result of the page before.
pub async fn paging_is_stable<S: LogAndIndex>(store: S) {
    for n in 1..=7 {
        let title = if n % 3 == 0 { "Zebra title" } else { "Other" };
        open(
            &store,
            &alice(),
            n,
            title,
            None,
            vec![say(&alice(), "a zebra in a message")],
        )
        .await;
    }
    // rank 2 (3 and 6, newest first), then rank 0 (7, 5, 4, 2, 1)
    let want = [6, 3, 7, 5, 4, 2, 1].map(thread_id);
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = store
            .search_threads(
                &alice(),
                &SearchRequest {
                    limit: 3,
                    after,
                    ..request("zebra")
                },
            )
            .await
            .unwrap();
        assert!(page.rows.len() <= 3);
        seen.extend(ids(&page));
        if !page.more {
            break;
        }
        let last = page.rows.last().unwrap();
        after = Some(SearchPosition {
            rank: last.rank,
            thread: last.thread.id,
        });
    }
    assert_eq!(seen, want);
}

/// A commit to a thread the index has not been through catches the whole log up, then indexes its
/// own events.
pub async fn a_commit_catches_up_with_the_log_the_index_has_not_been_through<S: LogAndIndex>(
    store: S,
    forget: impl AsyncFnOnce(&S),
) {
    let v = open(
        &store,
        &alice(),
        1,
        "Walrus",
        Some("walrus"),
        vec![say(&alice(), "walrus one")],
    )
    .await;
    let v = append(&store, 1, v, vec![reply("m1", "walrus two", true, None)]).await;
    forget(&store).await;
    assert!(
        find(&store, &alice(), "walrus").await.rows.is_empty(),
        "nothing is indexed"
    );
    append(&store, 1, v, vec![say(&alice(), "walrus three")]).await;
    let rows = find(&store, &alice(), "walrus").await;
    assert_eq!(rows.rows[0].rank, SearchRank::Title);
    assert_eq!(rows.rows[0].message_hits, 3);
}

/// The backfill indexes a log the index was never written for, a page at a time, and running it
/// again changes nothing.
pub async fn backfill_indexes_old_events_and_is_idempotent<S: LogAndIndex>(
    store: S,
    forget: impl AsyncFnOnce(&S),
) {
    for n in 1..=3 {
        let v = open(
            &store,
            &alice(),
            n,
            "Walrus",
            None,
            vec![say(&alice(), "hello")],
        )
        .await;
        append(
            &store,
            n,
            v,
            vec![reply("m1", "an otter answer", true, None)],
        )
        .await;
    }
    // a fork made by an older build: its copied events are in its log too
    store
        .fork_thread(
            new_thread(&alice(), 4, "Fork", None),
            ForkOrigin {
                parent: thread_id(1),
                cut: 2,
                kind: ForkKind::Fork,
            },
            commit(vec![say(&alice(), "otter in the fork")]),
        )
        .await
        .unwrap();
    let before_otter = find(&store, &alice(), "otter").await;
    let before_walrus = find(&store, &alice(), "walrus").await;
    forget(&store).await;
    assert!(find(&store, &alice(), "otter").await.rows.is_empty());
    assert!(find(&store, &alice(), "walrus").await.rows.is_empty());

    let first = store.backfill_search(None, 2).await.unwrap();
    assert_eq!((first.threads, first.skipped), (2, 0));
    assert!(first.next.is_some());
    let mut threads = first.threads;
    let mut next = first.next;
    while next.is_some() {
        let page = store.backfill_search(next, 2).await.unwrap();
        threads += page.threads;
        next = page.next;
    }
    assert_eq!(threads, 4);
    assert_eq!(find(&store, &alice(), "otter").await, before_otter);
    assert_eq!(find(&store, &alice(), "walrus").await, before_walrus);

    // again: nothing to do, nothing changes
    let again = store.backfill_search(None, 50).await.unwrap();
    assert_eq!(
        (again.threads, again.messages, again.skipped, again.next),
        (0, 0, 0, None)
    );
    assert_eq!(find(&store, &alice(), "otter").await, before_otter);
}

/// A limit of zero is one, and a request with no term answers nothing.
pub async fn a_limit_of_zero_is_one_and_no_term_matches_nothing<S: LogAndIndex>(store: S) {
    open(
        &store,
        &alice(),
        1,
        "Zebra",
        None,
        vec![say(&alice(), "zebra")],
    )
    .await;
    open(
        &store,
        &alice(),
        2,
        "Zebra",
        None,
        vec![say(&alice(), "zebra")],
    )
    .await;
    let page = store
        .search_threads(
            &alice(),
            &SearchRequest {
                limit: 0,
                ..request("zebra")
            },
        )
        .await
        .unwrap();
    assert_eq!((page.rows.len(), page.more), (1, true));
    let none = SearchRequest {
        terms: SearchTerms::parse("a").unwrap(),
        ..request("zebra")
    };
    assert!(
        store
            .search_threads(&alice(), &none)
            .await
            .unwrap()
            .rows
            .is_empty()
    );
}
