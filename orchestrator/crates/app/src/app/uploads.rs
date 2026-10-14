//! Large files by a presigned upload (`uploads/v1`, ADR 0058, `docs/api/uploads-v1.md`): what the
//! deployment offers, the slot `request_upload` gives, and what the dispatcher needs to check the
//! id an agent hands back.
//!
//! The application decides; the surface (`orch-surface-thread-tools`) is the tool's face. Nothing
//! here holds state between calls: the id carries what the dispatcher must know and is signed with
//! the thread-tools key (`orch-thread-token`), the limits of a job are read off the log, and the
//! store signs the URL. The URL is a credential for its few minutes: nothing here logs it, and
//! nothing that reaches the event log holds it.

use std::collections::BTreeMap;
use std::time::Duration;

use jiff::Timestamp;
use orch_core::{
    AgentId, Caller, EventBody, EventKind, KnownExtension, ThreadId, UploadOffer, report,
};
use orch_ports::{
    ArtifactError, ArtifactKey, ArtifactMeta, ArtifactStore, Clock, Ports, ThreadStore,
};
use orch_thread_token::{ThreadToolsKeys, UploadClaims, mint_upload_id};

use super::App;
use crate::dispatcher::{canonical_media_type, clean_filename};

/// How many of a thread's newest artifact events are read to count what a job has kept. A job that
/// has logged more artifacts than this (a coder's checks, branches and texts count too) is counted
/// by its newest ones; the dispatcher counts again when the reference arrives.
const ARTIFACT_WINDOW: u32 = 2000;

/// What a deployment offers for large files (`artifacts.upload`, ADR 0058) and the key its upload
/// ids are signed with.
#[derive(Debug, Clone)]
pub struct UploadSettings {
    max_bytes: u64,
    ttl: Duration,
    keys: ThreadToolsKeys,
}

/// Why [`UploadSettings`] cannot be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("uploads need at least one byte and a lifetime of 30 to 3600 whole seconds")]
pub struct UploadSettingsError;

impl UploadSettings {
    /// The limits (`maxBytes` at least 1, `ttl` a whole number of seconds from 30 to 3600) and the
    /// keys the ids are signed with: the thread-tools keys, so any replica reads an id any replica
    /// issued.
    ///
    /// # Errors
    /// [`UploadSettingsError`] for numbers a message could not carry.
    pub fn new(
        max_bytes: u64,
        ttl: Duration,
        keys: ThreadToolsKeys,
    ) -> Result<Self, UploadSettingsError> {
        let settings = UploadSettings {
            max_bytes,
            ttl,
            keys,
        };
        if ttl.subsec_nanos() == 0 && settings.offer().is_valid() {
            Ok(settings)
        } else {
            Err(UploadSettingsError)
        }
    }

    /// The limits as the agent is told them.
    pub fn offer(&self) -> UploadOffer {
        UploadOffer {
            max_bytes: self.max_bytes,
            ttl_secs: self.ttl.as_secs(),
        }
    }

    /// The most bytes one uploaded file may be.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }

    /// How long a slot's URL works.
    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    /// The keys the ids are signed and read with.
    pub fn keys(&self) -> &ThreadToolsKeys {
        &self.keys
    }
}

/// What an agent asks a slot for: the file it has, as `request_upload` takes it. Built by
/// [`UploadRequest::parse`], which checks the shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadRequest {
    /// The name, cleaned as for any file (one name: no separator, control or direction character).
    pub filename: String,
    /// The media type the object is put with: `type/subtype` in lower case, no parameters, and for
    /// an image its canonical spelling (`image/jpg` is `image/jpeg`).
    pub media_type: String,
    /// The size in bytes, at least 1.
    pub size: u64,
    /// The SHA-256 of the content: the second half of the object's key.
    pub sha256: [u8; 32],
}

