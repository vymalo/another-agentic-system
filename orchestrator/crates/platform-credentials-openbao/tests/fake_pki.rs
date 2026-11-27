//! The adapter against an in-process stand-in of OpenBao's Kubernetes login and PKI signing
//! (no cluster, no OpenBao): a root and an intermediate minted here, a login that reads the
//! service-account token, a signing endpoint that parses the CSR it is sent and answers a leaf
//! dated by a fake clock, and modes that refuse, fail or mis-issue.
//!
//! The stand-in is written from OpenBao's documented API (`auth/<mount>/login`,
//! `<pki>/sign/<role>`), not from a live server: what it proves is the adapter's own behaviour
//! (what it asks, what it checks, when it renews), never OpenBao's.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use jiff::Timestamp;
use orch_core::{Classify, ErrorClass};
use orch_platform_credentials_openbao::{BuildError, OpenBaoClientCertificates, OpenBaoPkiConfig};
use orch_ports::memory::FixedClock;
use orch_ports::{PlatformClientCertificates, PlatformCredentialsError};
use rcgen::{
    BasicConstraints, CertificateParams, CertificateSigningRequestParams, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, PublicKeyData, SanType,
    SerialNumber,
};
use serde_json::{Value, json};

const SPIFFE: &str = "spiffe://example.org/ns/orchestrator/sa/orchestrator";
const AUTH_ROLE: &str = "orchestrator-platform";
const PKI_MOUNT: &str = "pki-workload";
const PKI_ROLE: &str = "orchestrator-platform-client";
/// What the pod's token file holds: a planted value that must never appear anywhere else.
const SA_JWT: &str = "SENTINEL-SA-JWT-0123456789";
/// What OpenBao's login answers: the same.
const CLIENT_TOKEN: &str = "s.SENTINEL-CLIENT-TOKEN-abcdef";
const START: &str = "2026-11-27T12:00:00Z";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mis {
    /// clientAuth and serverAuth both.
    ServerAuthToo,
    /// serverAuth only.
    ServerAuthOnly,
    /// No extended key usage at all.
    NoEku,
    /// Two URI names.
    TwoUris,
    /// One URI name, not ours.
    OtherUri,
    /// Our URI name and a DNS name.
    ExtraDnsName,
    /// No name.
    NoName,
    /// A leaf for a key that is not the one in the CSR.
    ForeignKey,
    /// Already over.
    Expired,
    /// Valid, and for less than the renewal margin.
    ShortLived,
    /// clientAuth and anyExtendedKeyUsage.
    AnyEku,
    /// A certificate authority (`cA` true).
    CaLeaf,
    /// Not a CA, but allowed to sign certificates (`keyCertSign`).
    KeyCertSign,
    /// Issued by itself: issuer and subject are the same name.
    SelfSigned,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Ok,
    LoginRefused,
    LoginDown,
    SignRefused,
    SignDown,
    /// A 200 that is not JSON.
    Garbage,
    Mis(Mis),
}

struct Authority {
    root_der: Vec<u8>,
    root_pem: String,
    inter_der: Vec<u8>,
    inter_pem: String,
    issuer: Issuer<'static, KeyPair>,
    /// Nine more intermediates under the root, for a chain that is too long.
    extras_pem: Vec<String>,
}

fn odt(at: Timestamp) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp(at.as_second()).unwrap()
}

