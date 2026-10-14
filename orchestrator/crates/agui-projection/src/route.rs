//! Where a viewer fetches a thread's kept files from: the `href` of a `vymalo.artifact`
//! (`docs/api/agui.md`, "Typed artifacts").
//!
//! A kept file is served by one route per way of reading the thread (ADR 0032, ADR 0040): the
//! owner's `/api/threads/{id}/artifacts/{sha256}`, and a share link's own,
//! `/api/shared/{token}/artifacts/{sha256}` for a signed-in reader and
//! `/api/public/shared/{token}/artifacts/{sha256}` for anybody. A reader of a link has the link
//! and not the thread, so the owner's route is a 404 for them: the projection names the one they
//! can use. The route changes the `href` and nothing else of a frame.

use orch_core::{FileRef, ThreadId};

/// The route a viewer's files are served by.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FileRoute {
    /// The owner's: `/api/threads/{thread}/artifacts/{sha256}`.
    #[default]
    Owner,
    /// A share link's: `{base}/artifacts/{sha256}`, the `base` of which carries the link's token
    /// (a capability the reader holds already, as it is in the address of the page and of the
    /// stream).
    Link {
        /// `/api/shared/{token}` or `/api/public/shared/{token}`.
        base: String,
    },
}

impl FileRoute {
    /// The files of a thread read through its link by a **signed-in** reader
    /// (`GET /api/shared/{token}/artifacts/{sha256}`).
    pub fn shared(token: &str) -> Self {
        FileRoute::Link {
            base: format!("/api/shared/{token}"),
        }
    }

    /// The files of a thread read through its link by **anybody**
    /// (`GET /api/public/shared/{token}/artifacts/{sha256}`).
    pub fn public(token: &str) -> Self {
        FileRoute::Link {
            base: format!("/api/public/shared/{token}"),
        }
    }

    /// Where `file` of `thread` is served for this viewer, relative to the API's origin.
    pub fn href(&self, thread: ThreadId, file: &FileRef) -> String {
        match self {
            FileRoute::Owner => file.href(thread),
            FileRoute::Link { base } => format!("{base}/artifacts/{}", file.sha256),
        }
    }
}
