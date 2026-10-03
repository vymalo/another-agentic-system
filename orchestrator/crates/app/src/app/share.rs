//! Sharing a thread by a revocable link (ADR 0040): what its owner does, and what somebody who has
//! the link may read.
//!
//! **The owner** ([`App::share_thread`], [`App::rotate_share`], [`App::unshare_thread`]) holds
//! `thread.share` over their own thread, within the deployment's cap. Each is one commit of the
//! thread: the event (`thread_shared` with the digest of the nonce, or `thread_unshared`) and the
//! row's share, together. Revoking needs only ownership, so a role that loses `thread.share`, or a
//! deployment that withholds it, never leaves a link up that its owner cannot take down.
//!
//! **A reader** ([`App::open_shared`], [`App::open_public`]) has a token. The nonce in it finds the
//! thread, the MAC is checked in constant time, and the thread is served at
//! `min(visibility, cap)`, computed now and never stored. Every way a link can fail is the same
//! [`AppError::NotFound`]: an unknown token, a bad MAC, a private or revoked thread, a cap that
//! has been lowered, a file that is not the thread's. What the reader sees of the log is the
//! [reader projection](crate::reader).

use std::sync::Arc;

use futures::StreamExt;
use futures::stream::BoxStream;
use orch_core::{
    EventBody, EventKind, Input, ShareLevel, ShareNonce, ThreadId, ThreadRecord, Visibility,
};
use orch_ports::{ArtifactMeta, ByteStream, IdGen, Ports, ThreadStore};
use tokio::time::Instant;

use super::{ApplyOutcome, FeedItem};
use crate::reader::{ReaderAudience, ReaderRules, SharedThreadView, reader_event, reader_thread};
use crate::share_link::ShareKeys;
use crate::sharing::{ShareAction, ShareView, SharingMode, SharingSettings, SharingStats};
use crate::{Access, App, AppError, Denied, Permission, Requester, Resource};

/// The most artifact events read to tell whether a file is one the thread's log names.
const NAMED_FILES_WINDOW: u32 = 1000;

/// A thread opened through its link: what the reader is allowed to read of it, decided once.
#[derive(Debug, Clone)]
pub struct SharedRead {
    thread: ThreadRecord,
    nonce: ShareNonce,
    effective: Visibility,
    rules: ReaderRules,
    is_owner: bool,
}

impl SharedRead {
    /// The thread, as the store holds it. **Not for a reader**: it names the owner. What a reader
    /// is given is [`App::shared_view`], and the log goes through [`App::shared_feed`].
    pub fn thread(&self) -> &ThreadRecord {
        &self.thread
    }

    /// What the thread is served at: `min(visibility, cap)`, `internal` or `public`.
    pub fn effective(&self) -> Visibility {
        self.effective
    }

    /// What this reader may see of the log.
    pub fn rules(&self) -> &ReaderRules {
        &self.rules
    }

    /// Whether the reader is the thread's owner (a signed-in read of their own link).
    pub fn is_owner(&self) -> bool {
        self.is_owner
    }
}

impl<P: Ports> App<P> {
    /// What the deployment says about sharing (`sharing` of the configuration).
    pub fn sharing(&self) -> &SharingSettings {
        &self.cfg.sharing
    }

    /// The counters of sharing in this process (ADR 0040, section 11).
    pub fn sharing_stats(&self) -> SharingStats {
        self.sharing_counters.snapshot()
    }

    /// The cap `GET /api/me` tells `who`: the deployment's, when their roles hold `thread.share`,
    /// else `disabled`. A hint for the web, never a check.
    pub fn sharing_mode_for(&self, who: &impl Requester) -> SharingMode {
        if self.access(who).has(Permission::ThreadShare) {
            self.cfg.sharing.mode()
        } else {
            SharingMode::Disabled
        }
    }