fn authority(now: Timestamp) -> Authority {
    let ten_years = Duration::from_secs(10 * 365 * 24 * 3600);
    let mut root = CertificateParams::new(Vec::new()).unwrap();
    root.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root.distinguished_name
        .push(DnType::CommonName, "Fake Platform Root");
    root.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    root.not_before = odt(now.checked_sub(Duration::from_secs(86_400)).unwrap());
    root.not_after = odt(now.checked_add(ten_years).unwrap());
    let root_key = KeyPair::generate().unwrap();
    let root_cert = root.self_signed(&root_key).unwrap();
    let root_issuer = Issuer::new(root, root_key);

    let mut inter = CertificateParams::new(Vec::new()).unwrap();
    inter.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    inter
        .distinguished_name
        .push(DnType::CommonName, "Fake Workload Intermediate");
    inter.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    inter.not_before = odt(now.checked_sub(Duration::from_secs(86_400)).unwrap());
    inter.not_after = odt(now.checked_add(ten_years).unwrap());
    let inter_key = KeyPair::generate().unwrap();
    let inter_cert = inter.signed_by(&inter_key, &root_issuer).unwrap();
    let extras_pem = (0..9)
        .map(|i| {
            let mut extra = CertificateParams::new(Vec::new()).unwrap();
            extra.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
            extra
                .distinguished_name
                .push(DnType::CommonName, format!("Fake Extra {i}"));
            extra.key_usages = vec![KeyUsagePurpose::KeyCertSign];
            extra.not_before = odt(now.checked_sub(Duration::from_secs(86_400)).unwrap());
            extra.not_after = odt(now.checked_add(ten_years).unwrap());
            extra
                .signed_by(&KeyPair::generate().unwrap(), &root_issuer)
                .unwrap()
                .pem()
        })
        .collect();
    Authority {
        extras_pem,
        root_der: root_cert.der().to_vec(),
        root_pem: root_cert.pem(),
        inter_der: inter_cert.der().to_vec(),
        inter_pem: inter_cert.pem(),
        issuer: Issuer::new(inter, inter_key),
    }
}

struct Fake {
    authority: Authority,
    clock: FixedClock,
    mode: Mutex<Mode>,
    logins: AtomicUsize,
    signs: AtomicUsize,
    serial: AtomicU64,
    sign_delay_ms: AtomicU64,
    /// Answer the root in `ca_chain` whatever the request says about `remove_roots_from_chain`.
    always_send_root: AtomicBool,
    /// How many of the authority's extra intermediates to append to `ca_chain`.
    extra_chain: AtomicUsize,
    /// Answer `"ca_chain": null`.
    null_chain: AtomicBool,
    jwts: Mutex<Vec<String>>,
    sign_bodies: Mutex<Vec<Value>>,
    /// The public point of the key of each CSR.
    csr_keys: Mutex<Vec<Vec<u8>>>,
    /// For each CSR: how many names it asked for and how many parts its subject had.
    csr_names: Mutex<Vec<(usize, usize)>>,
    /// The DER of each leaf issued.
    leaves: Mutex<Vec<Vec<u8>>>,
}

type Shared = Arc<Fake>;

impl Fake {
    fn mode(&self) -> Mode {
        *self.mode.lock().unwrap()
    }
    fn set(&self, mode: Mode) {
        *self.mode.lock().unwrap() = mode;
    }
    fn logins(&self) -> usize {
        self.logins.load(Ordering::SeqCst)
    }
    fn signs(&self) -> usize {
        self.signs.load(Ordering::SeqCst)
    }
}

