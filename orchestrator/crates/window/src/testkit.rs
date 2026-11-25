//! The conformance testkit of the [`Window`] port, and [`FakeWindow`], an adapter with no network.
//!
//! `window_conformance!(make, expire = path)`: `make` is an `fn() -> W` giving a fresh adapter and
//! `expire` a `fn(&W, ThreadId, &WindowId)` that makes a window past its TTL. The calling
//! crate needs `tokio` (with `macros` and `rt-multi-thread`) as a dev-dependency.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::*;
use orch_core::ThreadId;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

/// An adapter with no network: frames are made up, names resolve from a table.
#[derive(Default)]
pub struct FakeWindow {
    state: Mutex<FakeState>,
    hosts: HashMap<String, Vec<IpAddr>>,
    unguarded: bool,
}

#[derive(Default)]
struct FakeState {
    next: u64,
    live: HashMap<(ThreadId, WindowId), u64>,
}

impl FakeWindow {
    /// A fake that guards every URL, as an adapter must.
    pub fn new() -> Self {
        Self::default()
    }

    /// Names `host` as resolving to `addrs` (a name not named here resolves to nothing).
    pub fn with_host(mut self, host: &str, addrs: Vec<IpAddr>) -> Self {
        self.hosts.insert(host.to_ascii_lowercase(), addrs);
        self
    }

    /// A fake that skips the guard. **Only to see the guard's cases go red once**: no deployment
    /// builds this.
    pub fn unguarded() -> Self {
        Self {
            unguarded: true,
            ..Self::default()
        }
    }

