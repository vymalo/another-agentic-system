//! Files an agent hands over (ADR 0032), through the dispatcher: the ingest puts the bytes in the
//! artifact store **before** the commit, the log gets a reference and never the bytes, and a file
//! that cannot be kept is an artifact entry without a file and an error, with the turn going on.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt as _;
use orch_app::{
    App, AppConfig, AppError, Dispatcher, FileLimits, FirstMessage, ForkAt, ForkRequest,
};
use orch_core::{
    AgentUpdate, ArtifactData, Classify as _, EventBody, Origin, ThreadId, ThreadState,
};
use orch_ports::memory::{
    MemoryArtifacts, MemoryStore, MemoryWakeup, ScriptedAgent, ScriptedModel, SeqIds,
};
use orch_ports::{
    ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream, FixedRegistry,
    NoArtifacts, PortSet, SystemClock, ThreadListing, ThreadStore,
};
use support::*;
use tokio_util::sync::CancellationToken;

type FilePorts<X> = PortSet<
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

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR....";

fn app_with<X: ArtifactStore>(w: &World, store: X, files: FileLimits) -> Arc<App<FilePorts<X>>> {
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
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

fn raw(name: &str, media_type: Option<&str>, filename: Option<&str>, bytes: &[u8]) -> AgentUpdate {
    AgentUpdate::File {
        name: name.to_owned(),
        media_type: media_type.map(str::to_owned),
        filename: filename.map(str::to_owned),
        bytes: bytes.to_vec(),
    }
}

/// Runs one turn of the `files` script to its end and returns the thread's events.
async fn turn<X: ArtifactStore>(
    w: &World,
    store: X,
    limits: FileLimits,
    updates: Vec<AgentUpdate>,
) -> (ThreadId, Vec<orch_core::Event>) {
    w.agent.set_files(updates);
    let app = app_with(w, store, limits);
    let token = CancellationToken::new();
    let handle = tokio::spawn(Dispatcher::new(Arc::clone(&app), fast(), "d1").run(token.clone()));
    let t = app
        .create_thread(
            &alice(),
            orch_app::NewThread {
                title: None,
                target: target("plain"),
                text: "files please".to_owned(),
            },
        )
        .await
        .unwrap();
    eventually("the thread is done", || async {
        let t = app.get_thread(&alice(), t.id).await.unwrap();
        (t.state == ThreadState::Done).then_some(())
    })
    .await;
    let events = app.list_events(&alice(), t.id, 0, 500).await.unwrap();
    token.cancel();
    handle.await.unwrap();
    (t.id, events)
}

fn artifacts(events: &[orch_core::Event]) -> Vec<&ArtifactData> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Artifact(a) => Some(a),
            _ => None,
        })
        .collect()
}

fn errors(events: &[orch_core::Event]) -> Vec<(&str, bool)> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Error(d) => Some((d.message.as_str(), d.retryable)),
            _ => None,
        })
        .collect()
}

async fn body_of(store: &impl ArtifactStore, key: &ArtifactKey) -> (ArtifactMeta, Vec<u8>) {
    let (meta, mut stream) = store.get(key).await.unwrap().expect("kept");
    let mut bytes = Vec::new();
    while let Some(piece) = stream.next().await {
        bytes.extend_from_slice(&piece.unwrap());
    }
    (meta, bytes)
}

#[tokio::test]
async fn a_file_is_kept_and_the_log_holds_a_reference_and_never_the_bytes() {
    let w = World::new();
    let store = MemoryArtifacts::new();
    let (thread, events) = turn(
        &w,
        store.clone(),
        FileLimits::default(),
        vec![raw("chart", Some("image/png"), Some("chart.png"), PNG)],
    )
    .await;
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
    assert_eq!(artifact.name, "chart");
    assert_eq!(artifact.mime_type.as_deref(), Some("image/png"));
    assert_eq!(
        (artifact.uri.as_deref(), artifact.text.as_deref()),
        (None, None)
    );
    assert_eq!(file.size, PNG.len() as u64);
    assert_eq!(file.filename.as_deref(), Some("chart.png"));
    assert_eq!(file.sha256.len(), 64);

    // the file is in the store under the thread and the hash, with its meta
    let key: ArtifactKey = format!("threads/{thread}/{}", file.sha256).parse().unwrap();
    let (meta, bytes) = body_of(&store, &key).await;
    assert_eq!(bytes, PNG);
    assert_eq!(meta.media_type, "image/png");
    assert_eq!(meta.filename.as_deref(), Some("chart.png"));
    assert_eq!(meta.size, PNG.len() as u64);

    // the bytes never reached the log: not in an event, in no form
    let json = serde_json::to_string(&events).unwrap();
    assert!(
        !json.contains("IHDR") && !json.contains("\\u0089PNG"),
        "{json}"
    );
    assert!(!json.contains("\"bytes\""), "{json}");
    assert!(json.contains(&file.sha256));
}