async fn login(State(fake): State<Shared>, Json(body): Json<Value>) -> Response {
    fake.logins.fetch_add(1, Ordering::SeqCst);
    fake.jwts
        .lock()
        .unwrap()
        .push(body["jwt"].as_str().unwrap_or_default().to_owned());
    if body["role"] != AUTH_ROLE {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match fake.mode() {
        Mode::LoginRefused => StatusCode::FORBIDDEN.into_response(),
        Mode::LoginDown => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        _ => Json(json!({"auth": {"client_token": CLIENT_TOKEN, "lease_duration": 60}}))
            .into_response(),
    }
}

async fn sign(State(fake): State<Shared>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    fake.signs.fetch_add(1, Ordering::SeqCst);
    let delay = fake.sign_delay_ms.load(Ordering::SeqCst);
    if delay > 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    let token = headers.get("x-vault-token").and_then(|v| v.to_str().ok());
    if token != Some(CLIENT_TOKEN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match fake.mode() {
        Mode::SignRefused => return StatusCode::FORBIDDEN.into_response(),
        Mode::SignDown => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Mode::Garbage => return (StatusCode::OK, "<html>not json</html>").into_response(),
        _ => {}
    }
    fake.sign_bodies.lock().unwrap().push(body.clone());
    let csr = CertificateSigningRequestParams::from_pem(body["csr"].as_str().unwrap()).unwrap();
    fake.csr_keys
        .lock()
        .unwrap()
        .push(csr.public_key.der_bytes().to_vec());
    fake.csr_names.lock().unwrap().push((
        csr.params.subject_alt_names.len(),
        csr.params.distinguished_name.iter().count(),
    ));

    let now = fake.clock.now();
    let ttl: u64 = body["ttl"]
        .as_str()
        .and_then(|t| t.strip_suffix('s'))
        .and_then(|t| t.parse().ok())
        .unwrap();
    let mut not_before = now.checked_sub(Duration::from_secs(30)).unwrap();
    let mut not_after = now.checked_add(Duration::from_secs(ttl)).unwrap();
    let mis = match fake.mode() {
        Mode::Mis(mis) => Some(mis),
        _ => None,
    };
    match mis {
        Some(Mis::Expired) => {
            not_before = now.checked_sub(Duration::from_secs(7_200)).unwrap();
            not_after = now.checked_sub(Duration::from_secs(3_600)).unwrap();
        }
        Some(Mis::ShortLived) => {
            not_after = now.checked_add(Duration::from_secs(3_600)).unwrap();
        }
        _ => {}
    }
    let uri = |text: &str| SanType::URI(text.try_into().unwrap());
    let ours = uri(body["uri_sans"].as_str().unwrap());
    let names = match mis {
        Some(Mis::TwoUris) => vec![ours, uri("spiffe://example.org/other")],
        Some(Mis::OtherUri) => vec![uri("spiffe://example.org/ns/other/sa/other")],
        Some(Mis::ExtraDnsName) => vec![ours, SanType::DnsName("example.org".try_into().unwrap())],
        Some(Mis::NoName) => Vec::new(),
        _ => vec![ours],
    };
    let usages = match mis {
        Some(Mis::ServerAuthToo) => vec![
            ExtendedKeyUsagePurpose::ClientAuth,
            ExtendedKeyUsagePurpose::ServerAuth,
        ],
        Some(Mis::ServerAuthOnly) => vec![ExtendedKeyUsagePurpose::ServerAuth],
        Some(Mis::AnyEku) => vec![
            ExtendedKeyUsagePurpose::ClientAuth,
            ExtendedKeyUsagePurpose::Any,
        ],
        Some(Mis::NoEku) => Vec::new(),
        _ => vec![ExtendedKeyUsagePurpose::ClientAuth],
    };

    let mut params = csr.params.clone();
    params.not_before = odt(not_before);
    params.not_after = odt(not_after);
    params.serial_number = Some(SerialNumber::from(
        1000 + fake.serial.fetch_add(1, Ordering::SeqCst) + 1,
    ));
    params.subject_alt_names = names;
    params.extended_key_usages = usages;
    params.key_usages = match mis {
        Some(Mis::KeyCertSign) => vec![
            KeyUsagePurpose::DigitalSignature,
            KeyUsagePurpose::KeyCertSign,
        ],
        _ => vec![KeyUsagePurpose::DigitalSignature],
    };
    params.is_ca = match mis {
        Some(Mis::CaLeaf) => IsCa::Ca(BasicConstraints::Constrained(0)),
        _ => IsCa::NoCa,
    };
    let leaf = if mis == Some(Mis::ForeignKey) {
        params
            .signed_by(&KeyPair::generate().unwrap(), &fake.authority.issuer)
            .unwrap()
    } else if mis == Some(Mis::SelfSigned) {
        // The same name as subject and as issuer, signed by a key that is not ours.
        params.distinguished_name = rcgen::DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, "self");
        let mut own = CertificateParams::new(Vec::new()).unwrap();
        own.distinguished_name.push(DnType::CommonName, "self");
        params
            .signed_by(
                &csr.public_key,
                &Issuer::new(own, KeyPair::generate().unwrap()),
            )
            .unwrap()
    } else {
        params
            .signed_by(&csr.public_key, &fake.authority.issuer)
            .unwrap()
    };
    fake.leaves.lock().unwrap().push(leaf.der().to_vec());

    let mut chain = vec![fake.authority.inter_pem.clone()];
    if fake.always_send_root.load(Ordering::SeqCst) || body["remove_roots_from_chain"] != true {
        chain.push(fake.authority.root_pem.clone());
    }
    let extra = fake.extra_chain.load(Ordering::SeqCst);
    chain.extend(fake.authority.extras_pem.iter().take(extra).cloned());
    let ca_chain = if fake.null_chain.load(Ordering::SeqCst) {
        Value::Null
    } else {
        json!(chain)
    };
    Json(json!({"data": {
        "certificate": leaf.pem(),
        "ca_chain": ca_chain,
        "issuing_ca": fake.authority.inter_pem,
        "serial_number": "ignored-by-the-adapter",
    }}))
    .into_response()
}

struct Harness {
    fake: Shared,
    clock: FixedClock,
    adapter: Arc<OpenBaoClientCertificates>,
    token_file: PathBuf,
    address: String,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.token_file);
    }
}