impl UploadRequest {
    /// The request an agent's arguments make.
    ///
    /// # Errors
    /// [`UploadRefusal::BadRequest`], saying what is wrong: a name that is nothing once cleaned, a
    /// type that is not `type/subtype`, a size of zero, a hash that is not 64 lowercase hexadecimal
    /// digits.
    pub fn parse(
        filename: &str,
        media_type: &str,
        size: u64,
        sha256: &str,
    ) -> Result<Self, UploadRefusal> {
        let bad = |what: &str| Err(UploadRefusal::BadRequest(what.to_owned()));
        let Some(filename) = clean_filename(filename) else {
            return bad("filename must be a file name: not empty, no path, no control character");
        };
        let Some(media_type) = canonical_media_type(media_type) else {
            return bad("mediaType must be a media type like application/pdf");
        };
        if size == 0 {
            return bad("size must be at least 1 byte: a file with no bytes goes in the message");
        }
        let Some(sha256) = unhex(sha256) else {
            return bad("sha256 must be 64 lowercase hexadecimal digits");
        };
        Ok(UploadRequest {
            filename,
            media_type,
            size,
            sha256,
        })
    }
}

/// 64 lowercase hex digits, or `None`.
fn unhex(text: &str) -> Option<[u8; 32]> {
    let digits = text.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let value = |d: u8| match d {
        b'0'..=b'9' => Some(d - b'0'),
        b'a'..=b'f' => Some(d - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 32];
    for (slot, pair) in out.iter_mut().zip(digits.chunks_exact(2)) {
        *slot = value(pair[0])? << 4 | value(pair[1])?;
    }
    Some(out)
}

/// Who asks: what the verified token of the thread-tools endpoint says.
#[derive(Debug, Clone)]
pub struct UploadCaller {
    /// The thread.
    pub thread: ThreadId,
    /// The job the token was minted for.
    pub job: u32,
    /// The agent the token was minted for.
    pub agent: AgentId,
    /// The addressed agent, or one it asked.
    pub caller: Caller,
}

/// A slot: the presigned `PUT` and the id to hand back.
///
/// `Debug` shows neither the URL nor a header value nor the id: the URL is a credential for its
/// minutes.
#[derive(Clone, PartialEq, Eq)]
pub struct UploadSlot {
    /// The id the agent names in its artifact.
    pub upload_id: String,
    /// Where to `PUT`: the host is the store's public endpoint.
    pub url: String,
    /// `PUT`.
    pub method: &'static str,
    /// Every header the request must send, with exactly these values.
    pub headers: BTreeMap<String, String>,
    /// When the URL stops working.
    pub expires_at: Timestamp,
}

impl std::fmt::Debug for UploadSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UploadSlot")
            .field("url", &"[redacted]")
            .field("upload_id", &"[redacted]")
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Why a slot was not given: the codes of the contract's table, a closed set (ADR 0004).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum UploadRefusal {
    /// No message of this task activated the extension: the deployment offers none, or the agent's
    /// live card no longer lists `uploads/v1` and `thread-tools/v1`.
    #[error("the extension is not activated for this agent: {0}")]
    NotActivated(&'static str),
    /// The size is over `maxBytes`.
    #[error("size is over the most one file may be: {max_bytes} bytes")]
    TooLarge {
        /// `artifacts.upload.maxBytes`.
        max_bytes: u64,
    },
    /// The job's file count or bytes would be exceeded.
    #[error("{0}")]
    OverLimit(String),
    /// The store cannot presign.
    #[error("this deployment's artifact store cannot take uploads; send the file in the message")]
    Unsupported,
    /// A member is missing or malformed.
    #[error("{0}")]
    BadRequest(String),
    /// The caller is an asked agent.
    #[error("only the agent the conversation is addressed to may upload a file")]
    CallerNotAllowed,
    /// The thread's task is over: the thread is finished, or the token belongs to an earlier job.
    #[error("this task is over")]
    TaskOver,
    /// The thread, the registry or the store could not be read now.
    #[error("temporarily unavailable; try again")]
    Unavailable,
}

impl UploadRefusal {
    /// The code of the contract's table (`docs/api/uploads-v1.md` section 3). `TaskOver` and
    /// `Unavailable` are not in it: the tool says them in words, as its siblings do.
    pub const fn code(&self) -> &'static str {
        match self {
            UploadRefusal::NotActivated(_) => "not_activated",
            UploadRefusal::TooLarge { .. } => "too_large",
            UploadRefusal::OverLimit(_) => "over_limit",
            UploadRefusal::Unsupported => "unsupported",
            UploadRefusal::BadRequest(_) => "bad_request",
            UploadRefusal::CallerNotAllowed => "caller_not_allowed",
            UploadRefusal::TaskOver => "task_over",
            UploadRefusal::Unavailable => "unavailable",
        }
    }
}