#[tokio::test]
async fn the_file_is_in_the_store_before_the_event_is_committed() {
    /// Looks at the log at the moment of the put.
    #[derive(Clone)]
    struct Probe {
        inner: MemoryArtifacts,
        store: MemoryStore,
        artifacts_in_log_at_put: Arc<std::sync::Mutex<Vec<usize>>>,
    }
    impl ArtifactStore for Probe {
        async fn put(
            &self,
            key: &ArtifactKey,
            bytes: Bytes,
            meta: &ArtifactMeta,
        ) -> Result<(), ArtifactError> {
            let events = self.store.list_events(key.thread(), 0, 500).await.unwrap();
            let seen = events
                .iter()
                .filter(|e| matches!(e.body, EventBody::Artifact(_)))
                .count();
            self.artifacts_in_log_at_put.lock().unwrap().push(seen);
            self.inner.put(key, bytes, meta).await
        }
        async fn get(
            &self,
            key: &ArtifactKey,
        ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
            self.inner.get(key).await
        }
        async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
            self.inner.delete(key).await
        }
        async fn copy(&self, from: &ArtifactKey, to: &ArtifactKey) -> Result<(), ArtifactError> {
            self.inner.copy(from, to).await
        }
    }
    let w = World::new();
    let probe = Probe {
        inner: MemoryArtifacts::new(),
        store: w.store.clone(),
        artifacts_in_log_at_put: Arc::default(),
    };
    let (_, events) = turn(
        &w,
        probe.clone(),
        FileLimits::default(),
        vec![raw("a", None, Some("a.png"), PNG)],
    )
    .await;
    assert_eq!(artifacts(&events).len(), 1);
    assert_eq!(
        *probe.artifacts_in_log_at_put.lock().unwrap(),
        [0],
        "no artifact event was in the log when the put happened"
    );
}

#[tokio::test]
async fn a_type_that_disagrees_with_the_bytes_is_kept_as_octet_stream() {
    let w = World::new();
    let store = MemoryArtifacts::new();
    let (thread, events) = turn(
        &w,
        store.clone(),
        FileLimits::default(),
        vec![raw(
            "photo",
            Some("image/png"),
            Some("photo.png"),
            b"<html><script>alert(1)</script></html>",
        )],
    )
    .await;
    let [artifact] = artifacts(&events)[..] else {
        panic!("one artifact")
    };
    assert_eq!(
        artifact.mime_type.as_deref(),
        Some("application/octet-stream")
    );
    let file = artifact.file.as_ref().unwrap();
    let key: ArtifactKey = format!("threads/{thread}/{}", file.sha256).parse().unwrap();
    assert_eq!(
        body_of(&store, &key).await.0.media_type,
        "application/octet-stream"
    );
    assert!(errors(&events).is_empty(), "a mismatch is not an error");
}

#[tokio::test]
async fn a_type_that_is_missing_is_found_and_a_name_is_cleaned() {
    let w = World::new();
    let (_, events) = turn(
        &w,
        MemoryArtifacts::new(),
        FileLimits::default(),
        vec![raw("x", None, Some("../../tmp/evil\n.png"), PNG)],
    )
    .await;
    let [artifact] = artifacts(&events)[..] else {
        panic!()
    };
    assert_eq!(artifact.mime_type.as_deref(), Some("image/png"));
    assert_eq!(
        artifact.file.as_ref().unwrap().filename.as_deref(),
        Some("evil.png")
    );
}