fn unique_token_file() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "orch-sa-token-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ))
}

async fn start() -> Harness {
    start_with(|_| {}).await
}

async fn start_with(tweak: impl FnOnce(&mut OpenBaoPkiConfig)) -> Harness {
    let now: Timestamp = START.parse().unwrap();
    let clock = FixedClock::new(now);
    let fake = Arc::new(Fake {
        authority: authority(now),
        clock: clock.clone(),
        mode: Mutex::new(Mode::Ok),
        logins: AtomicUsize::new(0),
        signs: AtomicUsize::new(0),
        serial: AtomicU64::new(0),
        sign_delay_ms: AtomicU64::new(0),
        always_send_root: AtomicBool::new(false),
        extra_chain: AtomicUsize::new(0),
        null_chain: AtomicBool::new(false),
        jwts: Mutex::new(Vec::new()),
        sign_bodies: Mutex::new(Vec::new()),
        csr_keys: Mutex::new(Vec::new()),
        csr_names: Mutex::new(Vec::new()),
        leaves: Mutex::new(Vec::new()),
    });
    let app = Router::new()
        .route("/v1/auth/kubernetes/login", post(login))
        .route(
            "/v1/pki-workload/sign/orchestrator-platform-client",
            post(sign),
        )
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let token_file = unique_token_file();
    std::fs::write(&token_file, format!("{SA_JWT}\n")).unwrap();
    let mut config = OpenBaoPkiConfig::new(address.clone(), AUTH_ROLE, PKI_MOUNT, PKI_ROLE, SPIFFE);
    config.service_account_token_file = token_file.clone();
    tweak(&mut config);
    let adapter = OpenBaoClientCertificates::with_clock(config, Arc::new(clock.clone())).unwrap();
    Harness {
        fake,
        clock,
        adapter: Arc::new(adapter),
        token_file,
        address,
    }
}

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// The public point of a PKCS#8 key, as the CSR carries it.
fn public_point(pkcs8: &[u8]) -> Vec<u8> {
    KeyPair::try_from(pkcs8).unwrap().public_key_raw().to_vec()
}

#[tokio::test]
async fn a_leaf_is_issued_for_our_key_with_our_id_and_the_chain_without_the_root() {
    let h = start().await;
    let cert = h.adapter.current().await.unwrap();

    // One login with the pod's token, one signing, nothing more.
    assert_eq!(h.fake.logins(), 1);
    assert_eq!(h.fake.signs(), 1);
    assert_eq!(*h.fake.jwts.lock().unwrap(), [SA_JWT]);

    // The request: no names and no subject in the CSR; the id, the life and the format in the body.
    assert_eq!(*h.fake.csr_names.lock().unwrap(), [(0, 0)]);
    let body = h.fake.sign_bodies.lock().unwrap()[0].clone();
    assert_eq!(body["uri_sans"], SPIFFE);
    assert_eq!(body["ttl"], "86400s");
    assert_eq!(body["format"], "pem");
    assert_eq!(body["remove_roots_from_chain"], true);
    assert!(
        body["csr"]
            .as_str()
            .unwrap()
            .starts_with("-----BEGIN CERTIFICATE REQUEST-----")
    );

    // The key is the one the CSR was made with.
    assert_eq!(
        public_point(cert.expose_key_pkcs8()),
        h.fake.csr_keys.lock().unwrap()[0]
    );
    // The chain is the leaf and its intermediate, and no root.
    let leaf = h.fake.leaves.lock().unwrap()[0].clone();
    assert_eq!(cert.chain_der(), [leaf, h.fake.authority.inter_der.clone()]);
    assert!(!cert.chain_der().contains(&h.fake.authority.root_der));
    // The serial and the end of validity are the leaf's own.
    assert!(cert.serial().ends_with("03:e9"), "{}", cert.serial());
    assert_eq!(
        cert.not_after(),
        h.clock.now().checked_add(secs(86_400)).unwrap()
    );
}