/// What a job has kept, read off the log: each content once, with its size.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JobFiles {
    kept: BTreeMap<String, u64>,
}

impl JobFiles {
    /// How many files the job has kept.
    pub fn files(&self) -> usize {
        self.kept.len()
    }

    /// How many bytes they are.
    pub fn bytes(&self) -> u64 {
        self.kept
            .values()
            .fold(0u64, |sum, size| sum.saturating_add(*size))
    }

    /// Whether the content `sha256` (64 hex digits) of `size` bytes fits under the job's limits:
    /// one the job has kept already is free, a new one needs a count and bytes to spare.
    pub fn admits(&self, files: u32, bytes: u64, sha256: &str, size: u64) -> bool {
        self.kept.contains_key(sha256)
            || (self.kept.len() < usize::try_from(files).unwrap_or(usize::MAX)
                && self.bytes().saturating_add(size) <= bytes)
    }
}

impl<P: Ports> App<P> {
    /// What the deployment offers for large files, or `None`: no `artifacts.upload`, or a store that
    /// cannot presign. The dispatcher puts it in the grant of the messages it sends; the adapter
    /// activates the extension only for a card that lists it.
    pub fn upload_offer(&self) -> Option<UploadOffer> {
        let settings = self.cfg.uploads.as_ref()?;
        self.ports
            .artifacts()
            .can_presign()
            .then(|| settings.offer())
    }

    /// The settings of uploads, whether or not the store can presign (the ingest of a reference
    /// needs the keys and the limits).
    pub fn upload_settings(&self) -> Option<&UploadSettings> {
        self.cfg.uploads.as_ref()
    }