    /// What the owner is told of `thread`'s share, or `None` for a thread that is private. **Only
    /// ever for the owner**: it carries the link.
    pub fn share_view(&self, thread: &ThreadRecord) -> Option<ShareView> {
        let share = thread.share?;
        let settings = &self.cfg.sharing;
        Some(ShareView {
            visibility: share.level,
            effective: settings.mode().effective(share.level.visibility()),
            shared_at: share.shared_at,
            // Recomputed from the row under the current secret: nothing secret is stored, and the
            // owner can copy the link again whatever secret it was first made under.
            url: settings
                .keys()
                .map(|keys| settings.url_of(&keys.token(thread.id, &share.nonce))),
        })
    }

    /// The thread `id` for its owner to change who may read it: `thread.share` first (so a
    /// refusal for the missing permission is the same for every id), then the thread, which is
    /// [`AppError::NotFound`] when it is not the person's.
    async fn owned_for_sharing(
        &self,
        access: &Access<'_>,
        id: ThreadId,
    ) -> Result<ThreadRecord, AppError> {
        if !access.has(Permission::ThreadShare) {
            return Err(AppError::missing_permission(Permission::ThreadShare));
        }
        let thread = self
            .ports
            .store()
            .get_thread(None, id)
            .await?
            .ok_or(AppError::NotFound)?;
        match access.check(
            Permission::ThreadShare,
            &Resource::Thread {
                owner: &thread.owner,
            },
        ) {
            Ok(()) => Ok(thread),
            Err(Denied::Permission(p)) => Err(AppError::missing_permission(p)),
            Err(Denied::OutOfScope(_)) => Err(AppError::NotFound),
        }
    }

    /// The cap must allow sharing at all.
    fn sharing_enabled(&self) -> Result<SharingMode, AppError> {
        match self.cfg.sharing.mode() {
            SharingMode::Disabled => Err(AppError::SharingDisabled),
            mode => Ok(mode),
        }
    }

    /// Shares one of the person's threads at `level`, or changes the level of a share: a first
    /// share draws a nonce, a widening or a narrowing keeps it (the link stays the same). The
    /// same level as the thread has is the thread's current share and writes nothing. Returns what
    /// the owner is told, with the link. Which of these it is is decided on the thread as it is
    /// written, not as it was first read: a revocation in between makes this a first share with
    /// a new link, never the revoked one again.
    ///
    /// # Errors
    /// [`AppError::Forbidden`] without `thread.share`; [`AppError::NotFound`] for a thread that is
    /// not the person's; [`AppError::SharingDisabled`] under a `disabled` cap;
    /// [`AppError::OverCap`] for a level above the cap.
    pub async fn share_thread(
        &self,
        who: &impl Requester,
        id: ThreadId,
        level: ShareLevel,
    ) -> Result<ShareView, AppError> {
        let thread = self.owned_for_sharing(&self.access(who), id).await?;
        let mode = self.sharing_enabled()?;
        if !mode.allows(level) {
            return Err(AppError::OverCap { cap: mode });
        }
        if thread.share.is_some_and(|share| share.level == level) {
            return self.shared_now(&thread);
        }
        let mut action = None;
        let updated = self
            .apply_share(id, |now| {
                let (next, nonce) = match now.share {
                    Some(share) if share.level == level => (None, share.nonce),
                    Some(share) if share.level < level => (Some(ShareAction::Widen), share.nonce),
                    Some(share) => (Some(ShareAction::Narrow), share.nonce),
                    None => (Some(ShareAction::Share), self.new_nonce()),
                };
                action = next;
                Ok(next.map(|_| Input::Share {
                    user: who.user().clone(),
                    level,
                    nonce,
                }))
            })
            .await?;
        if let Some(action) = action {
            self.sharing_counters.changed(action);
        }
        self.shared_now(&updated)
    }

    /// A new link for a thread that is shared: a new nonce, so the old link is a 404 from now on,
    /// at the level it had. The thread must be shared when the new link is written: a revocation
    /// that lands first leaves it private.
    ///
    /// # Errors
    /// As [`share_thread`](Self::share_thread), and [`AppError::NotShared`] for a private thread.
    pub async fn rotate_share(
        &self,
        who: &impl Requester,
        id: ThreadId,
    ) -> Result<ShareView, AppError> {
        self.owned_for_sharing(&self.access(who), id).await?;
        self.sharing_enabled()?;
        let updated = self
            .apply_share(id, |now| {
                let share = now.share.ok_or(AppError::NotShared)?;
                Ok(Some(Input::Share {
                    user: who.user().clone(),
                    level: share.level,
                    nonce: self.new_nonce(),
                }))
            })
            .await?;
        self.sharing_counters.changed(ShareAction::Rotate);
        self.shared_now(&updated)
    }

