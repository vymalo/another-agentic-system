//! The certificate mode of the adapter against a **real TLS listener**: rustls with a
//! `WebPkiClientVerifier` over a root minted here, in front of a stand-in of the owner routes. What
//! the listener saw is what is asserted: the chain the client presented, whether an `Authorization`
//! header came, how many connections were opened, and that nothing at all reached it when there
//! was no identity to present.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use jiff::{SignedDuration, Timestamp};
use orch_ports::memory::FixedClock;
use orch_ports::{
    AgentKind, AgentOwner, AgentProvisioner, Clock, PlatformCertificateFuture,
    PlatformClientCertificate, PlatformClientCertificates, PlatformCredentialsError,
    ProvisionError,
};
use orch_provisioner_platform::{BuildError, PlatformConfig, PlatformProvisioner};
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use zeroize::Zeroizing;

fn odt(offset_secs: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(Timestamp::now().as_second() + offset_secs).unwrap()
}

/// A root, an intermediate under it, and what each signs.
struct Authority {
    root_der: Vec<u8>,
    root_pem: String,
    inter_der: Vec<u8>,
    issuer: Issuer<'static, KeyPair>,
}

fn authority(name: &str) -> Authority {
    let mut root = CertificateParams::new(Vec::new()).unwrap();
    root.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root.distinguished_name
        .push(DnType::CommonName, format!("{name} root"));
    root.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    root.not_before = odt(-86_400);
    root.not_after = odt(86_400 * 365);
    let root_key = KeyPair::generate().unwrap();
    let root_cert = root.self_signed(&root_key).unwrap();
    let root_issuer = Issuer::new(root, root_key);

    let mut inter = CertificateParams::new(Vec::new()).unwrap();
    inter.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    inter
        .distinguished_name
        .push(DnType::CommonName, format!("{name} intermediate"));
    inter.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    inter.not_before = odt(-86_400);
    inter.not_after = odt(86_400 * 365);
    let inter_key = KeyPair::generate().unwrap();
    let inter_cert = inter.signed_by(&inter_key, &root_issuer).unwrap();
    Authority {
        root_der: root_cert.der().to_vec(),
        root_pem: root_cert.pem(),
        inter_der: inter_cert.der().to_vec(),
        issuer: Issuer::new(inter, inter_key),
    }
}

impl Authority {
    /// A client certificate: leaf first, then the intermediate (no root), with its key, good for a day.
    fn client(&self, serial: u64) -> PlatformClientCertificate {
        self.client_until(serial, Timestamp::now() + SignedDuration::from_secs(86_400))
    }

    /// The same, whose end of validity (as the port reports it) is `not_after`.
    fn client_until(&self, serial: u64, not_after: Timestamp) -> PlatformClientCertificate {
        let mut params = CertificateParams::new(Vec::new()).unwrap();
        params.distinguished_name = rcgen::DistinguishedName::new();
        params.subject_alt_names = vec![SanType::URI(
            "spiffe://example.org/ns/orchestrator/sa/orchestrator"
                .try_into()
                .unwrap(),
        )];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.serial_number = Some(rcgen::SerialNumber::from(serial));
        params.not_before = odt(-60);
        params.not_after = odt(86_400);
        let key = KeyPair::generate().unwrap();
        let leaf = params.signed_by(&key, &self.issuer).unwrap();
        PlatformClientCertificate::new(
            vec![leaf.der().to_vec(), self.inter_der.clone()],
            Zeroizing::new(key.serialize_der()),
            format!("{serial:02x}"),
            not_after,
        )
    }

    /// A server certificate for 127.0.0.1, signed by the root's intermediate, with its chain.
    fn server(&self) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
        let mut params = CertificateParams::new(Vec::new()).unwrap();
        params.subject_alt_names = vec![SanType::IpAddress("127.0.0.1".parse().unwrap())];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.not_before = odt(-60);
        params.not_after = odt(86_400);
        let key = KeyPair::generate().unwrap();
        let leaf = params.signed_by(&key, &self.issuer).unwrap();
        (
            vec![
                CertificateDer::from(leaf.der().to_vec()),
                CertificateDer::from(self.inter_der.clone()),
            ],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
    }
}

/// What the stub certificate source answers, and how often it was asked.
struct Stub {
    answer: Mutex<Result<PlatformClientCertificate, PlatformCredentialsError>>,
    asked: AtomicUsize,
}

impl Stub {
    fn new(answer: Result<PlatformClientCertificate, PlatformCredentialsError>) -> Arc<Self> {
        Arc::new(Stub {
            answer: Mutex::new(answer),
            asked: AtomicUsize::new(0),
        })
    }
    fn set(&self, answer: Result<PlatformClientCertificate, PlatformCredentialsError>) {
        *self.answer.lock().unwrap() = answer;
    }
}

impl PlatformClientCertificates for Stub {
    fn current(&self) -> PlatformCertificateFuture<'_> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let answer = self.answer.lock().unwrap().clone();
        Box::pin(async move { answer })
    }
}