    /// Gives a slot for the file `request` describes, to the addressed agent of `caller`'s thread,
    /// or says why not. Checks, in this order: the caller is the thread's addressed agent; the
    /// deployment offers uploads and the store can presign; the task is not over; the size is
    /// within `maxBytes`; the job's limits (the files and bytes it has kept, each content once)
    /// leave room; and, last because it is a call to the agent, its **live** card lists
    /// `uploads/v1` and `thread-tools/v1` (read now, never cached, ADR 0008; a card that cannot be
    /// read is `Unavailable`). Nothing is written: the store signs, and the id carries what the
    /// dispatcher will check.
    ///
    /// # Errors
    /// [`UploadRefusal`].
    pub async fn request_upload(
        &self,
        caller: &UploadCaller,
        request: &UploadRequest,
    ) -> Result<UploadSlot, UploadRefusal> {
        if !caller.caller.is_main() {
            return Err(UploadRefusal::CallerNotAllowed);
        }
        let Some(settings) = &self.cfg.uploads else {
            return Err(UploadRefusal::NotActivated(
                "this deployment offers no uploads",
            ));
        };
        let store = self.ports.artifacts();
        if !store.can_presign() {
            return Err(UploadRefusal::Unsupported);
        }
        let thread = caller.thread;
        let record = match self.ports.store().get_thread(None, thread).await {
            Ok(Some(record)) => record,
            Ok(None) => return Err(UploadRefusal::TaskOver),
            Err(error) => {
                tracing::warn!(%thread, error = %report(&error), "request_upload could not read the thread");
                return Err(UploadRefusal::Unavailable);
            }
        };
        if record.state.is_terminal() || record.job.number != caller.job {
            return Err(UploadRefusal::TaskOver);
        }
        // What can be known without calling the agent comes first: its card is a network call.
        if request.size > settings.max_bytes {
            return Err(UploadRefusal::TooLarge {
                max_bytes: settings.max_bytes,
            });
        }
        let limits = self.cfg.files;
        let kept = self.job_files(thread, caller.job).await.map_err(|error| {
            tracing::warn!(%thread, error = %report(&error), "request_upload could not count the job's files");
            UploadRefusal::Unavailable
        })?;
        let sha256_hex = ArtifactKey::new(thread, request.sha256).sha256_hex();
        if !kept.admits(
            limits.max_files_per_job,
            limits.max_per_job_bytes,
            &sha256_hex,
            request.size,
        ) {
            return Err(UploadRefusal::OverLimit(format!(
                "this job has kept {} of at most {} files and {} of at most {} bytes; \
                 this file of {} bytes does not fit",
                kept.files(),
                limits.max_files_per_job,
                kept.bytes(),
                limits.max_per_job_bytes,
                request.size,
            )));
        }
        self.require_activated(&caller.agent).await?;

        let key = ArtifactKey::new(thread, request.sha256);
        let meta = ArtifactMeta {
            media_type: request.media_type.clone(),
            size: request.size,
            sha256: request.sha256,
            filename: Some(request.filename.clone()),
        };
        let signed = store.presign_put(&key, &meta, settings.ttl).await.map_err(
            |error| match error {
                ArtifactError::Unsupported | ArtifactError::NotConfigured => {
                    UploadRefusal::Unsupported
                }
                ArtifactError::Invalid(why) => UploadRefusal::BadRequest(why),
                other => {
                    tracing::warn!(%thread, error = %report(&other), "a slot could not be signed");
                    UploadRefusal::Unavailable
                }
            },
        )?;
        // The URL stops working `ttl` after the store signed it: a moment before this instant.
        let expires_at = whole_seconds(self.ports.clock().now())
            .checked_add(jiff::SignedDuration::from_secs(
                i64::try_from(settings.ttl.as_secs()).unwrap_or(i64::MAX),
            ))
            .map_err(|_| UploadRefusal::Unavailable)?;
        let upload_id = mint_upload_id(
            &settings.keys,
            &UploadClaims {
                thread,
                job: caller.job,
                sha256: request.sha256,
                size: request.size,
                media_type: request.media_type.clone(),
                expires_at,
            },
        )
        .map_err(|error| {
            tracing::error!(%thread, %error, "an upload id could not be written");
            UploadRefusal::Unavailable
        })?;
        tracing::info!(%thread, job = caller.job, size = request.size, "an upload slot was issued");
        Ok(UploadSlot {
            upload_id,
            url: signed.url,
            method: "PUT",
            headers: signed.headers,
            expires_at,
        })
    }

    /// Whether the agent's live card lists `uploads/v1` and `thread-tools/v1` **now**: what activates
    /// the extension on the messages it is sent, read for this call and never remembered
    /// (ADR 0008). A card that cannot be read is not activation (fail closed), and it is
    /// [`UploadRefusal::Unavailable`], not `NotActivated`: the agent did not say no, the answer is
    /// not known, and a retry can mend it.
    async fn require_activated(&self, agent: &AgentId) -> Result<(), UploadRefusal> {
        let not = |why| Err(UploadRefusal::NotActivated(why));
        let entry = match self.resolve_agent(agent).await {
            Ok(Some(entry)) => entry,
            Ok(None) => return not("the agent is no longer listed"),
            Err(error) => {
                tracing::warn!(%agent, error = %report(&error), "request_upload could not resolve the agent");
                return Err(UploadRefusal::Unavailable);
            }
        };
        match self.live_card(&entry.endpoint).await {
            Some(card)
                if card.offers(KnownExtension::Uploads)
                    && card.offers(KnownExtension::ThreadTools) =>
            {
                Ok(())
            }
            Some(_) => not("the agent's card does not list uploads/v1 and thread-tools/v1"),
            None => {
                tracing::warn!(%agent, "request_upload could not read the agent's card");
                Err(UploadRefusal::Unavailable)
            }
        }
    }

