//! A connector layer that judges every connection of the client of the servers people name, as
//! defence in depth behind the resolver hook and the judgement of the URL (ADR 0046, slice 5a
//! review).
//!
//! The resolver hook sees only names: the HTTP client's connector parses an address written in a
//! URL and never asks a resolver about it. `McpToolClient::guarded` judges the endpoint's address
//! before it builds the transport, and follows no redirect, so today that is enough. This layer
//! makes it true by construction instead: **every connection the client is asked to make** is
//! judged, whatever got it there (a literal address, a redirect target if redirects were ever
//! turned on, a name the hook was bypassed for), both
//!
//! 1. **before** the connection is made, on the destination the connector was given: an address
//!    written in the URL is judged by [`EgressPolicy::check_resolved`] and nothing is connected to
//!    when it is refused; and
//! 2. **after** it is made and **before** a byte of the request is written, on the address of the
//!    peer: the connection is dropped when it was made to an address the lists refuse, so the
//!    request, and the credential in its headers, never leave. (The TCP and TLS handshakes have
//!    happened by then, which carry no credential.)
//!
//! `reqwest` hands a connector layer the destination as a type it does not let anyone name
//! (`Unnameable`, whose only public face is `Debug`), so the destination is read from its `Debug`
//! text, `Unnameable(http://host:port/)`. A text this layer cannot read is **refused**, never
//! passed: a version of reqwest that changed it fails every connection of the guarded client, and
//! the tests of this crate fail first. The peer's address is read from the `HttpInfo` the connector
//! puts on every connection (the same one `Response::remote_addr` reports); a connection without it
//! is refused as well.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use http::Extensions;
use hyper_util::client::legacy::connect::{Connection, HttpInfo};
use orch_ports::{EgressPolicy, EgressRefusal};
use tower::{Layer, Service};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The layer: wraps the client's connector.
#[derive(Clone)]
pub(crate) struct PeerGuard {
    policy: EgressPolicy,
}

impl PeerGuard {
    pub(crate) fn new(policy: EgressPolicy) -> Self {
        PeerGuard { policy }
    }
}

impl<S> Layer<S> for PeerGuard {
    type Service = Guarded<S>;

    fn layer(&self, inner: S) -> Guarded<S> {
        Guarded {
            inner,
            policy: self.policy.clone(),
        }
    }
}

/// The connector with the layer on it.
#[derive(Clone)]
pub(crate) struct Guarded<S> {
    inner: S,
    policy: EgressPolicy,
}

/// The destination, as the text `Unnameable(<uri>)` of reqwest's `Debug` says it.
fn destination(request: &dyn fmt::Debug) -> Option<String> {
    let text = format!("{request:?}");
    let uri = text.strip_prefix("Unnameable(")?.strip_suffix(')')?;
    (!uri.is_empty()).then(|| uri.to_owned())
}

fn refused(refusal: EgressRefusal) -> BoxError {
    Box::new(refusal)
}