    /// Takes the link of one of the person's threads down: the thread is private again and the
    /// nonce is gone, so the old link is a 404 and a later share makes a new one. **Needs only
    /// ownership**, and is never refused for the cap: whoever shared a thread can always stop.
    /// A thread that is not shared is left as it is, with no event.
    ///
    /// # Errors
    /// [`AppError::NotFound`] for a thread that is not the person's.
    pub async fn unshare_thread(&self, who: &impl Requester, id: ThreadId) -> Result<(), AppError> {
        let thread = self
            .ports
            .store()
            .get_thread(None, id)
            .await?
            .ok_or(AppError::NotFound)?;
        if &thread.owner != who.user() {
            return Err(AppError::NotFound);
        }
        let mut revoked = false;
        self.apply_share(id, |now| {
            revoked = now.share.is_some();
            Ok(revoked.then(|| Input::Unshare {
                user: who.user().clone(),
            }))
        })
        .await?;
        if revoked {
            self.sharing_counters.changed(ShareAction::Revoke);
        }
        Ok(())
    }

    /// Draws the nonce of a new link through the port, so the core stays pure and tests are
    /// deterministic.
    fn new_nonce(&self) -> ShareNonce {
        ShareNonce::new(self.ports.ids().new_token_bytes())
    }

    /// Applies what `decide` makes of the thread as each attempt of the commit loop reads it
    /// ([`App::apply_decided`]), so a share, a new link and a revocation that race are each
    /// decided on what the other left.
    async fn apply_share(
        &self,
        id: ThreadId,
        decide: impl FnMut(&ThreadRecord) -> Result<Option<Input>, AppError>,
    ) -> Result<ThreadRecord, AppError> {
        match self.apply_decided(id, decide).await? {
            ApplyOutcome::Applied { thread, .. } => Ok(thread),
            ApplyOutcome::Duplicate => Err(AppError::internal(
                "a share without an idempotency key was reported as a duplicate",
            )),
            ApplyOutcome::Fenced => Err(AppError::internal(
                "a commit without a lease was reported as fenced",
            )),
        }
    }

    fn shared_now(&self, thread: &ThreadRecord) -> Result<ShareView, AppError> {
        self.share_view(thread)
            .ok_or_else(|| AppError::internal("a thread that was just shared has no share"))
    }

    // ---- reading through a link -------------------------------------------------------------

    /// The thread a token names, how it is served now, and its share; [`AppError::NotFound`] for
    /// every way a token can fail to be a readable link: no secret to check it with (the cap is
    /// `disabled`), not 43 characters of the alphabet, no thread has the nonce, the MAC is not the
    /// thread's under the current or the previous secret, the thread is not shared, or the cap is
    /// below what it is shared at.
    async fn resolve_link(
        &self,
        token: &str,
    ) -> Result<(ThreadRecord, ShareNonce, Visibility), AppError> {
        let keys = self.cfg.sharing.keys().ok_or(AppError::NotFound)?;
        let opened = ShareKeys::open(token).ok_or(AppError::NotFound)?;
        let thread = self
            .ports
            .store()
            .thread_by_share_nonce(opened.nonce())
            .await?
            .ok_or(AppError::NotFound)?;
        if !keys.verify(thread.id, &opened) {
            return Err(AppError::NotFound);
        }
        let share = thread.share.ok_or(AppError::NotFound)?;
        let effective = self.cfg.sharing.mode().effective(share.level.visibility());
        if effective == Visibility::Private {
            return Err(AppError::NotFound);
        }
        Ok((thread, share.nonce, effective))
    }