    /// Whether the live card of `agent` lists `uploads/v1` and `thread-tools/v1` now: the
    /// thread-tools endpoint offers `request_upload` to an agent that does.
    pub async fn uploads_listed(&self, agent: &AgentId) -> bool {
        self.require_activated(agent).await.is_ok()
    }

    /// The files `job` of `thread` has kept, read off the log: the `artifact` events with a file
    /// since the job began (its `job_started`, or the start of the log for job 1), each content
    /// once. Bounded: the newest [`ARTIFACT_WINDOW`] artifacts are read.
    ///
    /// # Errors
    /// The store's, when it cannot be read.
    pub async fn job_files(
        &self,
        thread: ThreadId,
        job: u32,
    ) -> Result<JobFiles, orch_ports::StoreError> {
        let store = self.ports.store();
        let began = if job <= 1 {
            0
        } else {
            store
                .latest_events(thread, EventKind::JobStarted, 1)
                .await?
                .into_iter()
                .find_map(|event| match event.body {
                    EventBody::JobStarted(started) if started.job == job => Some(event.seq),
                    _ => None,
                })
                // no `job_started` for this job in the newest one: the log is not as the ledger says;
                // count nothing rather than the whole thread's files
                .unwrap_or(i64::MAX)
        };
        let mut kept = BTreeMap::new();
        for event in store
            .latest_events(thread, EventKind::Artifact, ARTIFACT_WINDOW)
            .await?
        {
            if event.seq <= began {
                break;
            }
            if let EventBody::Artifact(artifact) = event.body
                && let Some(file) = artifact.file
            {
                kept.insert(file.sha256, file.size);
            }
        }
        Ok(JobFiles { kept })
    }

    /// Whether the log of `thread` holds a file with the content `sha256` (64 hex digits), in any
    /// job, **or may**: the newest [`ARTIFACT_WINDOW`] artifacts are read, and when the thread has
    /// at least that many the older ones were not seen, so the answer is "may". The dispatcher
    /// deletes an upload it refused only when this is `false`: the object of a key is also the
    /// file of every reference to the same content, and a reference must go on working.
    ///
    /// # Errors
    /// The store's, when it cannot be read.
    pub async fn thread_may_hold_file(
        &self,
        thread: ThreadId,
        sha256: &str,
    ) -> Result<bool, orch_ports::StoreError> {
        let newest = self
            .ports
            .store()
            .latest_events(thread, EventKind::Artifact, ARTIFACT_WINDOW)
            .await?;
        let held = newest.iter().any(|event| {
            matches!(&event.body, EventBody::Artifact(artifact)
                if artifact.file.as_ref().is_some_and(|file| file.sha256 == sha256))
        });
        Ok(held || window_is_full(newest.len()))
    }
}

/// Whether a read of the newest artifacts came back full, so that older ones may exist unseen.
fn window_is_full(read: usize) -> bool {
    read >= usize::try_from(ARTIFACT_WINDOW).unwrap_or(usize::MAX)
}