#[tokio::test]
async fn a_file_over_the_cap_is_an_entry_without_a_file_and_an_error_and_the_turn_goes_on() {
    let w = World::new();
    let store = MemoryArtifacts::new();
    let limits = FileLimits {
        max_file_bytes: 10,
        ..FileLimits::default()
    };
    let (_, events) = turn(
        &w,
        store.clone(),
        limits,
        vec![
            raw("big", Some("application/zip"), Some("big.zip"), &[7; 11]),
            raw("small", Some("text/plain"), Some("s.txt"), b"ten bytes!"),
        ],
    )
    .await;
    let found = artifacts(&events);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].name, "big");
    assert!(found[0].file.is_none());
    assert_eq!(found[0].mime_type.as_deref(), Some("application/zip"));
    assert_eq!(found[1].name, "small");
    assert!(found[1].file.is_some(), "the next file is not affected");
    assert_eq!(errors(&events), [("the file is too large to keep", false)]);
    assert_eq!(store.len(), 1);
    assert_eq!(
        shape(&events).last().map(String::as_str),
        Some("thread_state:done")
    );
}

#[tokio::test]
async fn the_limits_of_a_job_count_bytes_and_files_and_a_content_once() {
    // bytes: 6 + 6 is over 10
    let w = World::new();
    let store = MemoryArtifacts::new();
    let limits = FileLimits {
        max_per_job_bytes: 10,
        ..FileLimits::default()
    };
    let (_, events) = turn(
        &w,
        store.clone(),
        limits,
        vec![
            raw("a", Some("text/plain"), None, b"aaaaaa"),
            raw("b", Some("text/plain"), None, b"bbbbbb"),
            raw("c", Some("text/plain"), None, b"cccc"),
        ],
    )
    .await;
    let kept: Vec<_> = artifacts(&events)
        .iter()
        .map(|a| a.file.is_some())
        .collect();
    assert_eq!(
        kept,
        [true, false, true],
        "6, then 6 more is too many, then 4 fits"
    );
    let [(message, retryable)] = errors(&events)[..] else {
        panic!("one error")
    };
    assert_eq!(
        message,
        "this job has reached its limit of files, so the file is not kept"
    );
    assert!(!retryable);
    assert_eq!(store.len(), 2);

    // files: at most two
    let w = World::new();
    let store = MemoryArtifacts::new();
    let limits = FileLimits {
        max_files_per_job: 2,
        ..FileLimits::default()
    };
    let (_, events) = turn(
        &w,
        store.clone(),
        limits,
        (0..4u8)
            .map(|n| raw("f", Some("text/plain"), None, &[b'a' + n]))
            .collect(),
    )
    .await;
    let kept: Vec<_> = artifacts(&events)
        .iter()
        .map(|a| a.file.is_some())
        .collect();
    assert_eq!(kept, [true, true, false, false]);
    assert_eq!(errors(&events).len(), 2);

    // the same content again is the same object and costs nothing
    let w = World::new();
    let store = MemoryArtifacts::new();
    let limits = FileLimits {
        max_files_per_job: 1,
        ..FileLimits::default()
    };
    let (_, events) = turn(
        &w,
        store.clone(),
        limits,
        vec![
            raw("one", Some("image/png"), Some("a.png"), PNG),
            raw("two", Some("image/png"), Some("b.png"), PNG),
            raw("three", Some("image/png"), Some("c.png"), PNG),
        ],
    )
    .await;
    let found = artifacts(&events);
    assert!(found.iter().all(|a| a.file.is_some()), "{found:?}");
    let hashes: std::collections::HashSet<_> = found
        .iter()
        .map(|a| a.file.as_ref().unwrap().sha256.clone())
        .collect();
    assert_eq!(hashes.len(), 1);
    assert_eq!(
        store.len(),
        1,
        "one object for one content (idempotent put)"
    );
    assert!(errors(&events).is_empty());
}

#[tokio::test]
async fn a_store_that_fails_is_a_file_that_could_not_be_kept_and_the_turn_goes_on() {
    /// Fails every put with a message that must never reach the chat.
    struct Broken;
    impl ArtifactStore for Broken {
        async fn put(
            &self,
            _: &ArtifactKey,
            _: Bytes,
            _: &ArtifactMeta,
        ) -> Result<(), ArtifactError> {
            Err(ArtifactError::unavailable(
                "bucket orchestrator-secret-bucket refused the write",
            ))
        }
        async fn get(
            &self,
            _: &ArtifactKey,
        ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
            Ok(None)
        }
        async fn delete(&self, _: &ArtifactKey) -> Result<(), ArtifactError> {
            Ok(())
        }
        async fn copy(&self, _: &ArtifactKey, _: &ArtifactKey) -> Result<(), ArtifactError> {
            Err(ArtifactError::unavailable("the store is broken"))
        }
    }
    let w = World::new();
    let (_, events) = turn(
        &w,
        Broken,
        FileLimits::default(),
        vec![raw("chart", Some("image/png"), Some("chart.png"), PNG)],
    )
    .await;
    let [artifact] = artifacts(&events)[..] else {
        panic!()
    };
    assert!(artifact.file.is_none());
    assert_eq!(errors(&events), [("the file could not be kept", false)]);
    let json = serde_json::to_string(&events).unwrap();
    assert!(!json.contains("secret-bucket"), "{json}");
}

