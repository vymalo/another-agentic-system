//! A token issuer on a local port, for tests (feature `testkit`): its discovery document and its
//! JWKS over real HTTP, keys of every kind the authenticator reads, and tokens signed as a case
//! says, the ones that must be refused included.
//!
//! The keys (RSA 2048, P-256, Ed25519, twice: one set the issuer publishes, one it does not
//! until [`TestIdp::publish_unpublished_keys`]) are made once per process. Each [`TestIdp`] has
//! its own port, its own publication state and its own counters, so tests run side by side.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::{KeyPair as RsaKeyPair, KeySize, PublicKeyComponents};
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair as _, RSA_PKCS1_SHA256,
    RSA_PKCS1_SHA384,
};
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_ports::testkit::bearer::{Alg, Signing};
use serde_json::{Map, Value, json};
use tokio::task::JoinHandle;

fn b64(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// One generation of keys: an RSA key (RS256 and RS384), a P-256 key and an Ed25519 key.
struct Generation {
    name: &'static str,
    rsa: RsaKeyPair,
    ec: EcdsaKeyPair,
    ed: Ed25519KeyPair,
}

impl Generation {
    fn make(name: &'static str) -> Self {
        Generation {
            name,
            rsa: RsaKeyPair::generate(KeySize::Rsa2048).unwrap(),
            ec: EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap(),
            ed: Ed25519KeyPair::generate().unwrap(),
        }
    }

    fn kid(&self, alg: Alg) -> String {
        let kind = match alg {
            Alg::Rs256 | Alg::Rs384 => "rsa",
            Alg::Es256 => "ec",
            Alg::EdDsa => "ed",
        };
        format!("{}-{kind}", self.name)
    }

    fn jwks(&self) -> Vec<Value> {
        let rsa = PublicKeyComponents::<Vec<u8>>::from(self.rsa.public_key());
        let ec = self.ec.public_key().as_ref().to_vec();
        assert_eq!(ec.len(), 65, "an uncompressed P-256 point");
        vec![
            json!({"kty": "RSA", "use": "sig", "kid": self.kid(Alg::Rs256), "n": b64(&rsa.n), "e": b64(&rsa.e)}),
            json!({"kty": "EC", "use": "sig", "crv": "P-256", "kid": self.kid(Alg::Es256),
                   "x": b64(&ec[1..33]), "y": b64(&ec[33..65])}),
            json!({"kty": "OKP", "use": "sig", "crv": "Ed25519", "kid": self.kid(Alg::EdDsa),
                   "x": b64(self.ed.public_key().as_ref())}),
        ]
    }

    fn sign(&self, alg: Alg, message: &[u8]) -> Vec<u8> {
        match alg {
            Alg::Rs256 | Alg::Rs384 => {
                let padding: &'static dyn aws_lc_rs::signature::RsaEncoding = if alg == Alg::Rs256 {
                    &RSA_PKCS1_SHA256
                } else {
                    &RSA_PKCS1_SHA384
                };
                let mut signature = vec![0; self.rsa.public_modulus_len()];
                self.rsa
                    .sign(padding, &SystemRandom::new(), message, &mut signature)
                    .unwrap();
                signature
            }
            Alg::Es256 => self
                .ec
                .sign(&SystemRandom::new(), message)
                .unwrap()
                .as_ref()
                .to_vec(),
            Alg::EdDsa => self.ed.sign(message).as_ref().to_vec(),
        }
    }
}

fn generations() -> &'static (Generation, Generation) {
    static KEYS: OnceLock<(Generation, Generation)> = OnceLock::new();
    KEYS.get_or_init(|| (Generation::make("pub"), Generation::make("unpub")))
}

/// What signs a token's signing input.
type Signer<'a> = Box<dyn Fn(&[u8]) -> Vec<u8> + 'a>;