#[tokio::test]
async fn a_root_that_is_sent_anyway_is_not_presented() {
    let h = start().await;
    h.fake.always_send_root.store(true, Ordering::SeqCst);
    let cert = h.adapter.current().await.unwrap();
    assert_eq!(cert.chain_der().len(), 2);
    assert!(!cert.chain_der().contains(&h.fake.authority.root_der));
}

#[tokio::test]
async fn nothing_is_asked_again_outside_the_renewal_margin() {
    let h = start().await;
    let first = h.adapter.current().await.unwrap();
    // The leaf lives 24 h and is renewed with 8 h left: due at 16 h.
    h.clock.advance(secs(16 * 3600 - 1));
    let again = h.adapter.current().await.unwrap();
    assert_eq!(again.serial(), first.serial());
    assert_eq!(h.fake.logins(), 1);
    assert_eq!(h.fake.signs(), 1);
}

#[tokio::test]
async fn it_is_renewed_inside_the_margin_with_a_new_serial_and_a_new_key() {
    let h = start().await;
    let first = h.adapter.current().await.unwrap();
    h.clock.advance(secs(16 * 3600));
    let second = h.adapter.current().await.unwrap();

    assert_ne!(second.serial(), first.serial());
    assert_eq!(h.fake.signs(), 2);
    // A fresh key each time: the two requests carry different keys, and each leaf is for its own.
    let keys = h.fake.csr_keys.lock().unwrap().clone();
    assert_ne!(keys[0], keys[1]);
    assert_eq!(public_point(second.expose_key_pkcs8()), keys[1]);
    assert_ne!(first.expose_key_pkcs8(), second.expose_key_pkcs8());
    // And the new leaf lives from now.
    assert_eq!(
        second.not_after(),
        h.clock.now().checked_add(secs(86_400)).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_inside_the_margin_makes_one_signing() {
    let h = start().await;
    let first = h.adapter.current().await.unwrap();
    h.clock.advance(secs(16 * 3600));
    // Slow enough for the eight callers to overlap.
    h.fake.sign_delay_ms.store(200, Ordering::SeqCst);

    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let adapter = h.adapter.clone();
            tokio::spawn(async move { adapter.current().await })
        })
        .collect();
    let mut serials = Vec::new();
    for task in tasks {
        serials.push(task.await.unwrap().unwrap().serial().to_owned());
    }

    assert_eq!(h.fake.signs(), 2, "one signing for the whole burst");
    assert_eq!(h.fake.logins(), 2);
    assert!(serials.iter().all(|s| *s == serials[0]), "{serials:?}");
    assert_ne!(serials[0], first.serial());
}

#[tokio::test]
async fn past_the_end_with_the_pki_down_there_is_no_identity() {
    let h = start().await;
    h.adapter.current().await.unwrap();
    h.fake.set(Mode::SignDown);
    h.clock.advance(secs(24 * 3600 + 1));
    let error = h.adapter.current().await.unwrap_err();
    assert_eq!(error, PlatformCredentialsError::Unavailable);
    assert_eq!(error.class(), ErrorClass::Transient);
    // The old key is not served again, even when asked again.
    assert_eq!(
        h.adapter.current().await.unwrap_err(),
        PlatformCredentialsError::Unavailable
    );
}

#[tokio::test]
async fn inside_the_margin_the_old_leaf_is_served_while_the_pki_is_down_and_not_hammered() {
    let h = start().await;
    let first = h.adapter.current().await.unwrap();
    h.fake.set(Mode::SignDown);
    h.clock.advance(secs(17 * 3600));

    let served = h.adapter.current().await.unwrap();
    assert_eq!(served.serial(), first.serial());
    assert_eq!(h.fake.logins(), 2, "one renewal was tried");
    // Asked again at once: the old leaf, and no new try.
    h.adapter.current().await.unwrap();
    assert_eq!(h.fake.logins(), 2);
    // After the pause it is tried again, and when the PKI is back it is renewed.
    h.clock.advance(secs(31));
    h.fake.set(Mode::Ok);
    let renewed = h.adapter.current().await.unwrap();
    assert_ne!(renewed.serial(), first.serial());
    assert_eq!(h.fake.logins(), 3);
}