#[tokio::test]
async fn without_a_store_a_file_could_not_be_kept_and_nothing_crashes() {
    let w = World::new();
    let (_, events) = turn(
        &w,
        NoArtifacts,
        FileLimits::default(),
        vec![raw("chart", Some("image/png"), Some("chart.png"), PNG)],
    )
    .await;
    let [artifact] = artifacts(&events)[..] else {
        panic!()
    };
    assert!(artifact.file.is_none());
    assert_eq!(artifact.name, "chart");
    assert_eq!(errors(&events), [("the file could not be kept", false)]);
    assert_eq!(
        shape(&events).last().map(String::as_str),
        Some("thread_state:done")
    );
}

#[tokio::test]
async fn a_file_is_kept_once_whichever_way_the_turn_is_read() {
    // the `files` script keys each update by its position: a poll that replays the stream's
    // envelopes collapses into the same events (ADR 0032, idempotent by content and by key)
    let w = World::new();
    let store = MemoryArtifacts::new();
    w.agent.set_resubscribe_supported(false);
    let (_, events) = turn(
        &w,
        store.clone(),
        FileLimits::default(),
        vec![raw("chart", Some("image/png"), Some("chart.png"), PNG)],
    )
    .await;
    assert_eq!(artifacts(&events).len(), 1);
    assert_eq!(store.len(), 1);
}

/// The bytes of the file `sha256` of `thread`, read the way the API serves it: through the
/// application's own check, as the thread's owner.
async fn opened<P: orch_ports::Ports>(
    app: &App<P>,
    thread: ThreadId,
    sha256: &str,
) -> Result<(ArtifactMeta, Vec<u8>), AppError> {
    let (meta, mut stream) = app.open_artifact(&alice(), thread, sha256).await?;
    let mut bytes = Vec::new();
    while let Some(piece) = stream.next().await {
        bytes.extend_from_slice(&piece.unwrap());
    }
    Ok((meta, bytes))
}

#[tokio::test]
async fn a_fork_after_a_turn_opens_the_files_it_inherited() {
    let w = World::new();
    let store = MemoryArtifacts::new();
    let (parent, events) = turn(
        &w,
        store.clone(),
        FileLimits::default(),
        vec![raw("chart", Some("image/png"), Some("chart.png"), PNG)],
    )
    .await;
    let sha = artifacts(&events)[0].file.as_ref().unwrap().sha256.clone();
    let app = app_with(&w, store.clone(), FileLimits::default());

    let fork = app
        .fork_thread(
            &alice(),
            parent,
            ForkRequest {
                at: ForkAt::AfterTurn {
                    seq: 1,
                    first: None,
                },
                target: None,
                id: None,
            },
        )
        .await
        .unwrap()
        .thread;

    // the fork's copied log refers to the file under the fork's own id (the projection builds the
    // `href` from the thread of the request), so the file must be openable there
    let (meta, bytes) = opened(&app, fork.id, &sha)
        .await
        .expect("the fork opens the file it inherited");
    assert_eq!(bytes, PNG);
    assert_eq!(meta.filename.as_deref(), Some("chart.png"));
    assert_eq!(meta.media_type, "image/png");
    // the parent's own copy is untouched
    assert_eq!(opened(&app, parent, &sha).await.unwrap().1, PNG);
    assert_eq!(
        store.len(),
        2,
        "one object per thread, none for anyone else"
    );
}

fn first_message(text: &str) -> FirstMessage {
    FirstMessage {
        text: text.to_owned(),
        message_id: Some("m-first".to_owned()),
        run_id: Some("run-1".to_owned()),
        origin: Origin::Agui,
        ui_catalog: None,
        mentions: Vec::new(),
    }
}

fn fork_id() -> ThreadId {
    ThreadId(uuid::Uuid::from_u128(
        0x0190_0000_0000_7000_8000_0000_0000_0042,
    ))
}