/// What the listener saw.
#[derive(Default)]
struct Seen {
    /// TCP connections accepted.
    tcp: AtomicUsize,
    /// TLS handshakes that failed (a client the verifier refused).
    refused: AtomicUsize,
    /// The chain each completed handshake's client presented.
    chains: Mutex<Vec<Vec<Vec<u8>>>>,
    /// Whether each request carried an `Authorization` header.
    authorization: Mutex<Vec<bool>>,
}

struct TlsListener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
    seen: Arc<Seen>,
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let Ok((tcp, addr)) = self.tcp.accept().await else {
                continue;
            };
            self.seen.tcp.fetch_add(1, Ordering::SeqCst);
            match self.acceptor.accept(tcp).await {
                Ok(tls) => {
                    let chain = tls
                        .get_ref()
                        .1
                        .peer_certificates()
                        .map(|chain| chain.iter().map(|der| der.to_vec()).collect())
                        .unwrap_or_default();
                    self.seen.chains.lock().unwrap().push(chain);
                    return (tls, addr);
                }
                Err(_) => {
                    self.seen.refused.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.tcp.local_addr()
    }
}

struct Server {
    url: String,
    seen: Arc<Seen>,
    authority: Authority,
}

/// A TLS server that requires a client certificate under the authority's root, and answers the one
/// read the adapter's `ensure_ready` needs: the agent is ready.
async fn serve() -> Server {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let authority = authority("platform");
    let mut roots = RootCertStore::empty();
    roots
        .add(CertificateDer::from(authority.root_der.clone()))
        .unwrap();
    let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .unwrap();
    let (chain, key) = authority.server();
    let config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)
        .unwrap();
    let seen = Arc::new(Seen::default());
    let app = Router::new().route(
        "/v1/owners/{owner}/agents/{name}",
        get({
            let seen = seen.clone();
            move |Path((_owner, name)): Path<(String, String)>, headers: HeaderMap| {
                let seen = seen.clone();
                async move {
                    seen.authorization
                        .lock()
                        .unwrap()
                        .push(headers.contains_key(header::AUTHORIZATION));
                    (
                        StatusCode::OK,
                        [(header::ETAG, "\"1-1\"")],
                        serde_json::json!({"name": name, "state": "ready", "conditions": []})
                            .to_string(),
                    )
                        .into_response()
                }
            }
        }),
    );
    let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("https://{}", tcp.local_addr().unwrap());
    let listener = TlsListener {
        tcp,
        acceptor: TlsAcceptor::from(Arc::new(config)),
        seen: seen.clone(),
    };
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Server {
        url,
        seen,
        authority,
    }
}

fn provisioner(server: &Server, certs: Arc<Stub>) -> PlatformProvisioner {
    let config = PlatformConfig::with_client_certificate(server.url.clone(), certs)
        .with_server_ca_pem(server.authority.root_pem.clone())
        .without_system_proxy();
    PlatformProvisioner::new(config).unwrap()
}

fn owner() -> AgentOwner {
    AgentOwner::new("alice").unwrap()
}

#[tokio::test]
async fn the_leaf_and_its_intermediate_are_presented_and_no_authorization_is_sent() {
    let server = serve().await;
    let cert = server.authority.client(0x0a);
    let expected: Vec<Vec<u8>> = cert.chain_der().to_vec();
    let provisioner = provisioner(&server, Stub::new(Ok(cert)));

    let agent = provisioner
        .ensure_ready(&owner(), AgentKind::Coder)
        .await
        .unwrap();
    assert_eq!(agent.name, "coder-alice");

    // The listener verified the chain and saw exactly the leaf and its intermediate.
    assert_eq!(*server.seen.chains.lock().unwrap(), [expected]);
    // And no bearer was sent: the certificate is the identity.
    let authorization = server.seen.authorization.lock().unwrap().clone();
    assert!(!authorization.is_empty(), "no request reached the listener");
    assert!(authorization.iter().all(|sent| !sent), "{authorization:?}");
}