#[tokio::test]
async fn a_refused_login_or_signing_is_refused_and_nothing_is_kept() {
    let h = start().await;
    for (mode, logins, signs) in [(Mode::LoginRefused, 1, 0), (Mode::SignRefused, 2, 1)] {
        // Past the pause after a refusal, so that the next one is really asked.
        h.clock.advance(secs(61));
        h.fake.set(mode);
        let error = h.adapter.current().await.unwrap_err();
        assert_eq!(error, PlatformCredentialsError::Refused, "{mode:?}");
        assert_eq!(error.class(), ErrorClass::Unauthenticated);
        assert_eq!(h.fake.logins(), logins, "{mode:?}");
        assert_eq!(h.fake.signs(), signs, "{mode:?}");
    }
    // The refusal did not poison the adapter: when OpenBao grants again, it issues.
    h.clock.advance(secs(61));
    h.fake.set(Mode::Ok);
    h.adapter.current().await.unwrap();
}

#[tokio::test]
async fn a_failing_or_unreadable_openbao_is_unavailable() {
    let h = start().await;
    for mode in [Mode::LoginDown, Mode::SignDown, Mode::Garbage] {
        // Past the pause after a failure, so that the next one is really asked.
        h.clock.advance(secs(31));
        h.fake.set(mode);
        assert_eq!(
            h.adapter.current().await.unwrap_err(),
            PlatformCredentialsError::Unavailable,
            "{mode:?}"
        );
    }

    // Nothing listens.
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", closed.local_addr().unwrap());
    drop(closed);
    // With the token file there, so that it is the network that fails and not the file.
    let mut config = OpenBaoPkiConfig::new(address, AUTH_ROLE, PKI_MOUNT, PKI_ROLE, SPIFFE);
    config.service_account_token_file = h.token_file.clone();
    let adapter = OpenBaoClientCertificates::new(config).unwrap();
    assert_eq!(
        adapter.current().await.unwrap_err(),
        PlatformCredentialsError::Unavailable
    );
}

#[tokio::test]
async fn a_missing_service_account_token_is_unavailable_and_asks_nothing() {
    let h = start().await;
    std::fs::remove_file(&h.token_file).unwrap();
    assert_eq!(
        h.adapter.current().await.unwrap_err(),
        PlatformCredentialsError::Unavailable
    );
    assert_eq!(h.fake.logins(), 0);
}

#[tokio::test]
async fn every_leaf_that_is_not_ours_is_refused_as_unusable() {
    let h = start().await;
    for mis in [
        Mis::ServerAuthToo,
        Mis::ServerAuthOnly,
        Mis::NoEku,
        Mis::TwoUris,
        Mis::OtherUri,
        Mis::ExtraDnsName,
        Mis::NoName,
        Mis::ForeignKey,
        Mis::Expired,
        Mis::ShortLived,
        Mis::AnyEku,
        Mis::CaLeaf,
        Mis::KeyCertSign,
        Mis::SelfSigned,
    ] {
        // Past the pause after the last failure, so that this one is really asked.
        h.clock.advance(secs(31));
        let signed_before = h.fake.signs();
        h.fake.set(Mode::Mis(mis));
        let error = h.adapter.current().await.unwrap_err();
        assert_eq!(error, PlatformCredentialsError::Unavailable, "{mis:?}");
        assert_eq!(
            h.fake.signs(),
            signed_before + 1,
            "{mis:?}: it was asked for and refused, not skipped"
        );
    }
    // And the right leaf is still accepted by the same adapter.
    h.clock.advance(secs(31));
    h.fake.set(Mode::Ok);
    h.adapter.current().await.unwrap();
}

