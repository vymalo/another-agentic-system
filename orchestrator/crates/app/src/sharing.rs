//! What a deployment says about sharing (ADR 0040, section 4) and what the application tells the
//! owner of a shared thread.
//!
//! [`SharingSettings`] is `sharing` of the configuration: the cap ([`SharingMode`]), the secrets the
//! link's MAC is made under ([`ShareKeys`]), and what a public reader may see beyond the default
//! ([`PublicView`]). It cannot be built wrong: a cap above `disabled` has keys.
//!
//! Nothing here reads the store or the clock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use orch_core::{ShareLevel, Timestamp, Visibility};

use crate::share_link::ShareKeys;

/// The cap `sharing.mode` puts on every thread's visibility: what the deployment allows. Ordered
/// like [`Visibility`]: `disabled < internal < public`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SharingMode {
    /// No thread is served through a link, and none can be shared; a link can still be taken down.
    #[default]
    Disabled,
    /// Signed-in people with the link.
    Internal,
    /// Anybody with the link.
    Public,
}

impl SharingMode {
    /// The name used in the configuration and by `GET /api/me`.
    pub const fn as_str(self) -> &'static str {
        match self {
            SharingMode::Disabled => "disabled",
            SharingMode::Internal => "internal",
            SharingMode::Public => "public",
        }
    }

    /// The mode named `name`, exactly.
    pub fn parse(name: &str) -> Option<SharingMode> {
        [
            SharingMode::Disabled,
            SharingMode::Internal,
            SharingMode::Public,
        ]
        .into_iter()
        .find(|m| m.as_str() == name)
    }

    /// The widest visibility the mode allows, `disabled` counted as private.
    pub const fn cap(self) -> Visibility {
        match self {
            SharingMode::Disabled => Visibility::Private,
            SharingMode::Internal => Visibility::Internal,
            SharingMode::Public => Visibility::Public,
        }
    }

    /// What is served for a thread stored at `stored`: `min(stored, cap)` (ADR 0040, section 1).
    pub fn effective(self, stored: Visibility) -> Visibility {
        stored.capped_by(self.cap())
    }

    /// Whether a thread may be shared at `level` under this cap.
    pub fn allows(self, level: ShareLevel) -> bool {
        level.visibility() <= self.cap()
    }
}

impl std::fmt::Display for SharingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a public reader may see beyond the default (`sharing.public`): the default is neither.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PublicView {
    /// A public reader sees steps' input and output (`sharing.public.stepIo`).
    pub step_io: bool,
    /// A public reader can open the thread's files (`sharing.public.files`).
    pub files: bool,
}

/// Why sharing settings cannot be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SharingError {
    /// A cap above `disabled` with no secret to make links with.
    #[error("sharing is {mode} but there is no secret to make links with")]
    NoSecret {
        /// The cap.
        mode: &'static str,
    },
    /// `public` settings with a cap that is not `public`: they would do nothing.
    #[error("sharing.public settings do nothing unless the mode is public")]
    PublicViewWithoutPublic,
}

/// How often a stream of a shared thread re-reads the thread's share when nothing wakes it
/// (ADR 0040, section 8): the planner's number, *unverified* under load.
pub const DEFAULT_RECHECK: Duration = Duration::from_secs(30);

/// `sharing` of the configuration (ADR 0040, section 4), as the application uses it.
#[derive(Debug, Clone)]
pub struct SharingSettings {
    mode: SharingMode,
    keys: Option<ShareKeys>,
    public: PublicView,
    base_url: Option<String>,
    recheck: Duration,
}

impl Default for SharingSettings {
    /// Disabled: a process that configures nothing shares nothing.
    fn default() -> Self {
        SharingSettings {
            mode: SharingMode::Disabled,
            keys: None,
            public: PublicView::default(),
            base_url: None,
            recheck: DEFAULT_RECHECK,
        }
    }
}

