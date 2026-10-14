//! Where a viewer fetches a thread's kept files from: the `href` of a `vymalo.artifact`
//! (`docs/api/agui.md`, "Typed artifacts").
//!
//! A kept file is served by one route per way of reading the thread (ADR 0032, ADR 0040): the
//! owner's `/api/threads/{id}/artifacts/{sha256}`, and a share link's own,
//! `/api/shared/{token}/artifacts/{sha256}` for a signed-in reader and
//! `/api/public/shared/{token}/artifacts/{sha256}` for anybody. A reader of a link has the link
//! and not the thread, so the owner's route is a 404 for them: the projection names the one they
//! can use. The route changes the `href` and nothing else of a frame.

use std::fmt;

use orch_core::{FileRef, ThreadId};

/// The route a viewer's files are served by.
///
/// `Debug` never shows the token of a link: a route is held by the `Projector` and the `Connect`,
/// which are printed, and the token is a capability to read the thread (`ShareKeys` and
/// `ShareNonce` keep the same rule). A route of a link is built by [`FileRoute::shared`] and
/// [`FileRoute::public`] and by nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FileRoute {
    /// The owner's: `/api/threads/{thread}/artifacts/{sha256}`.
    #[default]
    Owner,
    /// A share link's, for a **signed-in** reader: `/api/shared/{token}/artifacts/{sha256}`.
    Shared(LinkToken),
    /// A share link's, for **anybody**: `/api/public/shared/{token}/artifacts/{sha256}`.
    Public(LinkToken),
}

/// The token of a share link, as a route holds it: the capability the reader holds already, as it
/// is in the address of the page and of the stream. It cannot be built outside this crate, cannot
/// be read back and is not printed by `Debug`; it is only put in an `href`.
#[derive(Clone, PartialEq, Eq)]
pub struct LinkToken(String);

impl fmt::Debug for LinkToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("..")
    }
}

impl FileRoute {
    /// The files of a thread read through its link by a **signed-in** reader
    /// (`GET /api/shared/{token}/artifacts/{sha256}`).
    pub fn shared(token: &str) -> Self {
        FileRoute::Shared(LinkToken(token.to_owned()))
    }

    /// The files of a thread read through its link by **anybody**
    /// (`GET /api/public/shared/{token}/artifacts/{sha256}`).
    pub fn public(token: &str) -> Self {
        FileRoute::Public(LinkToken(token.to_owned()))
    }

    /// Where `file` of `thread` is served for this viewer, relative to the API's origin.
    pub fn href(&self, thread: ThreadId, file: &FileRef) -> String {
        match self {
            FileRoute::Owner => file.href(thread),
            FileRoute::Shared(LinkToken(token)) => {
                format!("/api/shared/{token}/artifacts/{}", file.sha256)
            }
            FileRoute::Public(LinkToken(token)) => {
                format!("/api/public/shared/{token}/artifacts/{}", file.sha256)
            }
        }
    }
}
