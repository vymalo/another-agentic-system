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

/// A client's DPoP key (RFC 9449): the key pair a browser keeps, and the proofs it signs, for tests.
/// Made fresh each time, so two clients are two keys.
pub enum DpopKey {
    /// ECDSA P-256 (`ES256`).
    Es256(EcdsaKeyPair),
    /// Ed25519 (`EdDSA`).
    EdDsa(Ed25519KeyPair),
}

impl DpopKey {
    /// A new P-256 key.
    pub fn es256() -> Self {
        DpopKey::Es256(EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap())
    }

    /// A new Ed25519 key.
    pub fn ed25519() -> Self {
        DpopKey::EdDsa(Ed25519KeyPair::generate().unwrap())
    }

    /// The `alg` this key signs with.
    pub fn alg(&self) -> &'static str {
        match self {
            DpopKey::Es256(_) => "ES256",
            DpopKey::EdDsa(_) => "EdDSA",
        }
    }

    /// The public key as a JWK: only `kty`, `crv` and the public coordinates.
    pub fn jwk(&self) -> Value {
        match self {
            DpopKey::Es256(key) => {
                let point = key.public_key().as_ref();
                json!({"kty": "EC", "crv": "P-256", "x": b64(&point[1..33]), "y": b64(&point[33..65])})
            }
            DpopKey::EdDsa(key) => {
                json!({"kty": "OKP", "crv": "Ed25519", "x": b64(key.public_key().as_ref())})
            }
        }
    }

    /// The RFC 7638 SHA-256 thumbprint of the public key, the value of a bound token's `cnf.jkt`,
    /// computed here from the canonical JSON and not by the code under test.
    pub fn jkt(&self) -> String {
        let jwk = self.jwk();
        let text = |name: &str| jwk[name].as_str().unwrap().to_owned();
        let canonical = match self {
            DpopKey::Es256(_) => format!(
                r#"{{"crv":"P-256","kty":"EC","x":"{}","y":"{}"}}"#,
                text("x"),
                text("y")
            ),
            DpopKey::EdDsa(_) => {
                format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{}"}}"#, text("x"))
            }
        };
        b64(aws_lc_rs::digest::digest(
            &aws_lc_rs::digest::SHA256,
            canonical.as_bytes(),
        ))
    }

    /// `ath`: the base64url SHA-256 of an access token.
    pub fn ath(access_token: &str) -> String {
        b64(aws_lc_rs::digest::digest(
            &aws_lc_rs::digest::SHA256,
            access_token.as_bytes(),
        ))
    }

    /// The header of a good proof: `typ`, this key's `alg` and public `jwk`.
    pub fn header(&self) -> Map<String, Value> {
        json!({"typ": "dpop+jwt", "alg": self.alg(), "jwk": self.jwk()})
            .as_object()
            .unwrap()
            .clone()
    }

    /// The claims of a good proof of `method` to `htu` with `access_token`, made now, with a
    /// `jti` nobody used.
    pub fn claims(method: &str, htu: &str, access_token: &str) -> Map<String, Value> {
        let mut jti = [0u8; 16];
        aws_lc_rs::rand::fill(&mut jti).unwrap();
        json!({
            "jti": b64(jti), "htm": method, "htu": htu, "iat": now(),
            "ath": Self::ath(access_token),
        })
        .as_object()
        .unwrap()
        .clone()
    }

    /// A good proof: [`DpopKey::header`] and [`DpopKey::claims`], signed.
    pub fn proof(&self, method: &str, htu: &str, access_token: &str) -> String {
        self.sign(&self.header(), &Self::claims(method, htu, access_token))
    }

    /// A proof with exactly this `header` and these `claims`, signed with this key whatever the
    /// header says (a case that needs a bad header or bad claims edits the good ones).
    pub fn sign(&self, header: &Map<String, Value>, claims: &Map<String, Value>) -> String {
        let signing_input = format!(
            "{}.{}",
            b64(serde_json::to_vec(header).unwrap()),
            b64(serde_json::to_vec(claims).unwrap())
        );
        let signature = match self {
            DpopKey::Es256(key) => key
                .sign(&SystemRandom::new(), signing_input.as_bytes())
                .unwrap()
                .as_ref()
                .to_vec(),
            DpopKey::EdDsa(key) => key.sign(signing_input.as_bytes()).as_ref().to_vec(),
        };
        format!("{signing_input}.{}", b64(signature))
    }
}

impl TestIdp {
    /// The claims of a good token for `audience` and `email` that is bound to `key` (`cnf.jkt`,
    /// RFC 9449 §6).
    pub fn bound_claims(&self, audience: &str, email: &str, key: &DpopKey) -> Map<String, Value> {
        let mut claims = self.claims(audience, email);
        claims.insert("cnf".to_owned(), json!({"jkt": key.jkt()}));
        claims
    }

    /// A good token (RS256) for `audience` and `email` bound to `key`.
    pub fn bound_token(&self, audience: &str, email: &str, key: &DpopKey) -> String {
        self.mint(
            &self.bound_claims(audience, email, key),
            Signing::Published(Alg::Rs256),
        )
    }
}