impl SharingSettings {
    /// Settings of `mode`, with the keys links are made under and what a public reader sees.
    ///
    /// # Errors
    /// [`SharingError`]: a cap above `disabled` needs keys, and `public` settings need a `public`
    /// cap.
    pub fn new(
        mode: SharingMode,
        keys: Option<ShareKeys>,
        public: PublicView,
    ) -> Result<Self, SharingError> {
        if mode != SharingMode::Disabled && keys.is_none() {
            return Err(SharingError::NoSecret {
                mode: mode.as_str(),
            });
        }
        if mode != SharingMode::Public && public != PublicView::default() {
            return Err(SharingError::PublicViewWithoutPublic);
        }
        Ok(SharingSettings {
            mode,
            // A disabled deployment makes and checks no link, whatever secret it still holds.
            keys: keys.filter(|_| mode != SharingMode::Disabled),
            public,
            base_url: None,
            recheck: DEFAULT_RECHECK,
        })
    }

    /// The address links are shown under (`https://host`, no trailing slash): without one a link is
    /// the path `/s/<token>`.
    #[must_use]
    pub fn with_base_url(mut self, base_url: Option<String>) -> Self {
        self.base_url = base_url.map(|u| u.trim_end_matches('/').to_owned());
        self
    }

    /// How often a stream re-reads the share when nothing wakes it. For tests.
    #[must_use]
    pub fn with_recheck(mut self, recheck: Duration) -> Self {
        self.recheck = recheck;
        self
    }

    /// The cap.
    pub const fn mode(&self) -> SharingMode {
        self.mode
    }

    /// What a public reader may see.
    pub const fn public(&self) -> PublicView {
        self.public
    }

    /// The keys, when the cap is above `disabled`.
    pub fn keys(&self) -> Option<&ShareKeys> {
        self.keys.as_ref()
    }

    /// How often a stream re-reads the share.
    pub const fn recheck(&self) -> Duration {
        self.recheck
    }

    /// The link of a token: `<base>/s/<token>`, or the path alone.
    pub fn url_of(&self, token: &str) -> String {
        match &self.base_url {
            Some(base) => format!("{base}/s/{token}"),
            None => format!("/s/{token}"),
        }
    }
}

/// What the owner is told of a thread's share (`share` of `GET /api/threads/{id}`, the answer of
/// `PUT …/share`). Only the owner is ever given the `url`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareView {
    /// What the thread is stored as.
    pub visibility: ShareLevel,
    /// What is served now: `min(visibility, cap)`. `private` when the deployment's cap is below
    /// what the thread is shared at (the link is paused, not gone).
    pub effective: Visibility,
    /// When the share was last set.
    pub shared_at: Timestamp,
    /// The link, recomputed from the thread's row under the current secret. Absent when the
    /// deployment has no secret (the cap is `disabled`): there is nothing to make it with.
    pub url: Option<String>,
}

/// How a share changed, for the audit counters (`share_changes_total{action}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareAction {
    /// A private thread was shared.
    Share,
    /// A thread shared with signed-in people was shared with anybody.
    Widen,
    /// A public thread was shared with signed-in people only.
    Narrow,
    /// A new link was made.
    Rotate,
    /// The link was taken down.
    Revoke,
}

impl ShareAction {
    /// The label of the counter.
    pub const fn as_str(self) -> &'static str {
        match self {
            ShareAction::Share => "share",
            ShareAction::Widen => "widen",
            ShareAction::Narrow => "narrow",
            ShareAction::Rotate => "rotate",
            ShareAction::Revoke => "revoke",
        }
    }

    /// Every action, in the order the counters are written.
    pub const ALL: [ShareAction; 5] = [
        ShareAction::Share,
        ShareAction::Widen,
        ShareAction::Narrow,
        ShareAction::Rotate,
        ShareAction::Revoke,
    ];

    const fn index(self) -> usize {
        match self {
            ShareAction::Share => 0,
            ShareAction::Widen => 1,
            ShareAction::Narrow => 2,
            ShareAction::Rotate => 3,
            ShareAction::Revoke => 4,
        }
    }
}

/// What a person who reads a shared thread is, for the counter `shared_reads_total{visibility}`:
/// the effective visibility the read was served at.
#[derive(Debug, Default)]
pub(crate) struct SharingCounters {
    changes: [AtomicU64; 5],
    reads_internal: AtomicU64,
    reads_public: AtomicU64,
}

