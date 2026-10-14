//! Large files by a presigned upload (`uploads/v1`, ADR 0058), through the application: the slot
//! `request_upload` gives and every way it is refused, and the ingest of the reference an agent
//! hands back, on the in-memory store that plays a bucket (`MemoryArtifacts::presigning`).
//!
//! The agent's side is played by the test: it asks for a slot, sends bytes to the "bucket" the way a
//! `PUT` would (`MemoryArtifacts::upload`, with the checks of a real one, or without them: `lenient`),
//! and the scripted agent reports the reference as the dispatcher would read it from the A2A mapper.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use jiff::Timestamp;
use orch_app::{
    App, AppConfig, Dispatcher, FileLimits, NewThread, UploadCaller, UploadRefusal, UploadRequest,
    UploadSettings, UploadSlot,
};
use orch_core::{
    AgentId, AgentUpdate, ArtifactData, Caller, Event, EventBody, KnownExtension, ThreadId,
    ThreadState, UploadOffer,
};
use orch_ports::memory::{Call, MemoryArtifacts, MemoryStore, MemoryWakeup, ScriptedAgent};
use orch_ports::memory::{ScriptedModel, SeqIds};
use orch_ports::{
    ArtifactKey, ArtifactStore, FixedRegistry, NoArtifacts, PortSet, PresignedPut, SystemClock,
};
use orch_thread_token::{ThreadToolsKeys, UploadClaims, mint_upload_id};
use secrecy::SecretString;
use sha2::{Digest, Sha256};
use support::*;
use tokio_util::sync::CancellationToken;

type UploadPorts<X> = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    ScriptedModel,
    FixedRegistry,
    orch_ports::RefuseAll,
    X,
>;

const KEY: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const OTHER_KEY: &str = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";
const GIB: u64 = 1 << 30;
const PDF: &[u8] = b"%PDF-1.7 a report that is too large to go in a message";

fn keys(key: &str) -> ThreadToolsKeys {
    ThreadToolsKeys::new(SecretString::from(key.to_owned()), None).unwrap()
}

fn settings(max_bytes: u64) -> UploadSettings {
    UploadSettings::new(max_bytes, Duration::from_secs(300), keys(KEY)).unwrap()
}

