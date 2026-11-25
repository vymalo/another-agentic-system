//! The `Window` port (ADR 0092, `docs/api/windows-v1.md`): a view onto something a run is
//! producing, that a person can watch and an agent can drive.
//!
//! | Call | Does |
//! |---|---|
//! | [`open`](Window::open) | Opens a window on a URL; answers its id and the first frame. |
//! | [`look`](Window::look) | The current frame, or `None` for a window that is closed or expired. |
//! | [`act`](Window::act) | Applies up to [`MAX_ACTIONS_PER_CALL`] actions; answers the frame after them. |
//! | [`close`](Window::close) | Closes; idempotent; `false` for an id the thread has not got. |
//!
//! Every adapter owes [`check_url`] on the **resolved** addresses before it connects. The
//! testkit (feature `testkit`) holds each adapter to it.

mod guard;
#[cfg(feature = "testkit")]
pub mod testkit;

pub use guard::check_url;
use orch_core::ThreadId;
use serde::{Deserialize, Serialize};
use std::future::Future;

/// One frame is a picture, not a film: a larger frame is refused.
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;
/// A person watches a page or two, not a desktop.
pub const MAX_WINDOWS_PER_THREAD: usize = 4;
/// One `act` is a short script.
pub const MAX_ACTIONS_PER_CALL: usize = 8;
/// A window left alone closes itself after this many seconds.
pub const WINDOW_TTL_SECS: u64 = 900;
/// The URL an `open` may name, in bytes.
pub const MAX_URL_BYTES: usize = 2048;

/// A window's id, scoped to a thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowId(pub String);

/// An action, in a closed set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// A click at a point of the frame.
    Click {
        /// Horizontal position.
        x: u32,
        /// Vertical position.
        y: u32,
    },
    /// Typed text.
    Type {
        /// The text.
        text: String,
    },
    /// A scroll by `dy` pixels (negative is up).
    Scroll {
        /// Vertical distance.
        dy: i32,
    },
    /// A named key.
    Key {
        /// The key's name.
        name: String,
    },
}

/// A frame: a picture handed over by reference, never inline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// The picture's media type.
    pub media_type: String,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Device scale.
    pub scale: u32,
    /// An artifact reference (`artifact:<id>`); the bytes are in the artifact store.
    pub url: String,
    /// When it was taken, RFC 3339.
    pub at: String,
}

/// What `open` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    /// The new window.
    pub id: WindowId,
    /// Its first frame.
    pub frame: Frame,
}

/// Why a call is refused. Each is a code of `windows-v1`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WindowError {
    /// The deployment serves no windows.
    #[error("windows_off")]
    WindowsOff,
    /// The caller is not the thread's main caller.
    #[error("not_main_caller")]
    NotMainCaller,
    /// The thread has no such window.
    #[error("window_unknown")]
    WindowUnknown,
    /// The URL is private, loopback, link-local, not http(s), too long or unresolvable.
    #[error("url_refused")]
    UrlRefused,
    /// The thread already has [`MAX_WINDOWS_PER_THREAD`] windows.
    #[error("too_many_windows")]
    TooManyWindows,
    /// Actions over the bound, or a frame over [`MAX_FRAME_BYTES`].
    #[error("invalid")]
    Invalid,
}

/// The port. An adapter resolves the URL's host itself and applies [`check_url`] to what it
/// resolved, before it connects.
pub trait Window: Send + Sync + 'static {
    /// Opens a window for `thread` on `url`.
    fn open(
        &self,
        thread: ThreadId,
        url: &str,
    ) -> impl Future<Output = Result<Opened, WindowError>> + Send;

    /// The current frame; `None` for a window that is closed or has expired.
    fn look(
        &self,
        thread: ThreadId,
        id: &WindowId,
    ) -> impl Future<Output = Result<Option<Frame>, WindowError>> + Send;

    /// Applies `actions` (at most [`MAX_ACTIONS_PER_CALL`]) and answers the frame after them.
    fn act(
        &self,
        thread: ThreadId,
        id: &WindowId,
        actions: &[Action],
    ) -> impl Future<Output = Result<Frame, WindowError>> + Send;

    /// Closes; idempotent. `false` when the thread had no such live window.
    fn close(
        &self,
        thread: ThreadId,
        id: &WindowId,
    ) -> impl Future<Output = Result<bool, WindowError>> + Send;
}