#[tokio::test]
async fn the_token_file_is_read_again_at_each_login() {
    let h = start().await;
    h.adapter.current().await.unwrap();
    std::fs::write(&h.token_file, "SENTINEL-SA-JWT-ROTATED").unwrap();
    h.clock.advance(secs(16 * 3600));
    h.adapter.current().await.unwrap();
    assert_eq!(
        *h.fake.jwts.lock().unwrap(),
        [SA_JWT, "SENTINEL-SA-JWT-ROTATED"]
    );
}

#[tokio::test]
async fn nothing_secret_is_in_a_debug_an_error_or_a_log() {
    use std::io::Write;
    use tracing_subscriber::filter::Targets;
    use tracing_subscriber::fmt::MakeWriter;
    use tracing_subscriber::layer::SubscriberExt;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Capture;
        fn make_writer(&'a self) -> Capture {
            self.clone()
        }
    }
    let capture = Capture::default();
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(capture.clone())
                .with_ansi(false),
        )
        .with(
            Targets::new().with_target("orch_platform_credentials_openbao", tracing::Level::TRACE),
        );
    let _guard = tracing::subscriber::set_default(subscriber);

    let h = start().await;
    let cert = h.adapter.current().await.unwrap();
    // A renewal that fails with a good leaf in hand logs a warning.
    h.fake.set(Mode::Mis(Mis::ServerAuthToo));
    h.clock.advance(secs(17 * 3600));
    h.adapter.current().await.unwrap();

    let mut shown = vec![
        format!("{:?}", h.adapter),
        format!(
            "{:?}",
            OpenBaoPkiConfig::new(h.address.clone(), AUTH_ROLE, PKI_MOUNT, PKI_ROLE, SPIFFE)
        ),
        format!("{cert:?}"),
        format!("{:?}", h.adapter.current().await.unwrap()),
    ];
    for failure in [
        PlatformCredentialsError::Unavailable,
        PlatformCredentialsError::Refused,
    ] {
        shown.push(failure.to_string());
        shown.push(format!("{failure:?}"));
    }
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("the platform client certificate is issued"),
        "the log under test is empty: {logs}"
    );
    assert!(
        logs.contains("could not be renewed"),
        "the failure was not logged: {logs}"
    );
    shown.push(logs);
    // Six bytes of the key past the constant PKCS#8 header, as a `Debug` of a slice would print them.
    let key_prefix = format!("{:?}", &cert.expose_key_pkcs8()[40..46]);
    for text in shown {
        for forbidden in [
            "BEGIN",
            "PRIVATE",
            SA_JWT,
            CLIENT_TOKEN,
            "SENTINEL",
            key_prefix.trim_matches(['[', ']']),
        ] {
            assert!(!text.contains(forbidden), "{forbidden} is in: {text}");
        }
    }
}