fn alg_name(alg: Alg) -> &'static str {
    match alg {
        Alg::Rs256 => "RS256",
        Alg::Rs384 => "RS384",
        Alg::Es256 => "ES256",
        Alg::EdDsa => "EdDSA",
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

#[derive(Default)]
struct Shared {
    issuer: String,
    unpublished_published: AtomicBool,
    jwks_down: AtomicBool,
    discovery_down: AtomicBool,
    jwks_fetches: AtomicUsize,
    discovery_fetches: AtomicUsize,
}

async fn discovery(State(shared): State<Arc<Shared>>) -> Response {
    shared.discovery_fetches.fetch_add(1, Ordering::SeqCst);
    if shared.discovery_down.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    axum::Json(json!({"issuer": shared.issuer, "jwks_uri": format!("{}/jwks", shared.issuer)}))
        .into_response()
}

async fn jwks(State(shared): State<Arc<Shared>>) -> Response {
    shared.jwks_fetches.fetch_add(1, Ordering::SeqCst);
    if shared.jwks_down.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let (published, unpublished) = generations();
    let mut keys = published.jwks();
    if shared.unpublished_published.load(Ordering::SeqCst) {
        keys.extend(unpublished.jwks());
    }
    axum::Json(json!({ "keys": keys })).into_response()
}

/// A token issuer on `127.0.0.1`, stopped when dropped.
pub struct TestIdp {
    shared: Arc<Shared>,
    server: JoinHandle<()>,
}

impl Drop for TestIdp {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl TestIdp {
    /// Starts the issuer on a free port.
    pub async fn start() -> Self {
        // The keys are made before the first request needs them.
        let _ = generations();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let shared = Arc::new(Shared {
            issuer,
            ..Shared::default()
        });
        let router = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .with_state(Arc::clone(&shared));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        TestIdp { shared, server }
    }

    /// The issuer, as `iss` must say it: `http://127.0.0.1:<port>`.
    pub fn issuer(&self) -> String {
        self.shared.issuer.clone()
    }

    /// The JWKS URL, for a configuration that names it.
    pub fn jwks_url(&self) -> String {
        format!("{}/jwks", self.shared.issuer)
    }

    /// Makes the JWKS answer 503 (`true`) or answer again.
    pub fn set_jwks_down(&self, down: bool) {
        self.shared.jwks_down.store(down, Ordering::SeqCst);
    }

    /// Makes the discovery document answer 503 (`true`) or answer again.
    pub fn set_discovery_down(&self, down: bool) {
        self.shared.discovery_down.store(down, Ordering::SeqCst);
    }

    /// Requests of the JWKS so far.
    pub fn jwks_fetches(&self) -> usize {
        self.shared.jwks_fetches.load(Ordering::SeqCst)
    }

    /// Requests of the discovery document so far.
    pub fn discovery_fetches(&self) -> usize {
        self.shared.discovery_fetches.load(Ordering::SeqCst)
    }

    /// Publishes the second set of keys: the issuer rotates.
    pub fn publish_unpublished_keys(&self) {
        self.shared
            .unpublished_published
            .store(true, Ordering::SeqCst);
    }

    /// The claims of a good token for `audience` and `email`: this issuer, valid now for five
    /// minutes, the e-mail verified.
    pub fn claims(&self, audience: &str, email: &str) -> Map<String, Value> {
        let now = now();
        let claims = json!({
            "iss": self.issuer(), "aud": audience, "sub": format!("sub-{email}"),
            "iat": now, "exp": now + 300, "email": email, "email_verified": true,
        });
        claims.as_object().unwrap().clone()
    }

    /// A good token (RS256) for `audience` and `email`.
    pub fn token(&self, audience: &str, email: &str) -> String {
        self.mint(
            &self.claims(audience, email),
            Signing::Published(Alg::Rs256),
        )
    }

    /// A token with exactly `claims`, signed as `signing` says.
    pub fn mint(&self, claims: &Map<String, Value>, signing: Signing) -> String {
        let (published, unpublished) = generations();
        let (header, signer): (Value, Signer<'_>) = match signing {
            Signing::Published(alg) => (
                json!({"alg": alg_name(alg), "typ": "JWT", "kid": published.kid(alg)}),
                Box::new(move |m| published.sign(alg, m)),
            ),
            Signing::Unpublished(alg) => (
                json!({"alg": alg_name(alg), "typ": "JWT", "kid": unpublished.kid(alg)}),
                Box::new(move |m| unpublished.sign(alg, m)),
            ),
            Signing::NoneAlg => (
                json!({"alg": "none", "typ": "JWT"}),
                Box::new(|_| Vec::new()),
            ),
            Signing::Hs256WithPublicKey => {
                let secret = published.rsa.public_key().as_ref().to_vec();
                (
                    json!({"alg": "HS256", "typ": "JWT", "kid": published.kid(Alg::Rs256)}),
                    Box::new(move |m| {
                        let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, &secret);
                        aws_lc_rs::hmac::sign(&key, m).as_ref().to_vec()
                    }),
                )
            }
        };
        let signing_input = format!(
            "{}.{}",
            b64(serde_json::to_vec(&header).unwrap()),
            b64(serde_json::to_vec(claims).unwrap())
        );
        let signature = signer(signing_input.as_bytes());
        format!("{signing_input}.{}", b64(signature))
    }
}
