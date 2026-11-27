use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use zeroize::Zeroizing;

use crate::memory::FixedClock;
use crate::{
    Clock, PlatformCertificateFuture, PlatformClientCertificate, PlatformClientCertificates,
    PlatformCredentialsError,
};

/// How far [`MemoryPlatformClientCertificates::set_failing`] moves the clock: past the longest
/// pause an adapter keeps after a failure (a minute).
pub const FAILURE_PAUSE_PASSED: Duration = Duration::from_secs(61);

#[derive(Default)]
struct State {
    held: Option<PlatformClientCertificate>,
    failing: Option<PlatformCredentialsError>,
    issued: u64,
}

/// An in-memory [`PlatformClientCertificates`]: it issues a made-up certificate (no real key, no
/// real X.509) that lives `lifetime` from the clock it is given, serves it until a third of its life
/// is left, and then issues another with a new serial and a new key. It can be made to fail
/// ([`set_failing`](Self::set_failing)): while it fails it keeps serving a certificate that is still
/// valid and answers the error once there is none, as the port says an error means no identity.
/// Clones share the same state.
#[derive(Clone)]
pub struct MemoryPlatformClientCertificates {
    clock: FixedClock,
    lifetime: SignedDuration,
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for MemoryPlatformClientCertificates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryPlatformClientCertificates")
            .field("lifetime", &self.lifetime)
            .finish_non_exhaustive()
    }
}

impl MemoryPlatformClientCertificates {
    /// A source whose certificates live `lifetime` on `clock`.
    pub fn new(clock: FixedClock, lifetime: Duration) -> Self {
        Self {
            clock,
            lifetime: SignedDuration::try_from(lifetime)
                .unwrap_or(SignedDuration::from_secs(86_400)),
            state: Arc::default(),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Makes the issuer fail with `error` (`Some`) or work again (`None`), **and moves its clock
    /// forward by [`FAILURE_PAUSE_PASSED`]** (every call, in either direction): the testkit's
    /// contract is that a change of what the issuer does takes a source past any pause it keeps
    /// after a failure, and the clock is this source's own. It keeps no pause itself, so for it the
    /// move is only a few seconds of a life of hours.
    pub fn set_failing(&self, error: Option<PlatformCredentialsError>) {
        self.clock.advance(FAILURE_PAUSE_PASSED);
        self.state().failing = error;
    }

    /// How many certificates were issued.
    pub fn issued(&self) -> u64 {
        self.state().issued
    }

    fn answer(&self) -> Result<PlatformClientCertificate, PlatformCredentialsError> {
        let now = self.clock.now();
        let mut state = self.state();
        let due_in = self.lifetime / 3;
        if let Some(held) = state.held.as_ref().filter(|h| now < h.not_after()) {
            let due = match held.not_after().checked_sub(due_in) {
                Ok(renew_at) => now >= renew_at,
                Err(_) => true,
            };
            if !due || state.failing.is_some() {
                return Ok(held.clone());
            }
        }
        if let Some(error) = state.failing {
            state.held = None;
            return Err(error);
        }
        let not_after: Timestamp = now
            .checked_add(self.lifetime)
            .map_err(|_| PlatformCredentialsError::Unavailable)?;
        state.issued += 1;
        let n = state.issued;
        let cert = PlatformClientCertificate::new(
            vec![
                format!("memory-leaf-{n}").into_bytes(),
                b"memory-intermediate".to_vec(),
            ],
            Zeroizing::new(format!("memory-key-{n}-0123456789abcdef").into_bytes()),
            format!("{n:02x}"),
            not_after,
        );
        state.held = Some(cert.clone());
        Ok(cert)
    }
}

impl PlatformClientCertificates for MemoryPlatformClientCertificates {
    fn current(&self) -> PlatformCertificateFuture<'_> {
        let answer = self.answer();
        Box::pin(async move { answer })
    }
}