#[tokio::test]
async fn the_settings_are_checked_when_the_adapter_is_built() {
    let good = || {
        OpenBaoPkiConfig::new(
            "https://bao.internal:8200",
            AUTH_ROLE,
            PKI_MOUNT,
            PKI_ROLE,
            SPIFFE,
        )
    };
    let build = |mut config: OpenBaoPkiConfig, tweak: &dyn Fn(&mut OpenBaoPkiConfig)| {
        tweak(&mut config);
        OpenBaoClientCertificates::new(config).err()
    };
    assert!(OpenBaoClientCertificates::new(good()).is_ok());
    assert!(matches!(
        build(good(), &|c| c.address = "bao.internal".into()),
        Some(BuildError::BadAddress)
    ));
    assert!(matches!(
        build(good(), &|c| c.address = "https://u:p@bao.internal".into()),
        Some(BuildError::BadAddress)
    ));
    assert!(matches!(
        build(good(), &|c| c.pki_mount = "a/../b".into()),
        Some(BuildError::BadName(_))
    ));
    assert!(matches!(
        build(good(), &|c| c.pki_role = "a/b".into()),
        Some(BuildError::BadName(_))
    ));
    assert!(matches!(
        build(good(), &|c| c.auth_role = String::new()),
        Some(BuildError::BadName(_))
    ));
    for bad in [
        "https://example.org/x",
        "spiffe://example.org",
        "spiffe://Example.org/x",
        "spiffe://example.org/x?y=1",
        "spiffe://example.org:8443/x",
    ] {
        assert!(
            matches!(
                build(good(), &|c| c.spiffe_id = bad.into()),
                Some(BuildError::BadSpiffeId)
            ),
            "{bad}"
        );
    }
    assert!(matches!(
        build(good(), &|c| c.renewal_margin = c.ttl),
        Some(BuildError::BadLifetime)
    ));
    assert!(matches!(
        build(good(), &|c| c.ttl = secs(30)),
        Some(BuildError::BadLifetime)
    ));
    assert!(matches!(
        build(good(), &|c| c.ca_pem = Some("not pem".into())),
        Some(BuildError::BadCa)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_with_no_leaf_asks_openbao_once_and_every_caller_gets_the_same_error() {
    for (mode, error, pause) in [
        (Mode::SignDown, PlatformCredentialsError::Unavailable, 30),
        (Mode::LoginRefused, PlatformCredentialsError::Refused, 60),
    ] {
        let h = start().await;
        h.fake.set(mode);
        // Slow enough for the eight callers to queue behind the first.
        h.fake.sign_delay_ms.store(200, Ordering::SeqCst);
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let adapter = h.adapter.clone();
                tokio::spawn(async move { adapter.current().await })
            })
            .collect();
        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap_err(), error, "{mode:?}");
        }
        assert_eq!(
            h.fake.logins(),
            1,
            "{mode:?}: one login for the whole burst"
        );
        assert!(h.fake.signs() <= 1, "{mode:?}: one signing at most");

        // Nothing more is asked while the pause lasts, and it is asked again when it is over.
        h.clock.advance(secs(pause - 1));
        assert_eq!(h.adapter.current().await.unwrap_err(), error);
        assert_eq!(h.fake.logins(), 1, "{mode:?}: inside the pause");
        h.clock.advance(secs(2));
        assert_eq!(h.adapter.current().await.unwrap_err(), error);
        assert_eq!(h.fake.logins(), 2, "{mode:?}: after the pause");
        // And the PKI coming back is seen at the next try.
        h.fake.set(Mode::Ok);
        h.clock.advance(secs(61));
        h.adapter.current().await.unwrap();
    }
}

#[tokio::test]
async fn a_back_off_that_runs_past_the_end_of_the_leaf_is_an_error_without_a_new_try() {
    let h = start().await;
    h.adapter.current().await.unwrap();
    h.fake.set(Mode::SignDown);
    // Ten seconds before the end: due, tried, failed; the old leaf is still served.
    h.clock.advance(secs(24 * 3600 - 10));
    h.adapter.current().await.unwrap();
    assert_eq!(h.fake.logins(), 2);
    // The leaf lapses inside the pause: no identity, the last error, and OpenBao is not asked.
    h.clock.advance(secs(15));
    assert_eq!(
        h.adapter.current().await.unwrap_err(),
        PlatformCredentialsError::Unavailable
    );
    assert_eq!(h.fake.logins(), 2);
    // Past the pause it is asked again.
    h.clock.advance(secs(30));
    assert_eq!(
        h.adapter.current().await.unwrap_err(),
        PlatformCredentialsError::Unavailable
    );
    assert_eq!(h.fake.logins(), 3);
}

#[tokio::test]
async fn a_chain_of_at_most_eight_is_presented_and_a_longer_one_is_refused() {
    let h = start().await;
    // The leaf, the intermediate and six more: eight.
    h.fake.extra_chain.store(6, Ordering::SeqCst);
    assert_eq!(h.adapter.current().await.unwrap().chain_der().len(), 8);
    // One more makes nine.
    h.clock.advance(secs(24 * 3600 + 1));
    h.fake.extra_chain.store(7, Ordering::SeqCst);
    assert_eq!(
        h.adapter.current().await.unwrap_err(),
        PlatformCredentialsError::Unavailable
    );
}

#[tokio::test]
async fn a_null_chain_falls_back_to_the_issuing_ca() {
    let h = start().await;
    h.fake.null_chain.store(true, Ordering::SeqCst);
    let cert = h.adapter.current().await.unwrap();
    assert_eq!(
        cert.chain_der(),
        [
            h.fake.leaves.lock().unwrap()[0].clone(),
            h.fake.authority.inter_der.clone()
        ]
    );
}
