//! How a host name becomes addresses for an HTTP client that judges its connections (ADR 0046,
//! decision 5), and the one verdict on the answer.
//!
//! The credential broker and the MCP client of the servers people name each have a resolver hook
//! on their HTTP client. Both ask a [`HostLookup`] for the addresses of a name, give the answer to
//! [`judge_lookup`], and hand the HTTP client only the addresses that passed, so the address that
//! was judged is the address connected to. The trait and the verdict are here once; the hook
//! itself (a `reqwest::dns::Resolve`) stays in each crate, because a port names no HTTP library.
//!
//! **A lookup that never answers is bounded by the caller**, with [`LOOKUP_TIMEOUT`]: the system
//! resolver runs `getaddrinfo` on the blocking pool, and that thread cannot be cancelled. Dropping
//! the future (what the timeout does) lets the request go on and the connection attempt fail, and
//! the thread is freed when the operating system's resolver gives up. It is why the number of
//! requests to people's servers in flight is bounded as well (`orch-surface-thread-tools`).

use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::pin::Pin;
use std::time::Duration;

use crate::{CheckedAddrs, EgressPolicy, EgressRefusal};

/// The longest a resolver hook waits for a [`HostLookup`] to answer: 5 seconds. The HTTP clients'
/// own connect timeout is 10, so a name that does not answer is told apart from a server that does
/// not accept.
pub const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

/// The future of a [`HostLookup`].
pub type LookupFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Vec<IpAddr>>> + Send + 'a>>;

/// How a host name becomes addresses. The default of each adapter asks the operating system's
/// resolver; a composition root or a test may put another in (the guard judges whatever it
/// answers, and the connection is made only to those addresses).
pub trait HostLookup: Send + Sync + 'static {
    /// The addresses `host` (a name, never a URL) resolves to.
    fn lookup<'a>(&'a self, host: &'a str) -> LookupFuture<'a>;
}

/// Why a resolver hook gave the HTTP client no address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LookupFailure {
    /// The guard refused the host or an address it resolved to. The rule is the cause, which the
    /// HTTP client keeps in the chain of its connect error.
    #[error("the egress guard refused the host")]
    Refused(#[source] EgressRefusal),
    /// The lookup failed.
    #[error("the host name could not be resolved")]
    NotResolved,
    /// The lookup did not answer within [`LOOKUP_TIMEOUT`].
    #[error("the host name did not resolve in time")]
    TimedOut,
}

/// The verdict on the answer of a lookup, for the hook of an HTTP client that was asked to connect
/// with `host` (a name, or an address in brackets or not): the addresses to hand the client, or
/// why none. `answer` is what the lookup said; for an address there is nothing to look up and the
/// hook passes it as the answer. The lists apply to `host` as to the host of a URL
/// ([`EgressPolicy::check_connection`]).
///
/// # Errors
/// [`LookupFailure::NotResolved`] for a lookup that failed, [`LookupFailure::Refused`] for the
/// guard's refusal.
pub fn judge_lookup(
    policy: &EgressPolicy,
    host: &str,
    answer: io::Result<Vec<IpAddr>>,
) -> Result<CheckedAddrs, LookupFailure> {
    let ips = answer.map_err(|_| LookupFailure::NotResolved)?;
    policy
        .check_connection(host, &ips)
        .map_err(LookupFailure::Refused)
}