#[tokio::test]
async fn no_identity_means_nothing_is_sent() {
    let server = serve().await;
    for failure in [
        PlatformCredentialsError::Refused,
        PlatformCredentialsError::Unavailable,
    ] {
        let stub = Stub::new(Err(failure));
        let provisioner = provisioner(&server, stub.clone());
        let error = provisioner
            .ensure_ready(&owner(), AgentKind::Coder)
            .await
            .unwrap_err();
        match failure {
            PlatformCredentialsError::Refused => {
                assert!(matches!(error, ProvisionError::Refused), "{error:?}");
            }
            _ => assert!(
                matches!(error, ProvisionError::Unavailable { .. }),
                "{error:?}"
            ),
        }
        assert_eq!(stub.asked.load(Ordering::SeqCst), 1, "it was asked");
    }
    // Not one connection, not even a handshake that would have shown a missing certificate.
    assert_eq!(server.seen.tcp.load(Ordering::SeqCst), 0);
    assert!(server.seen.authorization.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_new_serial_opens_a_new_connection_and_the_same_serial_keeps_the_old_one() {
    let server = serve().await;
    let first = server.authority.client(0x0a);
    let first_leaf = first.chain_der()[0].clone();
    let stub = Stub::new(Ok(first));
    let provisioner = provisioner(&server, stub.clone());

    provisioner
        .ensure_ready(&owner(), AgentKind::Coder)
        .await
        .unwrap();
    provisioner
        .ensure_ready(&owner(), AgentKind::Coder)
        .await
        .unwrap();
    assert_eq!(
        server.seen.tcp.load(Ordering::SeqCst),
        1,
        "the same certificate reuses its connection"
    );

    // A renewal: a new leaf under a new serial, which must be what the next request presents.
    let second = server.authority.client(0x0b);
    let second_leaf = second.chain_der()[0].clone();
    stub.set(Ok(second));
    provisioner
        .ensure_ready(&owner(), AgentKind::Coder)
        .await
        .unwrap();
    assert_eq!(server.seen.tcp.load(Ordering::SeqCst), 2);
    provisioner
        .ensure_ready(&owner(), AgentKind::Coder)
        .await
        .unwrap();
    assert_eq!(server.seen.tcp.load(Ordering::SeqCst), 2);

    let chains = server.seen.chains.lock().unwrap().clone();
    assert_eq!(chains.len(), 2);
    assert_eq!(chains[0][0], first_leaf);
    assert_eq!(chains[1][0], second_leaf);
}

#[tokio::test]
async fn a_certificate_of_another_authority_is_refused_by_the_listener() {
    let server = serve().await;
    let stranger = authority("stranger");
    let provisioner = provisioner(&server, Stub::new(Ok(stranger.client(0x0a))));
    let error = provisioner
        .ensure_ready(&owner(), AgentKind::Coder)
        .await
        .unwrap_err();
    assert!(
        matches!(error, ProvisionError::Unavailable { .. }),
        "{error:?}"
    );
    // The check is the listener's own: it refused the handshake (its own count, which an error from
    // anywhere else would not move), and no request got through.
    for _ in 0..100 {
        if server.seen.refused.load(Ordering::SeqCst) >= 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        server.seen.refused.load(Ordering::SeqCst) >= 1,
        "the listener refused no handshake"
    );
    assert!(server.seen.authorization.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_certificate_that_is_over_is_not_presented_and_nothing_is_sent() {
    let server = serve().await;
    // One clock for the certificate's end of validity and for the provisioner's guard, and it is the
    // one that moves. The port should never answer a certificate that is over, but if it does the
    // provisioner is the second check: at or before now is over, and not one new connection is opened.
    let clock = FixedClock::new(Timestamp::now());
    let end = clock.now() + SignedDuration::from_secs(10);
    let stub = Stub::new(Ok(server.authority.client_until(0x0c, end)));
    let config = PlatformConfig::with_client_certificate(server.url.clone(), stub.clone())
        .with_server_ca_pem(server.authority.root_pem.clone())
        .with_clock(Arc::new(clock.clone()))
        .without_system_proxy();
    let provisioner = PlatformProvisioner::new(config).unwrap();

    // Before its end it is presented.
    provisioner
        .ensure_ready(&owner(), AgentKind::Coder)
        .await
        .unwrap();
    assert_eq!(server.seen.tcp.load(Ordering::SeqCst), 1);

    // Exactly at its end, and after it, nothing is sent.
    for past in [10, 3_600] {
        clock.advance(std::time::Duration::from_secs(past));
        let error = provisioner
            .ensure_ready(&owner(), AgentKind::Coder)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ProvisionError::Unavailable { .. }),
            "{error:?}"
        );
    }
    assert_eq!(
        stub.asked.load(Ordering::SeqCst),
        3,
        "it was asked each time"
    );
    assert_eq!(
        server.seen.tcp.load(Ordering::SeqCst),
        1,
        "no connection after the end"
    );
    assert_eq!(server.seen.authorization.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_certificate_needs_an_https_url_and_a_readable_server_ca() {
    let stub = Stub::new(Err(PlatformCredentialsError::Unavailable));
    let plain = PlatformConfig::with_client_certificate("http://platform.internal", stub.clone());
    assert!(matches!(
        PlatformProvisioner::new(plain),
        Err(BuildError::CertificateNeedsHttps)
    ));
    let bad_ca = PlatformConfig::with_client_certificate("https://platform.internal", stub)
        .with_server_ca_pem("not pem");
    assert!(matches!(
        PlatformProvisioner::new(bad_ca),
        Err(BuildError::BadServerCa)
    ));
    // Shown, a configuration says no more than its mode.
    let shown = format!(
        "{:?}",
        PlatformConfig::with_client_certificate(
            "https://platform.internal",
            Stub::new(Err(PlatformCredentialsError::Refused))
        )
    );
    assert!(shown.contains("ClientCertificate(<redacted>)"), "{shown}");
}