#[tokio::test]
async fn a_fork_made_with_its_first_message_opens_the_files_it_inherited() {
    let w = World::new();
    let store = MemoryArtifacts::new();
    let (parent, events) = turn(
        &w,
        store.clone(),
        FileLimits::default(),
        vec![raw("chart", Some("image/png"), Some("chart.png"), PNG)],
    )
    .await;
    let sha = artifacts(&events)[0].file.as_ref().unwrap().sha256.clone();
    let app = app_with(&w, store.clone(), FileLimits::default());

    let fork = app
        .fork_and_send(
            &alice(),
            parent,
            fork_id(),
            1,
            None,
            first_message("echo again"),
        )
        .await
        .unwrap()
        .thread;
    assert_eq!(fork.state, ThreadState::Queued);

    let (meta, bytes) = opened(&app, fork.id, &sha)
        .await
        .expect("the fork opens the file it inherited");
    assert_eq!(bytes, PNG);
    assert_eq!(meta.filename.as_deref(), Some("chart.png"));
    assert_eq!(opened(&app, parent, &sha).await.unwrap().1, PNG);
    assert_eq!(
        store.len(),
        2,
        "one object per thread, none for anyone else"
    );
    // a resend copies nothing again and makes nothing
    let again = app
        .fork_and_send(
            &alice(),
            parent,
            fork_id(),
            1,
            None,
            first_message("echo again"),
        )
        .await
        .unwrap();
    assert!(!again.created);
    assert_eq!(store.len(), 2);
}

#[tokio::test]
async fn an_edit_fork_opens_the_files_before_the_message_it_replaces() {
    let w = World::new();
    let store = MemoryArtifacts::new();
    let (parent, events) = turn(
        &w,
        store.clone(),
        FileLimits::default(),
        vec![raw("chart", Some("image/png"), Some("chart.png"), PNG)],
    )
    .await;
    let sha = artifacts(&events)[0].file.as_ref().unwrap().sha256.clone();
    let app = app_with(&w, store.clone(), FileLimits::default());
    // a second message, the one to be edited (no dispatcher runs: the log is all an edit reads)
    app.post_message(&alice(), parent, "echo two".to_owned())
        .await
        .unwrap();
    let log = app.list_events(&alice(), parent, 0, 500).await.unwrap();
    let second = log
        .iter()
        .filter(|e| matches!(e.body, EventBody::UserMessage(_)))
        .nth(1)
        .unwrap()
        .seq;

    let fork = app
        .fork_thread(
            &alice(),
            parent,
            ForkRequest {
                at: ForkAt::Replace {
                    seq: second,
                    text: "echo edited".to_owned(),
                    message_id: Some("m-edit".to_owned()),
                },
                target: None,
                id: None,
            },
        )
        .await
        .unwrap()
        .thread;

    let (_, bytes) = opened(&app, fork.id, &sha)
        .await
        .expect("the edit fork opens the file of the turn it keeps");
    assert_eq!(bytes, PNG);
}

#[tokio::test]
async fn a_file_the_parent_never_kept_does_not_stop_a_fork() {
    let w = World::new();
    let store = MemoryArtifacts::new();
    let (parent, events) = turn(
        &w,
        store.clone(),
        FileLimits::default(),
        vec![raw("chart", Some("image/png"), Some("chart.png"), PNG)],
    )
    .await;
    let sha = artifacts(&events)[0].file.as_ref().unwrap().sha256.clone();
    // the object is gone (a store that was swapped, a purge): the log still refers to it
    let key: ArtifactKey = format!("threads/{parent}/{sha}").parse().unwrap();
    store.delete(&key).await.unwrap();
    let app = app_with(&w, store.clone(), FileLimits::default());

    let fork = app
        .fork_thread(
            &alice(),
            parent,
            ForkRequest {
                at: ForkAt::AfterTurn {
                    seq: 1,
                    first: None,
                },
                target: None,
                id: None,
            },
        )
        .await
        .expect("a fork is made though a file is missing")
        .thread;
    assert!(store.is_empty(), "nothing was invented for the fork");
    assert!(matches!(
        opened(&app, fork.id, &sha).await,
        Err(AppError::NotFound)
    ));
}

#[tokio::test]
async fn a_deployment_without_a_store_forks_a_log_that_names_files() {
    let w = World::new();
    let store = MemoryArtifacts::new();
    let (parent, _) = turn(
        &w,
        store,
        FileLimits::default(),
        vec![raw("chart", Some("image/png"), Some("chart.png"), PNG)],
    )
    .await;
    // the same log, read by a deployment that has no store any more
    let app = app_with(&w, NoArtifacts, FileLimits::default());
    app.fork_thread(
        &alice(),
        parent,
        ForkRequest {
            at: ForkAt::AfterTurn {
                seq: 1,
                first: None,
            },
            target: None,
            id: None,
        },
    )
    .await
    .expect("there is nothing to copy");
}

