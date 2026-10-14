//! Downloads by a redirect to a presigned `GET` (ADR 0058 A1, step 6b), through the application:
//! the checks of the three file routes are one function with the open (`readable_*`), a redirect is
//! offered only when the deployment asked for it and the store can sign, a shared link gets a
//! shorter life, and the meta of a file comes from the store without its bytes.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use orch_app::{
    App, AppConfig, AppError, DownloadMode, NewThread, Permission, Policy, PublicView, RoleGrant,
    ShareKeys, SharingMode, SharingSettings,
};
use orch_core::{
    Actor, ArtifactData, Classify as _, ErrorClass, EventBody, FileRef, ShareLevel, ThreadId,
    ThreadRecord, UserId,
};
use orch_ports::memory::{
    MemoryArtifacts, MemoryStore, MemoryWakeup, ScriptedAgent, ScriptedModel, SeqIds,
};
use orch_ports::{
    ArtifactError, ArtifactHead, ArtifactKey, ArtifactMeta, ArtifactStore, ByteStream, Commit,
    FixedRegistry, NewEvent, NoArtifacts, PortSet, PresignedGet, PresignedPut, Principal,
    ResponseOverrides, Role, SystemClock, ThreadStore,
};
use secrecy::SecretString;
use support::*;

type DownloadPorts<X> = PortSet<
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

const SECRET: &str = "0123456789abcdef0123456789abcdef";

fn sharing() -> SharingSettings {
    SharingSettings::new(
        SharingMode::Public,
        Some(ShareKeys::new(SecretString::from(SECRET.to_owned()), None).unwrap()),
        PublicView {
            step_io: false,
            files: true,
        },
    )
    .unwrap()
}

fn app_with<X: ArtifactStore>(
    w: &World,
    store: X,
    downloads: DownloadMode,
) -> Arc<App<DownloadPorts<X>>> {
    app_with_policy(w, store, downloads, Policy::default())
}