/// `at` with the sub-second part dropped: an id carries whole seconds.
fn whole_seconds(at: Timestamp) -> Timestamp {
    Timestamp::from_second(at.as_second()).unwrap_or(at)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const HASH: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn a_request_is_cleaned_and_checked() {
        let ok = UploadRequest::parse(
            "../../My Report.pdf",
            "Application/PDF; charset=x",
            12,
            HASH,
        )
        .unwrap();
        assert_eq!(ok.filename, "My Report.pdf");
        assert_eq!(ok.media_type, "application/pdf");
        assert_eq!(ok.size, 12);
        assert_eq!(ok.sha256[0], 0x2c);
        for (filename, media_type, size, sha) in [
            ("..", "application/pdf", 1, HASH),
            ("", "application/pdf", 1, HASH),
            ("a.pdf", "pdf", 1, HASH),
            ("a.pdf", "", 1, HASH),
            ("a.pdf", "application/pdf", 0, HASH),
            ("a.pdf", "application/pdf", 1, &HASH.to_uppercase()),
            ("a.pdf", "application/pdf", 1, &HASH[..63]),
            ("a.pdf", "application/pdf", 1, "not hex"),
        ] {
            let err = UploadRequest::parse(filename, media_type, size, sha).unwrap_err();
            assert!(
                matches!(err, UploadRefusal::BadRequest(_)),
                "{filename:?} {media_type:?} {size} {sha:?}: {err}"
            );
            assert_eq!(err.code(), "bad_request");
        }
    }

    #[test]
    fn the_media_type_is_the_canonical_spelling_the_ingest_compares_with() {
        for (given, canonical) in [
            ("image/jpg", "image/jpeg"),
            ("Image/PJPEG; charset=x", "image/jpeg"),
            ("image/svg", "image/svg+xml"),
            ("image/png", "image/png"),
            ("application/PDF", "application/pdf"),
            ("text/plain; charset=utf-8", "text/plain"),
        ] {
            let request = UploadRequest::parse("a", given, 1, HASH).unwrap();
            assert_eq!(request.media_type, canonical, "{given}");
        }
    }

    #[test]
    fn a_full_window_hides_older_artifacts() {
        assert!(!window_is_full(0));
        assert!(!window_is_full(ARTIFACT_WINDOW as usize - 1));
        assert!(window_is_full(ARTIFACT_WINDOW as usize));
    }

    #[test]
    fn the_codes_are_the_contracts() {
        let codes = [
            (UploadRefusal::NotActivated("x"), "not_activated"),
            (UploadRefusal::TooLarge { max_bytes: 1 }, "too_large"),
            (UploadRefusal::OverLimit("x".into()), "over_limit"),
            (UploadRefusal::Unsupported, "unsupported"),
            (UploadRefusal::BadRequest("x".into()), "bad_request"),
            (UploadRefusal::CallerNotAllowed, "caller_not_allowed"),
        ];
        for (refusal, code) in codes {
            assert_eq!(refusal.code(), code);
        }
    }

    #[test]
    fn a_job_counts_each_content_once_and_a_known_one_is_free() {
        let kept = JobFiles {
            kept: BTreeMap::from([("a".repeat(64), 6), ("b".repeat(64), 4)]),
        };
        assert_eq!((kept.files(), kept.bytes()), (2, 10));
        let (a, c) = ("a".repeat(64), "c".repeat(64));
        assert!(kept.admits(3, 100, &c, 90));
        assert!(!kept.admits(3, 100, &c, 91), "10 + 91 is over 100");
        assert!(!kept.admits(2, 100, &c, 1), "two files are the most");
        assert!(
            kept.admits(2, 10, &a, 6),
            "a content the job has kept costs nothing"
        );
    }

    #[test]
    fn a_slot_shows_neither_its_url_nor_its_id() {
        let slot = UploadSlot {
            upload_id: "secret-id".into(),
            url: "https://files.example/b/k?X-Amz-Signature=deadbeef".into(),
            method: "PUT",
            headers: BTreeMap::from([("content-type".into(), "text/plain".into())]),
            expires_at: Timestamp::UNIX_EPOCH,
        };
        let shown = format!("{slot:?}");
        assert!(
            !shown.contains("deadbeef")
                && !shown.contains("secret-id")
                && !shown.contains("files.example"),
            "{shown}"
        );
    }
}