#[tokio::test]
async fn a_copy_that_fails_fails_the_fork_and_commits_nothing() {
    /// Keeps and serves files, and cannot copy one: a bucket that went away between the two calls.
    #[derive(Clone)]
    struct NoCopy(MemoryArtifacts);
    impl ArtifactStore for NoCopy {
        async fn put(
            &self,
            key: &ArtifactKey,
            bytes: Bytes,
            meta: &ArtifactMeta,
        ) -> Result<(), ArtifactError> {
            self.0.put(key, bytes, meta).await
        }
        async fn get(
            &self,
            key: &ArtifactKey,
        ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
            self.0.get(key).await
        }
        async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
            self.0.delete(key).await
        }
        async fn copy(&self, _: &ArtifactKey, _: &ArtifactKey) -> Result<(), ArtifactError> {
            Err(ArtifactError::unavailable(
                "bucket orchestrator-secret-bucket went away",
            ))
        }
    }
    let w = World::new();
    let store = NoCopy(MemoryArtifacts::new());
    let (parent, _) = turn(
        &w,
        store.clone(),
        FileLimits::default(),
        vec![raw("chart", Some("image/png"), Some("chart.png"), PNG)],
    )
    .await;
    let app = app_with(&w, store, FileLimits::default());
    let before = app
        .list_threads(&alice(), ThreadListing::recent(None, 10, true))
        .await
        .unwrap();

    let err = app
        .fork_thread(
            &alice(),
            parent,
            ForkRequest {
                at: ForkAt::AfterTurn {
                    seq: 1,
                    first: None,
                },
                target: None,
                id: None,
            },
        )
        .await
        .expect_err("the fork fails with its files");
    assert!(matches!(err, AppError::Artifacts(_)), "{err}");
    assert!(
        err.is_retryable(),
        "a failed copy can be tried again: {err}"
    );
    // nothing was committed: the same threads, the same log
    assert_eq!(
        app.list_threads(&alice(), ThreadListing::recent(None, 10, true))
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn a_copy_that_fails_fails_the_fork_with_its_message_and_commits_nothing() {
    /// Keeps and serves files, and cannot copy one.
    #[derive(Clone)]
    struct NoCopy(MemoryArtifacts);
    impl ArtifactStore for NoCopy {
        async fn put(
            &self,
            key: &ArtifactKey,
            bytes: Bytes,
            meta: &ArtifactMeta,
        ) -> Result<(), ArtifactError> {
            self.0.put(key, bytes, meta).await
        }
        async fn get(
            &self,
            key: &ArtifactKey,
        ) -> Result<Option<(ArtifactMeta, ByteStream)>, ArtifactError> {
            self.0.get(key).await
        }
        async fn delete(&self, key: &ArtifactKey) -> Result<(), ArtifactError> {
            self.0.delete(key).await
        }
        async fn copy(&self, _: &ArtifactKey, _: &ArtifactKey) -> Result<(), ArtifactError> {
            Err(ArtifactError::unavailable("the bucket went away"))
        }
    }
    let w = World::new();
    let store = NoCopy(MemoryArtifacts::new());
    let (parent, _) = turn(
        &w,
        store.clone(),
        FileLimits::default(),
        vec![raw("chart", Some("image/png"), Some("chart.png"), PNG)],
    )
    .await;
    let app = app_with(&w, store, FileLimits::default());
    let before = app
        .list_threads(&alice(), ThreadListing::recent(None, 10, true))
        .await
        .unwrap();

    let err = app
        .fork_and_send(
            &alice(),
            parent,
            fork_id(),
            1,
            None,
            first_message("echo again"),
        )
        .await
        .expect_err("the fork fails with its files");
    assert!(matches!(err, AppError::Artifacts(_)), "{err}");
    assert!(err.is_retryable(), "{err}");
    // no thread, no delegation: the same threads and nothing of the fork's
    assert_eq!(
        app.list_threads(&alice(), ThreadListing::recent(None, 10, true))
            .await
            .unwrap(),
        before
    );
    assert!(app.get_thread(&alice(), fork_id()).await.is_err());
    assert!(w.store.outbox_of(fork_id()).is_empty());
}