fn app_with<X: ArtifactStore>(
    w: &World,
    store: X,
    uploads: Option<UploadSettings>,
    files: FileLimits,
) -> Arc<App<UploadPorts<X>>> {
    Arc::new(
        App::new(
            PortSet {
                artifacts: store,
                store: w.store.clone(),
                wakeup: w.wakeup.clone(),
                agents: w.agent.clone(),
                clock: SystemClock,
                ids: w.ids.clone(),
                model: w.model.clone(),
                auth: orch_ports::RefuseAll,
                registry: directory().fixed_registry(),
            },
            directory(),
            AppConfig {
                files,
                uploads,
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

/// An agent whose card lists `uploads/v1` and `thread-tools/v1`.
fn listing(w: &World) {
    w.agent.set_extensions(
        "plain",
        &[KnownExtension::Uploads, KnownExtension::ThreadTools],
    );
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn caller(thread: ThreadId) -> UploadCaller {
    UploadCaller {
        thread,
        job: 1,
        agent: AgentId::new("plain"),
        caller: Caller::Main,
    }
}

fn request(bytes: &[u8]) -> UploadRequest {
    UploadRequest::parse(
        "report.pdf",
        "application/pdf",
        bytes.len() as u64,
        &sha256_hex(bytes),
    )
    .unwrap()
}

/// A thread the dispatcher has not picked up yet: `queued`, job 1, addressed to `plain`.
async fn queued<X: ArtifactStore>(app: &Arc<App<UploadPorts<X>>>) -> ThreadId {
    app.create_thread(
        &alice(),
        NewThread {
            title: None,
            target: target("plain"),
            text: "files please".to_owned(),
        },
    )
    .await
    .unwrap()
    .id
}

fn put_of(slot: &UploadSlot) -> PresignedPut {
    PresignedPut {
        url: slot.url.clone(),
        headers: slot.headers.clone(),
    }
}

/// The agent's `PUT`, at `at`.
fn send(
    store: &MemoryArtifacts,
    slot: &UploadSlot,
    bytes: &[u8],
    at: Timestamp,
) -> Result<(), String> {
    store.upload(&put_of(slot), Bytes::copy_from_slice(bytes), at)
}

fn reference(slot: &UploadSlot) -> AgentUpdate {
    AgentUpdate::FileUploaded {
        name: "report".to_owned(),
        media_type: Some("application/pdf".to_owned()),
        filename: Some("report.pdf".to_owned()),
        upload_id: slot.upload_id.clone(),
    }
}

/// An id for the given claims, signed with the deployment's key, as the tool would have written it.
fn id_for(claims: &UploadClaims) -> String {
    mint_upload_id(&keys(KEY), claims).unwrap()
}

fn claims(thread: ThreadId, bytes: &[u8]) -> UploadClaims {
    UploadClaims {
        thread,
        job: 1,
        sha256: Sha256::digest(bytes).into(),
        size: bytes.len() as u64,
        media_type: "application/pdf".to_owned(),
        expires_at: Timestamp::from_second(Timestamp::now().as_second() + 300).unwrap(),
    }
}

/// Runs the delegation to its end with the scripted agent reporting `updates`, and returns the events.
async fn run<X: ArtifactStore>(
    w: &World,
    app: &Arc<App<UploadPorts<X>>>,
    thread: ThreadId,
    updates: Vec<AgentUpdate>,
) -> Vec<Event> {
    w.agent.set_files(updates);
    let token = CancellationToken::new();
    let handle = tokio::spawn(Dispatcher::new(Arc::clone(app), fast(), "d1").run(token.clone()));
    eventually("the thread is done", || async {
        let t = app.get_thread(&alice(), thread).await.unwrap();
        (t.state == ThreadState::Done).then_some(())
    })
    .await;
    let events = app.list_events(&alice(), thread, 0, 500).await.unwrap();
    token.cancel();
    handle.await.unwrap();
    events
}

fn artifacts(events: &[Event]) -> Vec<&ArtifactData> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Artifact(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn errors(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Error(d) => Some(d.message.as_str()),
            _ => None,
        })
        .collect()
}

const NOT_KEPT: &str = "the file could not be kept";

// ---- the offer in the grant ------------------------------------------------------------------

#[tokio::test]
async fn the_grant_offers_uploads_only_when_the_deployment_does_and_the_store_can_presign() {
    for (store, uploads, offered) in [
        (MemoryArtifacts::presigning(), Some(settings(GIB)), true),
        // the directory store and the in-memory store cannot presign: the extension is never offered
        (MemoryArtifacts::new(), Some(settings(GIB)), false),
        // a store that can, and no limit configured
        (MemoryArtifacts::presigning(), None, false),
    ] {
        let w = World::new();
        let app = app_with(&w, store, uploads, FileLimits::default());
        let thread = queued(&app).await;
        assert_eq!(
            app.upload_offer(),
            offered.then_some(UploadOffer {
                max_bytes: GIB,
                ttl_secs: 300
            })
        );
        let token = CancellationToken::new();
        let handle =
            tokio::spawn(Dispatcher::new(Arc::clone(&app), fast(), "d1").run(token.clone()));
        eventually("the thread is done", || async {
            let t = app.get_thread(&alice(), thread).await.unwrap();
            (t.state == ThreadState::Done).then_some(())
        })
        .await;
        token.cancel();
        handle.await.unwrap();
        let sent = w.agent.sends();
        let Call::Send { thread_tools, .. } = &sent[0] else {
            panic!("{sent:?}")
        };
        let grant = thread_tools.as_ref().expect("a grant");
        assert_eq!(grant.uploads.is_some(), offered, "{grant:?}");
        assert_eq!(grant.uploads, app.upload_offer());
    }
}

// ---- request_upload ---------------------------------------------------------------------------

#[tokio::test]
async fn a_slot_is_a_presigned_put_for_the_threads_key_and_an_id_that_carries_the_claims() {
    let w = World::new();
    listing(&w);
    let store = MemoryArtifacts::presigning();
    let app = app_with(
        &w,
        store.clone(),
        Some(settings(GIB)),
        FileLimits::default(),
    );
    let thread = queued(&app).await;

    let before = Timestamp::now().as_second();
    let slot = app
        .request_upload(&caller(thread), &request(PDF))
        .await
        .unwrap();
    let after = Timestamp::now().as_second();

    assert_eq!(slot.method, "PUT");
    let key = ArtifactKey::new(thread, Sha256::digest(PDF).into());
    assert!(slot.url.contains(&key.to_string()), "{}", slot.url);
    assert_eq!(slot.headers["content-type"], "application/pdf");
    assert!((before + 300..=after + 300).contains(&slot.expires_at.as_second()));
    // the id says what the dispatcher must know, and is signed with the thread-tools key
    let read = orch_thread_token::verify_upload_id(&keys(KEY), &slot.upload_id).unwrap();
    assert_eq!(read.thread, thread);
    assert_eq!(read.job, 1);
    assert_eq!(read.size, PDF.len() as u64);
    assert_eq!(read.media_type, "application/pdf");
    assert_eq!(read.expires_at, slot.expires_at);
    assert!(
        orch_thread_token::verify_upload_id(&keys(OTHER_KEY), &slot.upload_id).is_err(),
        "another key does not read it"
    );
    // nothing was written: signing is the store's, and the id keeps no table
    assert!(store.is_empty());
    let shown = format!("{slot:?}");
    assert!(
        !shown.contains(&slot.url) && !shown.contains(&slot.upload_id),
        "{shown}"
    );
}

#[tokio::test]
async fn a_slot_is_refused_for_each_reason_of_the_contract_and_nothing_is_signed() {
    let w = World::new();
    listing(&w);
    let store = MemoryArtifacts::presigning();
    let app = app_with(&w, store, Some(settings(1024)), FileLimits::default());
    let thread = queued(&app).await;
    let ask =
        |app: &Arc<App<UploadPorts<MemoryArtifacts>>>, who: UploadCaller, req: UploadRequest| {
            let app = Arc::clone(app);
            async move { app.request_upload(&who, &req).await.unwrap_err() }
        };

    // an asked agent
    let asked = UploadCaller {
        caller: Caller::Ask(1),
        ..caller(thread)
    };
    let err = ask(&app, asked, request(PDF)).await;
    assert_eq!(err, UploadRefusal::CallerNotAllowed);
    assert_eq!(err.code(), "caller_not_allowed");

    // over the most one file may be
    let big = UploadRequest::parse(
        "big.bin",
        "application/octet-stream",
        1025,
        &"ab".repeat(32),
    )
    .unwrap();
    let err = ask(&app, caller(thread), big).await;
    assert_eq!(err, UploadRefusal::TooLarge { max_bytes: 1024 });
    assert_eq!(err.code(), "too_large");
    // exactly the most: fine
    let exact =
        UploadRequest::parse("e.bin", "application/octet-stream", 1024, &"ab".repeat(32)).unwrap();
    assert!(app.request_upload(&caller(thread), &exact).await.is_ok());

    // a thread that does not exist, and a job that is over
    let err = ask(&app, caller(ThreadId(uuid::Uuid::now_v7())), request(PDF)).await;
    assert_eq!(err, UploadRefusal::TaskOver);
    let err = ask(
        &app,
        UploadCaller {
            job: 2,
            ..caller(thread)
        },
        request(PDF),
    )
    .await;
    assert_eq!(err, UploadRefusal::TaskOver);
}

#[tokio::test]
async fn a_slot_needs_a_deployment_that_offers_uploads_a_store_that_presigns_and_a_card_that_lists_them()
 {
    // the deployment offers none
    let w = World::new();
    listing(&w);
    let app = app_with(
        &w,
        MemoryArtifacts::presigning(),
        None,
        FileLimits::default(),
    );
    let thread = queued(&app).await;
    let err = app
        .request_upload(&caller(thread), &request(PDF))
        .await
        .unwrap_err();
    assert!(matches!(err, UploadRefusal::NotActivated(_)), "{err}");
    assert_eq!(err.code(), "not_activated");

    // a store that cannot presign
    let w = World::new();
    listing(&w);
    let app = app_with(
        &w,
        MemoryArtifacts::new(),
        Some(settings(GIB)),
        FileLimits::default(),
    );
    let thread = queued(&app).await;
    let err = app
        .request_upload(&caller(thread), &request(PDF))
        .await
        .unwrap_err();
    assert_eq!(err, UploadRefusal::Unsupported);
    assert_eq!(err.code(), "unsupported");
    // and no store at all
    let w = World::new();
    listing(&w);
    let app = app_with(&w, NoArtifacts, Some(settings(GIB)), FileLimits::default());
    let thread = queued(&app).await;
    assert_eq!(
        app.request_upload(&caller(thread), &request(PDF))
            .await
            .unwrap_err(),
        UploadRefusal::Unsupported
    );

    // a card that lists uploads alone, thread tools alone, or neither: the card is read now, for
    // every call, and anything else than both is "not activated"
    for (listed, down) in [
        (vec![KnownExtension::Uploads], false),
        (vec![KnownExtension::ThreadTools], false),
        (vec![], false),
    ] {
        let w = World::new();
        w.agent.set_extensions("plain", &listed);
        w.agent.set_card_down("plain", down);
        let app = app_with(
            &w,
            MemoryArtifacts::presigning(),
            Some(settings(GIB)),
            FileLimits::default(),
        );
        let thread = queued(&app).await;
        let err = app
            .request_upload(&caller(thread), &request(PDF))
            .await
            .unwrap_err();
        assert!(
            matches!(err, UploadRefusal::NotActivated(_)),
            "{listed:?} {down}: {err}"
        );
    }

    // a card that cannot be read is not a card that says "no": the call fails closed, but it says
    // that the card could not be read now, which a retry can mend (`unavailable`), not that the
    // agent does not offer the extension
    let w = World::new();
    w.agent.set_extensions(
        "plain",
        &[KnownExtension::Uploads, KnownExtension::ThreadTools],
    );
    w.agent.set_card_down("plain", true);
    let app = app_with(
        &w,
        MemoryArtifacts::presigning(),
        Some(settings(GIB)),
        FileLimits::default(),
    );
    let thread = queued(&app).await;
    let err = app
        .request_upload(&caller(thread), &request(PDF))
        .await
        .unwrap_err();
    assert_eq!(err, UploadRefusal::Unavailable);
    assert_eq!(err.code(), "unavailable");
    assert!(!app.uploads_listed(&AgentId::new("plain")).await);
    // and the retry, once the card can be read, is a slot
    w.agent.set_card_down("plain", false);
    assert!(
        app.request_upload(&caller(thread), &request(PDF))
            .await
            .is_ok()
    );

    // and an agent that drops the extension between two calls is refused at the second
    let w = World::new();
    listing(&w);
    let app = app_with(
        &w,
        MemoryArtifacts::presigning(),
        Some(settings(GIB)),
        FileLimits::default(),
    );
    let thread = queued(&app).await;
    assert!(
        app.request_upload(&caller(thread), &request(PDF))
            .await
            .is_ok()
    );
    w.agent
        .set_extensions("plain", &[KnownExtension::ThreadTools]);
    assert!(matches!(
        app.request_upload(&caller(thread), &request(PDF))
            .await
            .unwrap_err(),
        UploadRefusal::NotActivated(_)
    ));
    assert!(!app.uploads_listed(&AgentId::new("plain")).await);
}

#[tokio::test]
async fn the_size_and_the_limits_are_checked_before_the_card_is_read() {
    // the card is a call to the agent: what can be known without it is answered first, so a request
    // that could never be given is told so even while the card cannot be read
    let w = World::new();
    w.agent.set_extensions(
        "plain",
        &[KnownExtension::Uploads, KnownExtension::ThreadTools],
    );
    w.agent.set_card_down("plain", true);
    let limits = FileLimits {
        max_file_bytes: 1024,
        max_per_job_bytes: 100,
        max_files_per_job: 3,
    };
    let app = app_with(
        &w,
        MemoryArtifacts::presigning(),
        Some(settings(1000)),
        limits,
    );
    let thread = queued(&app).await;

    let too_large = vec![7u8; 1001];
    let err = app
        .request_upload(&caller(thread), &request(&too_large))
        .await
        .unwrap_err();
    assert_eq!(err, UploadRefusal::TooLarge { max_bytes: 1000 });

    let over_the_job = vec![7u8; 200];
    let err = app
        .request_upload(&caller(thread), &request(&over_the_job))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "over_limit", "{err}");

    // within both, the card decides: it cannot be read, so the call cannot be answered yet
    let small = vec![7u8; 50];
    assert_eq!(
        app.request_upload(&caller(thread), &request(&small))
            .await
            .unwrap_err(),
        UploadRefusal::Unavailable
    );
}

#[tokio::test]
async fn the_over_limit_message_names_the_limits_of_the_deployment() {
    let w = World::new();
    listing(&w);
    let limits = FileLimits {
        max_file_bytes: 1024,
        max_per_job_bytes: 100,
        max_files_per_job: 3,
    };
    let app = app_with(
        &w,
        MemoryArtifacts::presigning(),
        Some(settings(1000)),
        limits,
    );
    let thread = queued(&app).await;
    let big = vec![7u8; 200];
    let err = app
        .request_upload(&caller(thread), &request(&big))
        .await
        .unwrap_err();
    let UploadRefusal::OverLimit(message) = &err else {
        panic!("{err}")
    };
    assert!(
        message.contains("at most 3 files") && message.contains("at most 100 bytes"),
        "the message names the configured limits, not a constant: {message}"
    );
    assert!(message.contains("200 bytes"), "{message}");
}

#[tokio::test]
async fn a_slot_counts_what_the_job_has_kept_each_content_once() {
    let w = World::new();
    listing(&w);
    let store = MemoryArtifacts::presigning();
    // a job may keep two files and 200 bytes
    let limits = FileLimits {
        max_file_bytes: 1024,
        max_per_job_bytes: 200,
        max_files_per_job: 2,
    };
    let app = app_with(&w, store.clone(), Some(settings(200)), limits);
    let thread = queued(&app).await;
    let first = vec![1u8; 120];
    let none = app.job_files(thread, 1).await.unwrap();
    assert_eq!((none.files(), none.bytes()), (0, 0));

    // the job keeps one file of 120 bytes (an upload that ran)
    let slot = app
        .request_upload(&caller(thread), &request(&first))
        .await
        .unwrap();
    send(&store, &slot, &first, Timestamp::now()).unwrap();
    let events = run(&w, &app, thread, vec![reference(&slot)]).await;
    assert_eq!(artifacts(&events)[0].file.as_ref().unwrap().size, 120);
    let kept = app.job_files(thread, 1).await.unwrap();
    assert_eq!((kept.files(), kept.bytes()), (1, 120));

    // a content the job has kept costs nothing; a new one of 80 bytes fits, one of 81 would make
    // 201; and with two files kept a third is over the count
    let (hash_a, hash_b) = (sha256_hex(&first), sha256_hex(b"new"));
    assert!(kept.admits(2, 200, &hash_a, 120));
    assert!(kept.admits(2, 200, &hash_b, 80));
    assert!(!kept.admits(2, 200, &hash_b, 81));
    assert!(!kept.admits(1, 200, &hash_b, 1), "one file is the most");

    // the thread is done: a slot for it is refused (the task is over), whatever the room
    let again = vec![9u8; 10];
    assert_eq!(
        app.request_upload(&caller(thread), &request(&again))
            .await
            .unwrap_err(),
        UploadRefusal::TaskOver
    );
}

// ---- the ingest of a reference ----------------------------------------------------------------

#[tokio::test]
async fn an_upload_the_agent_names_is_kept_and_the_log_holds_a_reference_and_never_the_id_or_the_url()
 {
    let w = World::new();
    listing(&w);
    let store = MemoryArtifacts::presigning();
    let app = app_with(
        &w,
        store.clone(),
        Some(settings(GIB)),
        FileLimits::default(),
    );
    let thread = queued(&app).await;
    let slot = app
        .request_upload(&caller(thread), &request(PDF))
        .await
        .unwrap();
    send(&store, &slot, PDF, Timestamp::now()).unwrap();

    let events = run(&w, &app, thread, vec![reference(&slot)]).await;

    assert_eq!(
        shape(&events),
        [
            "user_message",
            "agent_status:working",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    let [artifact] = artifacts(&events)[..] else {
        panic!("one artifact")
    };
    let file = artifact.file.as_ref().expect("a reference");
    assert_eq!(artifact.name, "report");
    assert_eq!(artifact.mime_type.as_deref(), Some("application/pdf"));
    assert_eq!(file.sha256, sha256_hex(PDF));
    assert_eq!(file.size, PDF.len() as u64);
    assert_eq!(file.filename.as_deref(), Some("report.pdf"));
    assert!(errors(&events).is_empty(), "{:?}", errors(&events));
    // the object is where the log says, and nothing else was written
    let key = ArtifactKey::new(thread, Sha256::digest(PDF).into());
    assert!(store.contains(&key));
    assert_eq!(store.len(), 1);

    // no id, no URL, no signature anywhere in what the log keeps
    let json = serde_json::to_string(&events).unwrap();
    assert!(!json.contains(&slot.upload_id), "{json}");
    assert!(
        !json.contains("memory://") && !json.contains("uploadId"),
        "{json}"
    );
    assert!(json.contains(&file.sha256));
}

#[tokio::test]
async fn an_upload_is_not_kept_when_it_is_not_what_was_declared_and_an_object_that_is_not_the_content_is_deleted()
 {
    // (what the agent puts in the bucket, what it declared, whether the bucket takes it,
    //  whether the object is gone afterwards)
    struct Case {
        name: &'static str,
        store: MemoryArtifacts,
        declared: Vec<u8>,
        sent: Vec<u8>,
        deleted: bool,
    }
    let honest = PDF.to_vec();
    let other_same_length = {
        let mut v = PDF.to_vec();
        v[10] ^= 1;
        v
    };
    let cases = [
        Case {
            name: "other bytes of the same length, a bucket that does not enforce the checksum",
            store: MemoryArtifacts::presigning().lenient(),
            declared: honest.clone(),
            sent: other_same_length.clone(),
            deleted: true,
        },
        Case {
            name: "other bytes of another length",
            store: MemoryArtifacts::presigning().lenient(),
            declared: honest.clone(),
            sent: b"%PDF-1.7 short".to_vec(),
            deleted: true,
        },
        Case {
            name: "other bytes, a bucket that does not say a checksum: the worker hashes the object",
            store: MemoryArtifacts::presigning().lenient().without_checksums(),
            declared: honest.clone(),
            sent: other_same_length,
            deleted: true,
        },
    ];
    for case in cases {
        let w = World::new();
        listing(&w);
        let app = app_with(
            &w,
            case.store.clone(),
            Some(settings(GIB)),
            FileLimits::default(),
        );
        let thread = queued(&app).await;
        let slot = app
            .request_upload(&caller(thread), &request(&case.declared))
            .await
            .unwrap();
        send(&case.store, &slot, &case.sent, Timestamp::now()).unwrap();
        let events = run(&w, &app, thread, vec![reference(&slot)]).await;

        let [artifact] = artifacts(&events)[..] else {
            panic!("{}: one artifact", case.name)
        };
        assert!(artifact.file.is_none(), "{}", case.name);
        assert_eq!(errors(&events), [NOT_KEPT], "{}", case.name);
        let key = ArtifactKey::new(thread, Sha256::digest(&case.declared).into());
        assert_eq!(!case.store.contains(&key), case.deleted, "{}", case.name);
        // the thread goes on to done: the turn continues
        assert_eq!(
            shape(&events).last().map(String::as_str),
            Some("thread_state:done"),
            "{}",
            case.name
        );
    }
}

#[tokio::test]
async fn a_declared_size_that_lies_about_honest_content_is_refused_and_the_object_is_deleted() {
    // The bucket cannot hold an agent to the size it declared (`content-length` is not signed), so
    // the declared size, the limits and the job's bytes are checked here, after the fact. The
    // content hashes to the key, and the thread keeps no file with that hash: nothing refers to
    // the object, and it is deleted rather than left as bytes outside every limit.
    let w = World::new();
    listing(&w);
    let store = MemoryArtifacts::presigning();
    let app = app_with(
        &w,
        store.clone(),
        Some(settings(GIB)),
        FileLimits::default(),
    );
    let thread = queued(&app).await;
    let lie = UploadRequest::parse("r.pdf", "application/pdf", 5, &sha256_hex(PDF)).unwrap();
    let slot = app.request_upload(&caller(thread), &lie).await.unwrap();
    // the bucket checks the checksum, and the bytes are the declared hash's: it takes them
    send(&store, &slot, PDF, Timestamp::now()).unwrap();
    let key = ArtifactKey::new(thread, Sha256::digest(PDF).into());
    assert!(store.contains(&key), "the bucket took the bytes");

    let events = run(&w, &app, thread, vec![reference(&slot)]).await;

    assert_eq!(errors(&events), [NOT_KEPT]);
    assert!(artifacts(&events)[0].file.is_none());
    assert!(
        !store.contains(&key),
        "an object nothing refers to and that is not as declared is deleted"
    );
}

#[tokio::test]
async fn a_refused_upload_of_a_content_the_thread_keeps_already_leaves_the_object() {
    // The same bytes are a file the thread keeps (kept by an earlier job, put by a slot that told
    // the truth): the object is that file's, and its reference must go on working.
    let w = World::new();
    listing(&w);
    let store = MemoryArtifacts::presigning();
    let app = app_with(
        &w,
        store.clone(),
        Some(settings(GIB)),
        FileLimits::default(),
    );
    let thread = queued(&app).await;
    let honest = app
        .request_upload(&caller(thread), &request(PDF))
        .await
        .unwrap();
    send(&store, &honest, PDF, Timestamp::now()).unwrap();
    let events = run(&w, &app, thread, vec![reference(&honest)]).await;
    assert!(artifacts(&events)[0].file.is_some(), "job 1 keeps the file");
    let key = ArtifactKey::new(thread, Sha256::digest(PDF).into());

    // job 2: the person writes again (the scripted agent plays "files" for a text that begins with
    // it), and the agent asks for a slot that lies about the size
    app.post_message(&alice(), thread, "files again".to_owned())
        .await
        .unwrap();
    let second = UploadCaller {
        job: 2,
        ..caller(thread)
    };
    let lie = UploadRequest::parse("r.pdf", "application/pdf", 5, &sha256_hex(PDF)).unwrap();
    let slot = app.request_upload(&second, &lie).await.unwrap();
    send(&store, &slot, PDF, Timestamp::now()).unwrap();
    let events = run(&w, &app, thread, vec![reference(&slot)]).await;

    assert_eq!(errors(&events), [NOT_KEPT]);
    assert!(
        store.contains(&key),
        "the log refers to this object (job 1): it stays"
    );
    // and what the first reference was is still there to read
    let first = artifacts(&events)
        .into_iter()
        .find_map(|a| a.file.as_ref())
        .expect("the file of job 1 is in the log");
    assert_eq!(first.sha256, sha256_hex(PDF));
}

#[tokio::test]
async fn an_upload_nobody_put_an_id_that_is_not_the_threads_and_an_expired_one_are_not_kept() {
    let w = World::new();
    listing(&w);
    let store = MemoryArtifacts::presigning();
    let app = app_with(
        &w,
        store.clone(),
        Some(settings(GIB)),
        FileLimits::default(),
    );
    let thread = queued(&app).await;
    let other_thread = ThreadId(uuid::Uuid::now_v7());

    // nothing was put under the key
    let never = app
        .request_upload(&caller(thread), &request(PDF))
        .await
        .unwrap();

    // an object that is there, under the right key, but named by an id of another thread / job /
    // key, or a made-up one: the id is what opens the key, so nothing is read
    let there = b"%PDF-1.7 another report".to_vec();
    let slot = app
        .request_upload(&caller(thread), &request(&there))
        .await
        .unwrap();
    send(&store, &slot, &there, Timestamp::now()).unwrap();
    let ids = [
        ("another thread", id_for(&claims(other_thread, &there))),
        (
            "another job",
            id_for(&UploadClaims {
                job: 2,
                ..claims(thread, &there)
            }),
        ),
        (
            "another key",
            mint_upload_id(&keys(OTHER_KEY), &claims(thread, &there)).unwrap(),
        ),
        ("made up", "a.b.c".to_owned()),
        ("empty", String::new()),
    ];
    let mut updates = vec![reference(&never)];
    for (_, id) in &ids {
        updates.push(AgentUpdate::FileUploaded {
            name: "report".into(),
            media_type: None,
            filename: None,
            upload_id: id.clone(),
        });
    }
    let events = run(&w, &app, thread, updates).await;

    // every one is "the file could not be kept", none has a file, and the object stays
    let entries = artifacts(&events);
    assert_eq!(entries.len(), 1 + ids.len());
    assert!(entries.iter().all(|a| a.file.is_none()));
    assert_eq!(errors(&events), vec![NOT_KEPT; 1 + ids.len()]);
    let key = ArtifactKey::new(thread, Sha256::digest(&there).into());
    assert!(
        store.contains(&key),
        "an id that opens nothing deletes nothing"
    );
}

#[tokio::test]
async fn an_object_that_arrived_after_the_url_expired_is_not_kept() {
    let w = World::new();
    listing(&w);
    let store = MemoryArtifacts::presigning();
    let app = app_with(
        &w,
        store.clone(),
        Some(settings(GIB)),
        FileLimits::default(),
    );
    let thread = queued(&app).await;
    let slot = app
        .request_upload(&caller(thread), &request(PDF))
        .await
        .unwrap();
    // the bucket got it an hour after the URL stopped working
    let late = Timestamp::from_second(slot.expires_at.as_second() + 3600).unwrap();
    send(&store, &slot, PDF, late).unwrap();

    let events = run(&w, &app, thread, vec![reference(&slot)]).await;

    assert_eq!(errors(&events), [NOT_KEPT]);
    assert!(artifacts(&events)[0].file.is_none());
    let key = ArtifactKey::new(thread, Sha256::digest(PDF).into());
    assert!(
        !store.contains(&key),
        "nothing refers to it and it came too late: deleted"
    );
}

#[tokio::test]
async fn a_store_that_does_not_say_when_the_object_arrived_cannot_vouch_for_an_expired_url() {
    // `Last-Modified` absent (or one the adapter could not read) is "the store does not say", not
    // "in time": while the URL still lives the object cannot be late, after it it may be.
    for (expired, kept) in [(false, true), (true, false)] {
        let w = World::new();
        listing(&w);
        let store = MemoryArtifacts::presigning();
        let app = app_with(
            &w,
            store.clone(),
            Some(settings(GIB)),
            FileLimits::default(),
        );
        let thread = queued(&app).await;
        // an object with no arrival time (a plain `put` has none), named by an id whose URL stopped
        // working an hour ago, or has minutes left
        let meta = orch_ports::ArtifactMeta::of("application/pdf", None, PDF);
        store
            .put(&meta.key(thread), Bytes::from_static(PDF), &meta)
            .await
            .unwrap();
        let ends = Timestamp::now().as_second() + if expired { -3600 } else { 300 };
        let id = id_for(&UploadClaims {
            expires_at: Timestamp::from_second(ends).unwrap(),
            ..claims(thread, PDF)
        });
        let update = AgentUpdate::FileUploaded {
            name: "report".to_owned(),
            media_type: Some("application/pdf".to_owned()),
            filename: Some("report.pdf".to_owned()),
            upload_id: id,
        };

        let events = run(&w, &app, thread, vec![update]).await;

        let entry = artifacts(&events)[0];
        assert_eq!(entry.file.is_some(), kept, "expired: {expired}");
        assert_eq!(
            errors(&events).is_empty(),
            kept,
            "expired: {expired}: {:?}",
            errors(&events)
        );
        let key = ArtifactKey::new(thread, Sha256::digest(PDF).into());
        assert_eq!(
            store.contains(&key),
            kept,
            "expired: {expired}: an object refused for its time, that nothing refers to, is deleted"
        );
    }
}

#[tokio::test]
async fn an_image_must_agree_with_its_bytes_and_a_type_the_bytes_agree_with_is_kept() {
    for (media_type, bytes, kept) in [
        (
            "image/png",
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR....".to_vec(),
            true,
        ),
        // a lie: declared a png, the bytes are a page of markup
        (
            "image/png",
            b"<html><script>alert(1)</script></html>".to_vec(),
            false,
        ),
        // a type that is not an image is kept as declared
        ("text/plain", b"hello".to_vec(), true),
        (
            "application/octet-stream",
            b"\x89PNG\r\n\x1a\n....".to_vec(),
            true,
        ),
    ] {
        let w = World::new();
        listing(&w);
        let store = MemoryArtifacts::presigning();
        let app = app_with(
            &w,
            store.clone(),
            Some(settings(GIB)),
            FileLimits::default(),
        );
        let thread = queued(&app).await;
        let req =
            UploadRequest::parse("f", media_type, bytes.len() as u64, &sha256_hex(&bytes)).unwrap();
        let slot = app.request_upload(&caller(thread), &req).await.unwrap();
        send(&store, &slot, &bytes, Timestamp::now()).unwrap();
        let events = run(&w, &app, thread, vec![reference(&slot)]).await;
        let entry = artifacts(&events)[0];
        assert_eq!(entry.file.is_some(), kept, "{media_type}");
        let key = ArtifactKey::new(thread, Sha256::digest(&bytes).into());
        assert_eq!(
            store.contains(&key),
            kept,
            "{media_type}: a refused object nothing refers to is deleted"
        );
        if kept {
            // the type of the file is the one it was put with: that is the Content-Type it is served as
            assert_eq!(entry.mime_type.as_deref(), Some(media_type));
        } else {
            assert_eq!(errors(&events), [NOT_KEPT]);
        }
    }
}

#[tokio::test]
async fn an_image_type_in_a_variant_spelling_is_signed_and_kept_as_its_canonical_type() {
    // `image/jpg` is how many agents spell JPEG. The slot is for the canonical type (it is what is
    // signed, stored and served), so an honest file is not refused by the ingest comparing the
    // type its bytes have with the spelling it was asked in.
    const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0, 0x10, b'J', b'F', b'I', b'F', 0];
    const SVG: &[u8] = b"<svg xmlns='http://www.w3.org/2000/svg'/>";
    for (declared, canonical, bytes) in [
        ("image/jpg", "image/jpeg", JPEG),
        ("IMAGE/PJPEG; q=1", "image/jpeg", JPEG),
        ("image/jpeg", "image/jpeg", JPEG),
        ("image/svg", "image/svg+xml", SVG),
    ] {
        let w = World::new();
        listing(&w);
        let store = MemoryArtifacts::presigning();
        let app = app_with(
            &w,
            store.clone(),
            Some(settings(GIB)),
            FileLimits::default(),
        );
        let thread = queued(&app).await;
        let req =
            UploadRequest::parse("f", declared, bytes.len() as u64, &sha256_hex(bytes)).unwrap();
        assert_eq!(req.media_type, canonical, "{declared}");
        let slot = app.request_upload(&caller(thread), &req).await.unwrap();
        assert_eq!(slot.headers["content-type"], canonical, "{declared}");
        send(&store, &slot, bytes, Timestamp::now()).unwrap();

        let events = run(&w, &app, thread, vec![reference(&slot)]).await;

        let entry = artifacts(&events)[0];
        assert!(entry.file.is_some(), "{declared}: {:?}", errors(&events));
        assert_eq!(entry.mime_type.as_deref(), Some(canonical), "{declared}");
        assert!(errors(&events).is_empty(), "{declared}");
    }
}

#[tokio::test]
async fn an_upload_counts_against_the_jobs_limits_like_any_file() {
    let w = World::new();
    listing(&w);
    let store = MemoryArtifacts::presigning();
    // the job may keep one file
    let limits = FileLimits {
        max_file_bytes: 1024,
        max_per_job_bytes: GIB,
        max_files_per_job: 1,
    };
    let app = app_with(&w, store.clone(), Some(settings(GIB)), limits);
    let thread = queued(&app).await;
    let a = app
        .request_upload(&caller(thread), &request(PDF))
        .await
        .unwrap();
    send(&store, &a, PDF, Timestamp::now()).unwrap();
    let second = b"%PDF-1.7 a second report";
    let b = app
        .request_upload(&caller(thread), &request(second))
        .await
        .unwrap();
    send(&store, &b, second, Timestamp::now()).unwrap();

    // the same upload named twice is one file; a second content is over the count
    let events = run(
        &w,
        &app,
        thread,
        vec![reference(&a), reference(&a), reference(&b)],
    )
    .await;

    let entries = artifacts(&events);
    assert_eq!(entries.len(), 3);
    assert!(entries[0].file.is_some() && entries[1].file.is_some());
    assert!(entries[2].file.is_none());
    assert_eq!(
        errors(&events),
        ["this job has reached its limit of files, so the file is not kept"]
    );
    // the file that was kept stays; the one that did not fit was refused, nothing refers to it
    // and it is deleted: a job's limits bind the bucket too
    assert!(store.contains(&ArtifactKey::new(thread, Sha256::digest(PDF).into())));
    assert!(!store.contains(&ArtifactKey::new(thread, Sha256::digest(second).into())));
}

#[tokio::test]
async fn an_upload_over_the_most_one_may_be_is_too_large_to_keep() {
    let w = World::new();
    listing(&w);
    let store = MemoryArtifacts::presigning();
    // the slot was given when the deployment allowed a gigabyte; the dispatcher that reads the
    // reference allows ten bytes: the id is not honoured beyond what is allowed now
    let generous = app_with(
        &w,
        store.clone(),
        Some(settings(GIB)),
        FileLimits::default(),
    );
    let strict = app_with(&w, store.clone(), Some(settings(10)), FileLimits::default());
    let thread = queued(&generous).await;
    let slot = generous
        .request_upload(&caller(thread), &request(PDF))
        .await
        .unwrap();
    send(&store, &slot, PDF, Timestamp::now()).unwrap();

    let events = run(&w, &strict, thread, vec![reference(&slot)]).await;

    assert_eq!(errors(&events), ["the file is too large to keep"]);
    assert!(artifacts(&events)[0].file.is_none());
    let key = ArtifactKey::new(thread, Sha256::digest(PDF).into());
    assert!(
        !store.contains(&key),
        "over the size allowed and kept by no file: deleted, not left outside every limit"
    );
}

#[tokio::test]
async fn a_deployment_without_uploads_or_with_a_store_that_cannot_take_them_keeps_nothing() {
    for (name, uploads, store) in [
        ("no limit configured", None, MemoryArtifacts::presigning()),
        (
            "a store that cannot presign",
            Some(settings(GIB)),
            MemoryArtifacts::new(),
        ),
    ] {
        let w = World::new();
        listing(&w);
        let app = app_with(&w, store, uploads, FileLimits::default());
        let thread = queued(&app).await;
        let id = id_for(&claims(thread, PDF));
        let events = run(
            &w,
            &app,
            thread,
            vec![AgentUpdate::FileUploaded {
                name: "report".into(),
                media_type: None,
                filename: None,
                upload_id: id,
            }],
        )
        .await;
        assert_eq!(errors(&events), [NOT_KEPT], "{name}");
        assert!(artifacts(&events)[0].file.is_none(), "{name}");
    }
}