impl<S, R> Service<R> for Guarded<S>
where
    S: Service<R> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Response: Connection + Send + 'static,
    S::Error: Into<BoxError>,
    R: fmt::Debug + Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, BoxError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, request: R) -> Self::Future {
        let policy = self.policy.clone();
        let target = destination(&request).map(|uri| policy.check_url(&uri));
        // the service that was polled ready is the one that is called
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);
        Box::pin(async move {
            // a destination that cannot be read is not connected to
            let target = target
                .ok_or_else(|| refused(EgressRefusal::Host))?
                .map_err(refused)?;
            if let Some(addr) = target.literal_addr() {
                policy.check_resolved(&target, &[addr]).map_err(refused)?;
            }
            let connection = inner.call(request).await.map_err(Into::into)?;
            let mut extras = Extensions::new();
            connection.connected().get_extras(&mut extras);
            let peer = extras
                .get::<HttpInfo>()
                .map(HttpInfo::remote_addr)
                .ok_or_else(|| refused(EgressRefusal::NoAddress))?;
            policy.check_resolved(&target, &[peer]).map_err(refused)?;
            Ok(connection)
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use orch_ports::HostPattern;

    use super::*;

    fn pattern(text: &str) -> HostPattern {
        text.parse().unwrap()
    }

    /// A listener that counts the connections made to it and answers nothing.
    async fn tripwire(address: &str) -> (SocketAddr, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind((address, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&count);
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                held.push(socket);
            }
        });
        (addr, count)
    }

    /// A client with the layer and nothing else of the guard: no resolver hook, so that only the
    /// layer can refuse.
    fn layered(policy: EgressPolicy) -> reqwest::Client {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(std::time::Duration::from_secs(2))
            .connector_layer(PeerGuard::new(policy))
            .build()
            .unwrap()
    }

    #[test]
    fn the_destination_is_read_from_the_text_reqwest_gives() {
        // the shape of reqwest's own type: a tuple struct over the `Uri`
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Unnameable(http::Uri);
        assert_eq!(
            destination(&Unnameable("http://127.0.0.1:8080/".parse().unwrap())).as_deref(),
            Some("http://127.0.0.1:8080/")
        );
        // anything else is unreadable, and unreadable is refused
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Other(&'static str);
        assert_eq!(destination(&Other("http://127.0.0.1/")), None);
    }

    #[tokio::test]
    async fn a_literal_private_address_is_refused_before_the_connection_is_made() {
        // The resolver hook is not in this client, and a literal never reaches it anyway: only
        // the layer stands between the request and the listener.
        let (addr, connections) = tripwire("127.0.0.1").await;
        let client = layered(EgressPolicy::new().allowing_http());
        let error = client
            .post(format!("http://{addr}/mcp"))
            .header("authorization", "Bearer SENTINEL-not-sent")
            .send()
            .await
            .unwrap_err();
        let refusal = {
            let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
            let mut found = None;
            while let Some(e) = source {
                if let Some(r) = e.downcast_ref::<EgressRefusal>() {
                    found = Some(*r);
                }
                source = e.source();
            }
            found
        };
        assert_eq!(refusal, Some(EgressRefusal::PrivateAddress), "{error:?}");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            connections.load(Ordering::SeqCst),
            0,
            "a connection was made"
        );
    }

    #[tokio::test]
    async fn an_admitted_address_connects_and_the_peer_is_judged_after() {
        let (addr, connections) = tripwire("127.0.0.1").await;
        let policy = EgressPolicy::new()
            .allowing_http()
            .with_allow(pattern("127.0.0.1"));
        // the tripwire answers nothing, so the request times out; the connection was made
        let client = layered(policy);
        let _ = client
            .get(format!("http://{addr}/mcp"))
            .timeout(std::time::Duration::from_millis(300))
            .send()
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(connections.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_connection_made_to_an_address_the_lists_refuse_is_dropped_before_the_request() {
        // The name is allowed (so the destination passes the first judgement) and the connection
        // is made to a loopback address, which the policy's deny list names: the second
        // judgement, on the peer, refuses it, and no byte of the request is written.
        let (addr, connections) = tripwire("127.0.0.1").await;
        let policy = EgressPolicy::new()
            .allowing_http()
            .with_allow(pattern("fake-host.example"))
            .with_deny(pattern("127.0.0.1"));
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let client = reqwest::Client::builder()
            .no_proxy()
            .connector_layer(PeerGuard::new(policy))
            // the name resolves, outside the guard, to the tripwire
            .resolve("fake-host.example", addr)
            .build()
            .unwrap();
        let error = client
            .get(format!("http://fake-host.example:{}/mcp", addr.port()))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .unwrap_err();
        assert!(error.is_connect(), "{error:?}");
        // the connection was made (the layer sees a made connection) and then dropped, unused
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(connections.load(Ordering::SeqCst), 1);
    }
}