    /// The signed-in side of a read: `thread.read` first (so what a person's roles lack is
    /// refused the same whatever the token), then the link.
    async fn resolve_signed_in(
        &self,
        who: &impl Requester,
        token: &str,
    ) -> Result<(SharedRead, Access<'_>), AppError> {
        let access = self.access(who);
        if !access.has(Permission::ThreadRead) {
            return Err(AppError::missing_permission(Permission::ThreadRead));
        }
        let (thread, nonce, effective) = self.resolve_link(token).await?;
        // The link is the grant: any role that reads, at any scope, reads a thread that is
        // effectively shared with signed-in people or wider.
        if !access.allows(
            Permission::ThreadRead,
            &Resource::SharedThread { effective },
        ) {
            return Err(AppError::NotFound);
        }
        let is_owner = &thread.owner == who.user();
        let read = SharedRead {
            thread,
            nonce,
            effective,
            rules: ReaderRules::internal(),
            is_owner,
        };
        Ok((read, access))
    }

    /// A thread opened by a signed-in person through its link (`internal`, or `public` when asked
    /// while signed in). Counts one read.
    ///
    /// # Errors
    /// [`AppError::Forbidden`] when the person's roles hold no `thread.read`;
    /// [`AppError::NotFound`] for everything else that can go wrong.
    pub async fn open_shared(
        &self,
        who: &impl Requester,
        token: &str,
    ) -> Result<SharedRead, AppError> {
        let (read, _) = self.resolve_signed_in(who, token).await?;
        self.sharing_counters.read(read.effective);
        Ok(read)
    }

    /// A thread opened by anybody through its link: only a thread served as `public`, and the
    /// public projection (no step input or output and no files, unless the deployment's
    /// `sharing.public` says so). No identity is asked for and none is read. Counts one read.
    ///
    /// # Errors
    /// [`AppError::NotFound`] for everything that can go wrong, an `internal` thread included.
    pub async fn open_public(&self, token: &str) -> Result<SharedRead, AppError> {
        let (thread, nonce, effective) = self.resolve_link(token).await?;
        if effective != Visibility::Public {
            return Err(AppError::NotFound);
        }
        let view = self.cfg.sharing.public();
        self.sharing_counters.read(effective);
        Ok(SharedRead {
            thread,
            nonce,
            effective,
            rules: ReaderRules::public(view.step_io, view.files),
            is_owner: false,
        })
    }

    /// The thread as the reader of `read` is given it: no owner, no parent, no tools.
    pub fn shared_view(&self, read: &SharedRead) -> SharedThreadView {
        reader_thread(&read.thread, read.effective, read.is_owner)
    }

    /// Whether the thread's own log names a file with this hash: a shared file must be one the
    /// thread handed over, never any key in the store.
    async fn log_names_file(&self, thread: ThreadId, sha256: &str) -> Result<bool, AppError> {
        let events = self
            .ports
            .store()
            .latest_events(thread, EventKind::Artifact, NAMED_FILES_WINDOW)
            .await?;
        Ok(events.iter().any(|e| {
            matches!(&e.body, EventBody::Artifact(a)
                if a.file.as_ref().is_some_and(|f| f.sha256 == sha256))
        }))
    }

    /// A file of a thread read through its link by a signed-in person: `artifact.read` as well as
    /// `thread.read`, and the file must be one the thread's log names.
    ///
    /// # Errors
    /// [`AppError::Forbidden`] for a role without `artifact.read`; [`AppError::NotFound`] for
    /// every other refusal, a file that is not there and a hash that is not a hash.
    pub async fn open_shared_artifact(
        &self,
        who: &impl Requester,
        token: &str,
        sha256: &str,
    ) -> Result<(ArtifactMeta, ByteStream), AppError> {
        let access = self.access(who);
        if !access.has(Permission::ArtifactRead) {
            return Err(AppError::missing_permission(Permission::ArtifactRead));
        }
        let (read, access) = self.resolve_signed_in(who, token).await?;
        if !access.allows(
            Permission::ArtifactRead,
            &Resource::SharedThread {
                effective: read.effective,
            },
        ) {
            return Err(AppError::NotFound);
        }
        if !self.log_names_file(read.thread.id, sha256).await? {
            return Err(AppError::NotFound);
        }
        self.fetch_artifact(read.thread.id, sha256).await
    }