fn app_with_policy<X: ArtifactStore>(
    w: &World,
    store: X,
    downloads: DownloadMode,
    policy: Policy,
) -> Arc<App<DownloadPorts<X>>> {
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
                downloads,
                sharing: sharing(),
                policy,
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

fn redirect(secs: u64) -> DownloadMode {
    DownloadMode::redirect(Duration::from_secs(secs)).unwrap()
}

fn new_thread() -> NewThread {
    NewThread {
        title: None,
        target: target("plain"),
        text: "hi".to_owned(),
    }
}

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

/// A thread of alice's whose log names a file that is in the store, shared as `level`; the file's
/// hash and the link's token.
async fn shared_with_a_file<X: ArtifactStore>(
    w: &World,
    app: &App<DownloadPorts<X>>,
    store: &MemoryArtifacts,
    level: ShareLevel,
) -> (ThreadRecord, String, String) {
    let alice = principal("alice@example.com", &["user"]);
    let t = app.create_thread(&alice, new_thread()).await.unwrap();
    let bytes = Bytes::from_static(b"PK a report");
    let meta = ArtifactMeta::of("application/zip", Some("r&d report.zip".to_owned()), &bytes);
    let key = ArtifactKey::new(t.id, meta.sha256);
    store.put(&key, bytes, &meta).await.unwrap();
    let sha = key.sha256_hex();
    let record = w.store.get_thread(None, t.id).await.unwrap().unwrap();
    w.store
        .commit(
            t.id,
            record.version,
            Commit {
                new_state: record.state,
                job: None,
                events: vec![NewEvent {
                    at: jiff::Timestamp::now(),
                    actor: Actor::agent(&orch_core::AgentId::new("plain"), None),
                    body: EventBody::Artifact(ArtifactData {
                        name: "report".into(),
                        mime_type: Some("application/zip".into()),
                        uri: None,
                        text: None,
                        file: Some(FileRef {
                            sha256: sha.clone(),
                            size: meta.size,
                            filename: meta.filename.clone(),
                        }),
                    }),
                    idempotency_key: None,
                }],
                outbox: vec![],
                binding: None,
                now: jiff::Timestamp::now(),
                lease: None,
                watches: vec![],
                timers: vec![],
                inbox: None,
                finishes_outbox: None,
                title: None,
                description: None,
                sharing: None,
                skip_unsent_delegates: false,
            },
        )
        .await
        .unwrap();
    app.share_thread(&alice, t.id, level).await.unwrap();
    let shared = w.store.get_thread(None, t.id).await.unwrap().unwrap();
    let token = app
        .share_view(&shared)
        .unwrap()
        .url
        .unwrap()
        .strip_prefix("/s/")
        .unwrap()
        .to_owned();
    (shared, sha, token)
}

fn is_not_found<T>(result: Result<T, AppError>) {
    match result {
        Err(AppError::NotFound) => {}
        Err(other) => panic!("expected NotFound, got {other:?}"),
        Ok(_) => panic!("expected NotFound, got a success"),
    }
}

#[test]
fn a_redirect_is_a_lifetime_a_signature_can_carry_and_stream_is_the_default() {
    assert_eq!(DownloadMode::default(), DownloadMode::Stream);
    assert!(AppConfig::default().downloads == DownloadMode::Stream);
    for secs in [30, 300, 3600] {
        assert_eq!(
            DownloadMode::redirect(Duration::from_secs(secs)).unwrap(),
            DownloadMode::Redirect {
                ttl: Duration::from_secs(secs)
            }
        );
    }
    for bad in [
        Duration::ZERO,
        Duration::from_secs(29),
        Duration::from_secs(3601),
        Duration::from_millis(30_500),
    ] {
        assert!(DownloadMode::redirect(bad).is_err(), "{bad:?}");
    }
}

#[tokio::test]
async fn a_redirect_is_offered_only_when_asked_for_and_the_store_can_sign_and_a_link_gets_a_minute()
{
    let w = World::new();
    let store = MemoryArtifacts::presigning();
    let app = app_with(&w, store.clone(), redirect(300));
    let alice = principal("alice@example.com", &["user"]);
    let bob = principal("bob@example.com", &["user"]);
    let (t, sha, token) = shared_with_a_file(&w, &app, &store, ShareLevel::Public).await;

    // the owner: the lifetime of the deployment; a signed-in reader and an anonymous one of a link:
    // at most a minute (a revoked link must stop within it)
    let own = app.readable_artifact(&alice, t.id, &sha).await.unwrap();
    assert_eq!(app.redirect_ttl(&own), Some(Duration::from_secs(300)));
    let signed_in = app
        .readable_shared_artifact(&bob, &token, &sha)
        .await
        .unwrap();
    assert_eq!(app.redirect_ttl(&signed_in), Some(Duration::from_secs(60)));
    let anonymous = app.readable_public_artifact(&token, &sha).await.unwrap();
    assert_eq!(app.redirect_ttl(&anonymous), Some(Duration::from_secs(60)));

    // a deployment whose lifetime is already short keeps it for a link too
    let short = app_with(&w, store.clone(), redirect(30));
    let anonymous = short.readable_public_artifact(&token, &sha).await.unwrap();
    assert_eq!(
        short.redirect_ttl(&anonymous),
        Some(Duration::from_secs(30))
    );

    // streaming by choice, and a store that cannot sign, redirect nothing
    let streaming = app_with(&w, store.clone(), DownloadMode::Stream);
    let own = streaming
        .readable_artifact(&alice, t.id, &sha)
        .await
        .unwrap();
    assert_eq!(streaming.redirect_ttl(&own), None);
    let plain = MemoryArtifacts::new();
    let cannot = app_with(&w, plain, redirect(300));
    let own = cannot.readable_artifact(&alice, t.id, &sha).await.unwrap();
    assert_eq!(cannot.redirect_ttl(&own), None);
    let nothing = app_with(&w, NoArtifacts, redirect(300));
    let own = nothing.readable_artifact(&alice, t.id, &sha).await.unwrap();
    assert_eq!(nothing.redirect_ttl(&own), None);
}

#[tokio::test]
async fn the_checks_of_the_three_routes_are_the_open_ones_and_every_miss_is_the_same_answer() {
    let w = World::new();
    let store = MemoryArtifacts::presigning();
    let app = app_with(&w, store.clone(), redirect(300));
    let alice = principal("alice@example.com", &["user"]);
    let bob = principal("bob@example.com", &["user"]);
    let (t, sha, token) = shared_with_a_file(&w, &app, &store, ShareLevel::Internal).await;

    // another person's thread is a thread that does not exist, an administrator's included
    for who in [&bob, &principal("root@example.com", &["admin"])] {
        is_not_found(app.readable_artifact(who, t.id, &sha).await);
        is_not_found(app.open_artifact(who, t.id, &sha).await);
    }
    // a thread that is not there, a hash that is not a hash
    is_not_found(
        app.readable_artifact(&alice, ThreadId(uuid::Uuid::from_u128(7)), &sha)
            .await,
    );
    for bad in ["", "nope", &"A".repeat(64), &"f".repeat(63)] {
        is_not_found(app.readable_artifact(&alice, t.id, bad).await);
        is_not_found(app.readable_shared_artifact(&bob, &token, bad).await);
    }
    // a hash the log does not name, through a link
    is_not_found(
        app.readable_shared_artifact(&bob, &token, &"f".repeat(64))
            .await,
    );
    // an internal link is no public one
    is_not_found(app.readable_public_artifact(&token, &sha).await);
    // a bad link
    is_not_found(app.readable_shared_artifact(&bob, "not-a-link", &sha).await);

    // a role without `artifact.read` is told so, for any hash: the permission comes first
    let no_files = Policy::new(
        BTreeMap::from([(
            Role::new("reader"),
            RoleGrant {
                permissions: BTreeSet::from([Permission::ThreadRead]),
                ..RoleGrant::none()
            },
        )]),
        Some(Role::new("reader")),
    )
    .unwrap();
    let strict = app_with_policy(&w, store.clone(), redirect(300), no_files);
    let reader = principal("alice@example.com", &["reader"]);
    for sha in [&sha, &"f".repeat(64)] {
        match strict.readable_artifact(&reader, t.id, sha).await {
            Err(AppError::Forbidden { permission, .. }) => {
                assert_eq!(permission, Permission::ArtifactRead);
            }
            other => panic!("expected Forbidden, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn the_meta_comes_from_the_store_without_the_bytes_and_a_file_that_is_gone_is_not_found() {
    let w = World::new();
    let store = MemoryArtifacts::presigning();
    let app = app_with(&w, store.clone(), redirect(300));
    let alice = principal("alice@example.com", &["user"]);
    let (t, sha, _) = shared_with_a_file(&w, &app, &store, ShareLevel::Internal).await;

    let file = app.readable_artifact(&alice, t.id, &sha).await.unwrap();
    let meta = app.readable_meta(&file).await.unwrap();
    assert_eq!(meta.media_type, "application/zip");
    assert_eq!(meta.filename.as_deref(), Some("r&d report.zip"));
    assert_eq!(meta.size, 11);
    assert_eq!(hex(&meta.sha256), sha);

    // the key is the thread's: a file of the same hash in another thread is another file
    let key = ArtifactKey::parse(&format!("threads/{}/{sha}", t.id)).unwrap();
    store.delete(&key).await.unwrap();
    is_not_found(app.readable_meta(&file).await);
    is_not_found(app.open_readable(&file).await);
}

#[tokio::test]
async fn a_download_is_signed_for_the_key_with_the_overrides_and_a_wrong_request_is_a_bug_not_a_400()
 {
    let w = World::new();
    let store = MemoryArtifacts::presigning();
    let app = app_with(&w, store.clone(), redirect(300));
    let alice = principal("alice@example.com", &["user"]);
    let (t, sha, _) = shared_with_a_file(&w, &app, &store, ShareLevel::Internal).await;
    let file = app.readable_artifact(&alice, t.id, &sha).await.unwrap();

    let overrides = ResponseOverrides {
        content_disposition: Some("attachment; filename=\"r_d report.zip\"".to_owned()),
        content_type: Some("application/octet-stream".to_owned()),
        cache_control: Some("private, max-age=300".to_owned()),
    };
    let get = app
        .presign_download(&file, &overrides, Duration::from_secs(300))
        .await
        .unwrap();
    let served = store.download(&get, jiff::Timestamp::now()).unwrap();
    assert_eq!(served.bytes, "PK a report");
    assert_eq!(served.content_disposition, overrides.content_disposition);
    assert_eq!(served.content_type, "application/octet-stream");
    assert_eq!(served.cache_control, overrides.cache_control);
    // the URL is shown by nothing
    assert!(!format!("{get:?}").contains("memory://"));

    // the route builds the overrides: one a header cannot carry is our bug, a 500 and not a 400
    let bad = ResponseOverrides {
        content_disposition: Some("attachment\r\nx-evil: 1".to_owned()),
        ..overrides.clone()
    };
    let err = app
        .presign_download(&file, &bad, Duration::from_secs(300))
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Internal { .. }), "{err:?}");
    assert_eq!(err.class(), ErrorClass::Internal);
}

/// How the store fails the two calls a redirect makes.
#[derive(Clone, Copy, Debug)]
enum Trouble {
    Unavailable,
    Unauthenticated,
    Unsupported,
    NotConfigured,
    /// (`head` only) the object is there and says no content type.
    NoContentType,
}

impl Trouble {
    fn error(self) -> ArtifactError {
        match self {
            Trouble::Unavailable => ArtifactError::unavailable("the bucket is down"),
            Trouble::Unauthenticated => ArtifactError::Unauthenticated,
            Trouble::Unsupported => ArtifactError::Unsupported,
            Trouble::NotConfigured | Trouble::NoContentType => ArtifactError::NotConfigured,
        }
    }
}

/// The in-memory bucket that fails `head` and `presign_get` the way it is told to.
#[derive(Clone)]
struct Flaky {
    inner: MemoryArtifacts,
    head: Arc<Mutex<Option<Trouble>>>,
    sign: Arc<Mutex<Option<Trouble>>>,
}

impl ArtifactStore for Flaky {
    async fn put(
        &self,
        key: &ArtifactKey,
        bytes: Bytes,
        meta: &ArtifactMeta,
    ) -> Result<(), ArtifactError> {
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

    async fn delete_prefix(&self, thread: ThreadId) -> Result<u64, ArtifactError> {
        self.inner.delete_prefix(thread).await
    }

    fn can_presign(&self) -> bool {
        self.inner.can_presign()
    }

    async fn presign_put(
        &self,
        key: &ArtifactKey,
        meta: &ArtifactMeta,
        ttl: Duration,
    ) -> Result<PresignedPut, ArtifactError> {
        self.inner.presign_put(key, meta, ttl).await
    }

    async fn presign_get(
        &self,
        key: &ArtifactKey,
        overrides: &ResponseOverrides,
        ttl: Duration,
    ) -> Result<PresignedGet, ArtifactError> {
        let trouble = *self.sign.lock().unwrap();
        if let Some(trouble) = trouble {
            return Err(trouble.error());
        }
        self.inner.presign_get(key, overrides, ttl).await
    }

    async fn head(&self, key: &ArtifactKey) -> Result<Option<ArtifactHead>, ArtifactError> {
        let trouble = *self.head.lock().unwrap();
        match trouble {
            None => self.inner.head(key).await,
            Some(Trouble::NoContentType) => {
                let mut head = self.inner.head(key).await?;
                if let Some(head) = &mut head {
                    head.media_type = None;
                }
                Ok(head)
            }
            Some(trouble) => Err(trouble.error()),
        }
    }
}

#[tokio::test]
async fn a_store_that_fails_to_say_or_to_sign_is_the_error_it_is_and_only_a_missing_store_is_not_found()
 {
    let w = World::new();
    let memory = MemoryArtifacts::presigning();
    let store = Flaky {
        inner: memory.clone(),
        head: Arc::default(),
        sign: Arc::default(),
    };
    let app = app_with(&w, store.clone(), redirect(300));
    let alice = principal("alice@example.com", &["user"]);
    let (t, sha, _) = shared_with_a_file(&w, &app, &memory, ShareLevel::Internal).await;
    let file = app.readable_artifact(&alice, t.id, &sha).await.unwrap();
    let overrides = ResponseOverrides {
        content_disposition: Some("attachment; filename=\"r.zip\"".to_owned()),
        content_type: Some("application/octet-stream".to_owned()),
        cache_control: Some("private, max-age=300".to_owned()),
    };
    let ttl = Duration::from_secs(300);

    // the `HEAD`
    for (trouble, class) in [
        (Trouble::Unavailable, ErrorClass::Transient),
        (Trouble::Unauthenticated, ErrorClass::Unauthenticated),
        (Trouble::Unsupported, ErrorClass::Internal),
        (Trouble::NoContentType, ErrorClass::Corrupt),
    ] {
        *store.head.lock().unwrap() = Some(trouble);
        let err = app.readable_meta(&file).await.unwrap_err();
        assert_eq!(err.class(), class, "{trouble:?}: {err:?}");
    }
    // a store that can sign says what it holds: one that cannot is a bug of the deployment, not the caller's
    *store.head.lock().unwrap() = Some(Trouble::Unsupported);
    assert!(matches!(
        app.readable_meta(&file).await.unwrap_err(),
        AppError::Internal { .. }
    ));
    // a deployment with no store has no file: the 404 `open_readable` already says
    *store.head.lock().unwrap() = Some(Trouble::NotConfigured);
    is_not_found(app.readable_meta(&file).await);
    *store.head.lock().unwrap() = None;

    // the signing says the same of a store that is not configured, and nothing else changes
    for (trouble, class) in [
        (Trouble::Unavailable, ErrorClass::Transient),
        (Trouble::Unauthenticated, ErrorClass::Unauthenticated),
        (Trouble::Unsupported, ErrorClass::Internal),
    ] {
        *store.sign.lock().unwrap() = Some(trouble);
        let err = app
            .presign_download(&file, &overrides, ttl)
            .await
            .unwrap_err();
        assert_eq!(err.class(), class, "{trouble:?}: {err:?}");
    }
    *store.sign.lock().unwrap() = Some(Trouble::NotConfigured);
    is_not_found(app.presign_download(&file, &overrides, ttl).await);
    *store.sign.lock().unwrap() = None;
    assert!(app.presign_download(&file, &overrides, ttl).await.is_ok());
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