    /// Makes the window past its TTL.
    pub fn expire(&self, thread: ThreadId, id: &WindowId) {
        self.lock().live.remove(&(thread, id.clone()));
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn frame(n: u64) -> Frame {
        Frame {
            media_type: "image/png".into(),
            width: 1280,
            height: 800,
            scale: 1,
            url: format!("artifact:fake-{n}"),
            at: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn resolve(&self, url: &str) -> Vec<IpAddr> {
        url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            .and_then(|h| self.hosts.get(&h).cloned())
            .unwrap_or_default()
    }
}

impl Window for FakeWindow {
    async fn open(&self, thread: ThreadId, url: &str) -> Result<Opened, WindowError> {
        if !self.unguarded {
            check_url(url, &self.resolve(url))?;
        }
        let mut st = self.lock();
        if st.live.keys().filter(|(t, _)| *t == thread).count() >= MAX_WINDOWS_PER_THREAD {
            return Err(WindowError::TooManyWindows);
        }
        st.next += 1;
        let n = st.next;
        let id = WindowId(format!("w-{n}"));
        st.live.insert((thread, id.clone()), n);
        Ok(Opened {
            id,
            frame: Self::frame(n),
        })
    }

    async fn look(&self, thread: ThreadId, id: &WindowId) -> Result<Option<Frame>, WindowError> {
        Ok(self
            .lock()
            .live
            .get(&(thread, id.clone()))
            .map(|n| Self::frame(*n)))
    }

    async fn act(
        &self,
        thread: ThreadId,
        id: &WindowId,
        actions: &[Action],
    ) -> Result<Frame, WindowError> {
        let mut st = self.lock();
        if !st.live.contains_key(&(thread, id.clone())) {
            return Err(WindowError::WindowUnknown);
        }
        if actions.len() > MAX_ACTIONS_PER_CALL {
            return Err(WindowError::Invalid);
        }
        st.next += 1;
        let m = st.next;
        st.live.insert((thread, id.clone()), m);
        Ok(Self::frame(m))
    }

    async fn close(&self, thread: ThreadId, id: &WindowId) -> Result<bool, WindowError> {
        Ok(self.lock().live.remove(&(thread, id.clone())).is_some())
    }
}

fn thread() -> ThreadId {
    ThreadId(uuid::Uuid::now_v7())
}

/// The cases. Each takes a fresh adapter.
pub mod cases {
    use super::*;

    /// Every address class the guard refuses, by the public `open` path.
    pub const REFUSED_URLS: &[&str] = &[
        "http://127.0.0.1/",
        "http://127.255.255.254:8080/",
        "http://10.1.2.3/",
        "http://172.16.0.1/",
        "http://172.31.255.255/",
        "http://192.168.1.1/",
        "http://169.254.169.254/latest/meta-data/",
        "http://169.254.1.1/",
        "http://100.64.0.1/",
        "http://0.0.0.0/",
        "http://2130706433/",
        "http://0x7f.1/",
        "http://[::1]/",
        "http://[fc00::1]/",
        "http://[fd12:3456::1]/",
        "http://[fe80::1]/",
        "http://[::ffff:127.0.0.1]/",
        "http://[::ffff:10.0.0.1]/",
        "http://[::ffff:169.254.169.254]/",
        "ftp://93.184.216.34/",
        "file:///etc/passwd",
        "not a url",
        "http://unresolvable.invalid/",
    ];

    /// Each refused class is `UrlRefused`, and a public literal opens.
    pub async fn private_loopback_link_local_and_metadata_addresses_are_refused<W: Window>(w: W) {
        for url in REFUSED_URLS {
            assert_eq!(
                w.open(thread(), url).await.err(),
                Some(WindowError::UrlRefused),
                "{url} must be refused"
            );
        }
        assert!(w.open(thread(), "https://93.184.216.34/").await.is_ok());
        assert!(w.open(thread(), "http://[2606:4700::1111]/").await.is_ok());
    }

    /// A URL over `MAX_URL_BYTES` is refused, one at the bound is not judged by length.
    pub async fn an_over_long_url_is_refused<W: Window>(w: W) {
        let long = format!("http://93.184.216.34/{}", "a".repeat(MAX_URL_BYTES));
        assert_eq!(
            w.open(thread(), &long).await.err(),
            Some(WindowError::UrlRefused)
        );
    }

    /// Windows over the cap are refused; another thread is unaffected; a close frees a place.
    pub async fn windows_over_the_cap_are_refused<W: Window>(w: W) {
        let t = thread();
        let mut ids = Vec::new();
        for _ in 0..MAX_WINDOWS_PER_THREAD {
            ids.push(w.open(t, "http://93.184.216.34/").await.unwrap().id);
        }
        assert_eq!(
            w.open(t, "http://93.184.216.34/").await.err(),
            Some(WindowError::TooManyWindows)
        );
        assert!(w.open(thread(), "http://93.184.216.34/").await.is_ok());
        assert!(w.close(t, &ids[0]).await.unwrap());
        assert!(w.open(t, "http://93.184.216.34/").await.is_ok());
    }

    /// Concurrent opens of one thread end with exactly the cap, never more.
    pub async fn concurrent_opens_never_pass_the_cap<W: Window>(w: W) {
        let w = std::sync::Arc::new(w);
        let t = thread();
        let tasks: Vec<_> = (0..MAX_WINDOWS_PER_THREAD * 3)
            .map(|_| {
                let w = w.clone();
                tokio::spawn(async move { w.open(t, "http://93.184.216.34/").await })
            })
            .collect();
        let mut ok = 0;
        let mut ids = std::collections::HashSet::new();
        for task in tasks {
            if let Ok(o) = task.await.unwrap() {
                ok += 1;
                assert!(ids.insert(o.id), "ids are distinct");
            }
        }
        assert_eq!(ok, MAX_WINDOWS_PER_THREAD);
    }

    /// `close` answers `true` once and `false` after, never an error.
    pub async fn close_is_idempotent<W: Window>(w: W) {
        let t = thread();
        let id = w.open(t, "http://93.184.216.34/").await.unwrap().id;
        assert!(w.close(t, &id).await.unwrap());
        assert!(!w.close(t, &id).await.unwrap());
        assert!(!w.close(t, &WindowId("w-never".into())).await.unwrap());
    }

    /// A window of one thread is unknown to another.
    pub async fn a_window_belongs_to_its_thread<W: Window>(w: W) {
        let id = w.open(thread(), "http://93.184.216.34/").await.unwrap().id;
        let other = thread();
        assert_eq!(w.look(other, &id).await.unwrap(), None);
        assert_eq!(
            w.act(other, &id, &[]).await.err(),
            Some(WindowError::WindowUnknown)
        );
        assert!(!w.close(other, &id).await.unwrap());
    }

    /// `look` of an expired window answers none; `act` on it is `WindowUnknown`.
    pub async fn look_of_an_expired_window_is_closed<W: Window>(
        w: W,
        expire: fn(&W, ThreadId, &WindowId),
    ) {
        let t = thread();
        let id = w.open(t, "http://93.184.216.34/").await.unwrap().id;
        assert!(w.look(t, &id).await.unwrap().is_some());
        expire(&w, t, &id);
        assert_eq!(w.look(t, &id).await.unwrap(), None);
        assert_eq!(
            w.act(t, &id, &[]).await.err(),
            Some(WindowError::WindowUnknown)
        );
    }

    /// Up to the bound is applied; over it is `Invalid`, never a truncation.
    pub async fn actions_over_the_bound_are_invalid<W: Window>(w: W) {
        let t = thread();
        let id = w.open(t, "http://93.184.216.34/").await.unwrap().id;
        let click = Action::Click { x: 1, y: 2 };
        let ok = vec![click.clone(); MAX_ACTIONS_PER_CALL];
        assert!(w.act(t, &id, &ok).await.is_ok());
        let over = vec![click; MAX_ACTIONS_PER_CALL + 1];
        assert_eq!(w.act(t, &id, &over).await.err(), Some(WindowError::Invalid));
        assert!(w.look(t, &id).await.unwrap().is_some());
    }

    /// A frame is a reference, never bytes, and says its media type.
    pub async fn a_frame_is_handed_over_by_reference<W: Window>(w: W) {
        let f = w
            .open(thread(), "http://93.184.216.34/")
            .await
            .unwrap()
            .frame;
        assert!(f.url.starts_with("artifact:"));
        assert!(f.url.len() < 256);
        assert!(f.media_type.starts_with("image/"));
        assert!(f.width > 0 && f.height > 0 && f.scale > 0);
    }
}

/// Generates one `#[tokio::test]` per conformance case. See the module doc.
#[macro_export]
macro_rules! window_conformance {
    ($make:path, expire = $expire:path) => {
        $crate::window_conformance!(@cases $make;
            private_loopback_link_local_and_metadata_addresses_are_refused
            an_over_long_url_is_refused windows_over_the_cap_are_refused
            concurrent_opens_never_pass_the_cap close_is_idempotent
            a_window_belongs_to_its_thread actions_over_the_bound_are_invalid
            a_frame_is_handed_over_by_reference
        );
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn look_of_an_expired_window_is_closed() {
            $crate::testkit::cases::look_of_an_expired_window_is_closed($make(), $expire).await
        }
    };
    (@cases $make:path; $($case:ident)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                $crate::testkit::cases::$case($make()).await
            }
        )*
    };
}