impl SharingCounters {
    pub(crate) fn changed(&self, action: ShareAction) {
        self.changes[action.index()].fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn read(&self, effective: Visibility) {
        match effective {
            Visibility::Internal => self.reads_internal.fetch_add(1, Ordering::Relaxed),
            Visibility::Public => self.reads_public.fetch_add(1, Ordering::Relaxed),
            // A read is never served at `private`: there is nothing to count.
            Visibility::Private => 0,
        };
    }

    pub(crate) fn snapshot(&self) -> SharingStats {
        SharingStats {
            changes: ShareAction::ALL.map(|a| (a, self.changes[a.index()].load(Ordering::Relaxed))),
            reads_internal: self.reads_internal.load(Ordering::Relaxed),
            reads_public: self.reads_public.load(Ordering::Relaxed),
        }
    }
}

/// The counters of sharing in this process (ADR 0040, section 11): how many times a share changed,
/// by action, and how many times a shared thread was opened, by the visibility it was served at.
/// Never per person and never per address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SharingStats {
    /// `share_changes_total`, by action.
    pub changes: [(ShareAction, u64); 5],
    /// `shared_reads_total{visibility="internal"}`.
    pub reads_internal: u64,
    /// `shared_reads_total{visibility="public"}`.
    pub reads_public: u64,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use secrecy::SecretString;

    use super::*;

    fn keys() -> ShareKeys {
        ShareKeys::new(SecretString::from("x".repeat(32)), None).unwrap()
    }

    #[test]
    fn the_cap_counts_disabled_as_private_and_takes_the_narrower() {
        use SharingMode::{Disabled, Internal, Public};
        use Visibility as V;
        for (mode, stored, effective) in [
            (Disabled, V::Public, V::Private),
            (Disabled, V::Internal, V::Private),
            (Disabled, V::Private, V::Private),
            (Internal, V::Public, V::Internal),
            (Internal, V::Internal, V::Internal),
            (Internal, V::Private, V::Private),
            (Public, V::Public, V::Public),
            (Public, V::Internal, V::Internal),
            (Public, V::Private, V::Private),
        ] {
            assert_eq!(mode.effective(stored), effective, "{mode:?} caps {stored}");
        }
        assert!(Public.allows(ShareLevel::Public) && Public.allows(ShareLevel::Internal));
        assert!(Internal.allows(ShareLevel::Internal) && !Internal.allows(ShareLevel::Public));
        assert!(!Disabled.allows(ShareLevel::Internal) && !Disabled.allows(ShareLevel::Public));
        assert!(Disabled < Internal && Internal < Public);
        for m in [Disabled, Internal, Public] {
            assert_eq!(SharingMode::parse(m.as_str()), Some(m));
        }
        assert_eq!(SharingMode::parse("Public"), None);
        assert_eq!(SharingMode::default(), Disabled);
    }

    #[test]
    fn settings_cannot_be_built_wrong() {
        assert!(SharingSettings::new(SharingMode::Disabled, None, PublicView::default()).is_ok());
        for mode in [SharingMode::Internal, SharingMode::Public] {
            assert_eq!(
                SharingSettings::new(mode, None, PublicView::default()).unwrap_err(),
                SharingError::NoSecret {
                    mode: mode.as_str()
                }
            );
        }
        let view = PublicView {
            step_io: true,
            files: false,
        };
        for mode in [SharingMode::Disabled, SharingMode::Internal] {
            assert_eq!(
                SharingSettings::new(mode, Some(keys()), view).unwrap_err(),
                SharingError::PublicViewWithoutPublic
            );
        }
        let ok = SharingSettings::new(SharingMode::Public, Some(keys()), view).unwrap();
        assert_eq!(ok.public(), view);
        assert!(ok.keys().is_some());
        // a disabled deployment keeps no keys: it makes no link and checks none
        let off = SharingSettings::new(SharingMode::Disabled, Some(keys()), PublicView::default())
            .unwrap();
        assert!(off.keys().is_none());
        assert_eq!(SharingSettings::default().mode(), SharingMode::Disabled);
    }

    #[test]
    fn a_link_is_under_the_base_address_or_a_path() {
        let s = SharingSettings::default();
        assert_eq!(s.url_of("T"), "/s/T");
        let s = s.with_base_url(Some("https://chat.example.com/".into()));
        assert_eq!(s.url_of("T"), "https://chat.example.com/s/T");
    }
}