    /// A file of a thread read by anybody through its link: only when the deployment lets a public
    /// reader have files (`sharing.public.files`), and the file must be one the thread's log
    /// names.
    ///
    /// # Errors
    /// [`AppError::NotFound`] for every refusal.
    pub async fn open_public_artifact(
        &self,
        token: &str,
        sha256: &str,
    ) -> Result<(ArtifactMeta, ByteStream), AppError> {
        if !self.cfg.sharing.public().files {
            return Err(AppError::NotFound);
        }
        let (thread, _, effective) = self.resolve_link(token).await?;
        if effective != Visibility::Public || !self.log_names_file(thread.id, sha256).await? {
            return Err(AppError::NotFound);
        }
        self.fetch_artifact(thread.id, sha256).await
    }

    /// The thread's log and the live text of its replies, as the reader of `read` may see them
    /// (ADR 0040, section 8): [`feed_of`](Self::feed_of) over the reader projection, replayed from
    /// the start (`after` is the cursor the client holds) and then followed.
    ///
    /// **The stream ends** when the share changes under it: the link is taken down, replaced by a
    /// new one, or narrowed below what the stream was opened for (`public` for a public reader,
    /// `internal` for a signed-in one); the client's reconnect then gets the 404. The share is
    /// read again when a `thread_shared` or `thread_unshared` event passes, so a revocation ends a
    /// stream as soon as it is committed, and at least every
    /// [`SharingSettings::recheck`] (30 seconds) whatever else happens. A store that cannot answer
    /// ends it too: the client reconnects, and the answer is a 404 if the link is gone.
    pub fn shared_feed(
        self: &Arc<Self>,
        read: &SharedRead,
        after: i64,
    ) -> BoxStream<'static, FeedItem> {
        /// What a stream needs to know whether its link still serves it.
        struct Guard<P: Ports> {
            app: Arc<App<P>>,
            thread: ThreadId,
            nonce: ShareNonce,
            needs: Visibility,
        }
        impl<P: Ports> Guard<P> {
            /// Whether the link this stream was opened for still serves what it needs.
            async fn still_open(&self) -> bool {
                match self.app.ports.store().get_thread(None, self.thread).await {
                    Ok(Some(thread)) => thread.share.is_some_and(|share| {
                        share.nonce == self.nonce
                            && self
                                .app
                                .cfg
                                .sharing
                                .mode()
                                .effective(share.level.visibility())
                                >= self.needs
                    }),
                    Ok(None) => false,
                    Err(e) => {
                        tracing::warn!(error = %orch_core::report(&e), "a shared stream could not re-read its thread; ending it");
                        false
                    }
                }
            }
        }
        struct St<P: Ports> {
            guard: Guard<P>,
            inner: BoxStream<'static, FeedItem>,
            rules: ReaderRules,
            due: Instant,
        }
        let needs = match read.rules.audience {
            ReaderAudience::Public => Visibility::Public,
            ReaderAudience::Internal => Visibility::Internal,
        };
        let st = St {
            guard: Guard {
                app: Arc::clone(self),
                thread: read.thread.id,
                nonce: read.nonce,
                needs,
            },
            inner: self.feed_of(&read.thread, after),
            rules: read.rules,
            due: Instant::now() + self.cfg.sharing.recheck(),
        };
        futures::stream::unfold(st, |mut st| async move {
            loop {
                tokio::select! {
                    item = st.inner.next() => {
                        match item? {
                            FeedItem::Event(event) => {
                                let sharing = matches!(
                                    event.body,
                                    EventBody::ThreadShared(_) | EventBody::ThreadUnshared(_)
                                );
                                if sharing && !st.guard.still_open().await {
                                    return None;
                                }
                                let said = reader_event(&event, &st.rules);
                                return Some((FeedItem::Event(said), st));
                            }
                            // The words of a reply are the agent's, not the owner's.
                            piece @ FeedItem::Live(_) => return Some((piece, st)),
                        }
                    }
                    () = tokio::time::sleep_until(st.due) => {
                        if !st.guard.still_open().await {
                            return None;
                        }
                        st.due = Instant::now() + st.guard.app.cfg.sharing.recheck();
                    }
                }
            }
        })
        .boxed()
    }
}
