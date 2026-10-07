//! Conformance cases for an [`Authenticator`] that reads **signed bearer tokens** against the
//! keys of an issuer it fetches (ADR 0033): what the edge relies on from any such
//! implementation, stated once and run against the JWT authenticator over real HTTP, against a
//! local issuer whose keys the cases rotate and take down.
//!
//! An implementation supplies a [`TokenFixture`]: the authenticator, the issuer's name and the
//! audience it is configured for, the claims it reads, and an issuer that mints tokens as the cases
//! say (right, expired, signed with an algorithm that must be refused, signed by a key nobody
//! published) and publishes and withdraws its keys. Each case is `async fn(fixture)` and gives up
//! after 10 seconds.
//!
//! The policy the cases pin, from [ADR 0033](../../../../docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md):
//! the algorithms are RS256, RS384, ES256 and EdDSA and nothing else (`none` and HS256 are
//! refused, whatever key they claim); `iss` matches exactly; the configured audience is one of
//! `aud`; `exp` and `iat` are present; time has 60 seconds of leeway; a key that is not published
//! is not a key (and an unknown `kid` makes the authenticator ask the issuer again at most once
//! per interval); `email_verified: false` is refused; the user is the configured claim; the issuer
//! being down is `Unavailable`, not a refusal.
//!
//! What is deliberately not asserted: the wording of any detail, and how the keys are fetched.

use std::future::Future;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use orch_core::{Classify, ErrorClass};
use serde_json::{Map, Value, json};

use crate::{AuthError, Authenticator, CredentialKind, Credentials, Principal, Role};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(10);

/// A signature algorithm the authenticator must accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alg {
    /// RSASSA-PKCS1-v1_5 with SHA-256.
    Rs256,
    /// RSASSA-PKCS1-v1_5 with SHA-384.
    Rs384,
    /// ECDSA over P-256 with SHA-256.
    Es256,
    /// EdDSA over Ed25519.
    EdDsa,
}

/// Every algorithm the authenticator must accept.
pub const ALGORITHMS: [Alg; 4] = [Alg::Rs256, Alg::Rs384, Alg::Es256, Alg::EdDsa];

/// How the issuer signs a token it mints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signing {
    /// With a key the issuer publishes, of this algorithm. The header names its `kid`.
    Published(Alg),
    /// With a key the issuer has **not** published (yet): the header names a `kid` of its own.
    Unpublished(Alg),
    /// With `alg: none` and an empty signature.
    NoneAlg,
    /// With `alg: HS256`, the bytes of a published public key being the secret (the algorithm
    /// confusion attack).
    Hs256WithPublicKey,
}

/// An authenticator of signed bearer tokens under test, with an issuer behind it that the cases
/// drive.
pub trait TokenFixture: Send + Sync + 'static {
    /// The authenticator under test.
    type Auth: Authenticator;

    /// The authenticator.
    fn auth(&self) -> &Self::Auth;

    /// The issuer it is configured for (`iss` must match it exactly).
    fn issuer(&self) -> String;

    /// An audience it is configured for.
    fn audience(&self) -> String;

    /// The claim whose value is the user (`auth.jwt.userClaim`).
    fn user_claim(&self) -> String;

    /// The dotted path of the claim that holds the roles (`auth.jwt.rolesClaim`), for example
    /// `realm_access.roles`.
    fn roles_claim(&self) -> String;

    /// A token with `claims`, signed as `signing` says.
    fn mint(&self, claims: &Map<String, Value>, signing: Signing) -> String;

    /// Takes the issuer's key set down (`true`: a fetch fails) or brings it back.
    fn set_jwks_down(&self, down: bool);

    /// How many times the authenticator has fetched the key set.
    fn jwks_fetches(&self) -> usize;

    /// Publishes the keys that [`Signing::Unpublished`] signs with: the issuer rotates.
    fn publish_unpublished_keys(&self);

    /// Waits until the authenticator may fetch the key set again for an unknown `kid`, and again
    /// after a failure (the fixture configures them short; a case never waits on the real ones).
    fn wait_out_the_refetch_interval(&self) -> impl Future<Output = ()> + Send;
}

async fn within<T>(case: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, case)
        .await
        .expect("the case timed out")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after 1970")
        .as_secs()
}

/// The text of the e-mail of every case's token, and the user it names.
const EMAIL: &str = "Alice@Example.com";
const USER: &str = "alice@example.com";

/// Sets `value` at the dotted `path` of `claims`, making the objects on the way.
fn set_path(claims: &mut Map<String, Value>, path: &str, value: Value) {
    match path.split_once('.') {
        None => {
            claims.insert(path.to_owned(), value);
        }
        Some((head, rest)) => {
            let entry = claims
                .entry(head.to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(inner) = entry {
                set_path(inner, rest, value);
            }
        }
    }
}

/// The claims of a good token: this issuer, this audience, valid now for five minutes, Alice.
fn good_claims<F: TokenFixture>(f: &F) -> Map<String, Value> {
    let now = now();
    let mut claims = Map::new();
    claims.insert("iss".into(), json!(f.issuer()));
    claims.insert("aud".into(), json!(f.audience()));
    claims.insert("sub".into(), json!("subject-1"));
    claims.insert("iat".into(), json!(now));
    claims.insert("exp".into(), json!(now + 300));
    claims.insert("email".into(), json!(EMAIL));
    claims.insert("email_verified".into(), json!(true));
    claims.insert("name".into(), json!("Alice"));
    claims.insert(f.user_claim(), json!(EMAIL));
    set_path(&mut claims, &f.roles_claim(), json!(["user", "admin"]));
    claims
}

fn bearer(token: &str) -> Credentials<'_> {
    Credentials {
        bearer: Some(token),
        identity_header: None,
        ..Credentials::default()
    }
}

async fn check<F: TokenFixture>(f: &F, token: &str) -> Result<Principal, AuthError> {
    within(f.auth().authenticate(&bearer(token))).await
}

fn assert_refused(result: Result<Principal, AuthError>, what: &str) {
    match result {
        Err(
            err @ AuthError::Invalid {
                credential: CredentialKind::Bearer,
                ..
            },
        ) => {
            assert_eq!(err.class(), ErrorClass::Unauthenticated, "{what}");
        }
        other => panic!("{what}: expected the token to be refused, got {other:?}"),
    }
}

async fn assert_accepted<F: TokenFixture>(f: &F, claims: &Map<String, Value>, what: &str) {
    let token = f.mint(claims, Signing::Published(Alg::Rs256));
    let result = check(f, &token).await;
    assert!(
        result.is_ok(),
        "{what}: expected the token to be accepted, got {result:?}"
    );
}

async fn assert_not_accepted<F: TokenFixture>(f: &F, claims: &Map<String, Value>, what: &str) {
    let token = f.mint(claims, Signing::Published(Alg::Rs256));
    assert_refused(check(f, &token).await, what);
}

/// A token of each allowed algorithm names Alice: the user, the e-mail, the name and the roles.
pub async fn a_valid_token_names_the_user<F: TokenFixture>(f: F) {
    for alg in ALGORITHMS {
        let token = f.mint(&good_claims(&f), Signing::Published(alg));
        let principal = check(&f, &token)
            .await
            .unwrap_or_else(|e| panic!("{alg:?}: {e:?}"));
        assert_eq!(principal.user.as_str(), USER, "{alg:?}");
        assert_eq!(principal.email.as_deref(), Some(EMAIL), "{alg:?}");
        assert_eq!(principal.name.as_deref(), Some("Alice"), "{alg:?}");
        let roles: Vec<&str> = principal.roles.iter().map(Role::as_str).collect();
        assert_eq!(roles, ["admin", "user"], "{alg:?}");
    }
    within(f.auth().ready()).await.expect("ready after a fetch");
}

/// An expired token is refused; the leeway is 60 seconds.
pub async fn an_expired_token_is_refused<F: TokenFixture>(f: F) {
    let now = now();
    let mut claims = good_claims(&f);
    claims.insert("exp".into(), json!(now - 3600));
    assert_not_accepted(&f, &claims, "expired an hour ago").await;
    claims.insert("exp".into(), json!(now - 120));
    assert_not_accepted(&f, &claims, "expired two minutes ago").await;
    claims.insert("exp".into(), json!(now - 30));
    assert_accepted(&f, &claims, "expired 30 s ago, inside the leeway").await;
}

/// A token that is not valid yet (`nbf`) is refused; the leeway is 60 seconds.
pub async fn a_token_not_yet_valid_is_refused<F: TokenFixture>(f: F) {
    let now = now();
    let mut claims = good_claims(&f);
    claims.insert("nbf".into(), json!(now + 3600));
    assert_not_accepted(&f, &claims, "valid in an hour").await;
    claims.insert("nbf".into(), json!(now + 30));
    assert_accepted(&f, &claims, "valid in 30 s, inside the leeway").await;
    claims.insert("nbf".into(), json!(now - 10));
    assert_accepted(&f, &claims, "valid since 10 s").await;
}

/// `iss` must be the configured issuer, exactly: not a prefix, not with a slash more, not another
/// case, not absent.
pub async fn the_issuer_must_match_exactly<F: TokenFixture>(f: F) {
    let issuer = f.issuer();
    for wrong in [
        format!("{issuer}/"),
        issuer.to_uppercase(),
        issuer.chars().take(issuer.chars().count() - 1).collect(),
        "https://evil.example".to_owned(),
        String::new(),
    ] {
        if wrong == issuer {
            continue;
        }
        let mut claims = good_claims(&f);
        claims.insert("iss".into(), json!(wrong));
        assert_not_accepted(&f, &claims, &format!("issuer {wrong:?}")).await;
    }
    let mut claims = good_claims(&f);
    claims.remove("iss");
    assert_not_accepted(&f, &claims, "no issuer").await;
}

/// The configured audience must be one of `aud` (a string or a list); another is refused, and so
/// is none.
pub async fn the_audience_must_be_the_configured_one<F: TokenFixture>(f: F) {
    let mut claims = good_claims(&f);
    claims.insert("aud".into(), json!("someone-else"));
    assert_not_accepted(&f, &claims, "another audience").await;
    claims.insert("aud".into(), json!([]));
    assert_not_accepted(&f, &claims, "an empty list of audiences").await;
    claims.insert("aud".into(), json!(["someone-else", "another"]));
    assert_not_accepted(&f, &claims, "a list without it").await;
    claims.remove("aud");
    assert_not_accepted(&f, &claims, "no audience").await;
    claims.insert("aud".into(), json!(["someone-else", f.audience()]));
    assert_accepted(&f, &claims, "a list with it").await;
}

/// `exp` and `iat` must be present.
pub async fn exp_and_iat_are_required<F: TokenFixture>(f: F) {
    for missing in ["exp", "iat"] {
        let mut claims = good_claims(&f);
        claims.remove(missing);
        assert_not_accepted(&f, &claims, &format!("no {missing}")).await;
    }
}

/// `alg: none` is refused, even with claims that would pass.
pub async fn alg_none_is_refused<F: TokenFixture>(f: F) {
    let token = f.mint(&good_claims(&f), Signing::NoneAlg);
    assert_refused(check(&f, &token).await, "alg none");
}

/// HS256 is refused, even signed with the bytes of a published public key (the algorithm
/// confusion attack).
pub async fn hs256_signed_with_a_public_key_is_refused<F: TokenFixture>(f: F) {
    // The keys are fetched first, so a refusal is not just "nothing to check against".
    assert_accepted(&f, &good_claims(&f), "a good token").await;
    let token = f.mint(&good_claims(&f), Signing::Hs256WithPublicKey);
    assert_refused(check(&f, &token).await, "HS256 with the public key");
}

/// A token signed by a key the issuer does not publish is refused, whatever its claims.
pub async fn a_token_from_an_unpublished_key_is_refused<F: TokenFixture>(f: F) {
    for alg in ALGORITHMS {
        let token = f.mint(&good_claims(&f), Signing::Unpublished(alg));
        assert_refused(check(&f, &token).await, &format!("{alg:?} unpublished"));
    }
}

/// An unknown `kid` makes the authenticator ask the issuer for its keys again, at most once per
/// interval: a flood of tokens with invented ids is not a flood of requests to the issuer.
pub async fn an_unknown_kid_refetches_at_most_once_per_interval<F: TokenFixture>(f: F) {
    assert_accepted(&f, &good_claims(&f), "a good token").await;
    let before = f.jwks_fetches();
    for _ in 0..8 {
        let token = f.mint(&good_claims(&f), Signing::Unpublished(Alg::Rs256));
        assert_refused(check(&f, &token).await, "an unknown kid");
    }
    let asked = f.jwks_fetches() - before;
    assert!(asked <= 1, "{asked} fetches for one interval");
}

/// A key the issuer publishes after the authenticator fetched its keys is found once the
/// interval has passed: rotation needs no restart.
pub async fn a_rotated_key_is_found_once_published<F: TokenFixture>(f: F) {
    assert_accepted(&f, &good_claims(&f), "a good token").await;
    let token = f.mint(&good_claims(&f), Signing::Unpublished(Alg::Es256));
    assert_refused(check(&f, &token).await, "before the rotation");
    f.publish_unpublished_keys();
    f.wait_out_the_refetch_interval().await;
    let principal = check(&f, &token)
        .await
        .expect("accepted after the rotation");
    assert_eq!(principal.user.as_str(), USER);
}

/// An issuer whose keys cannot be fetched lets nobody in, and says so: `Unavailable`, not a
/// refusal, with `ready` failing too, and it recovers. A token that is only garbage is refused
/// as garbage, with no fetch.
pub async fn the_issuer_down_is_unavailable_and_closed<F: TokenFixture>(f: F) {
    f.set_jwks_down(true);
    let token = f.mint(&good_claims(&f), Signing::Published(Alg::Rs256));
    let err = check(&f, &token).await.expect_err("nobody is let in");
    assert!(matches!(err, AuthError::Unavailable { .. }), "{err:?}");
    assert_eq!(err.class(), ErrorClass::Transient);
    let not_ready = within(f.auth().ready()).await;
    assert!(
        matches!(not_ready, Err(AuthError::Unavailable { .. })),
        "{not_ready:?}"
    );

    let fetches = f.jwks_fetches();
    assert_refused(
        check(&f, "not.a.token").await,
        "garbage while the issuer is down",
    );
    assert_eq!(f.jwks_fetches(), fetches, "garbage does not make a fetch");

    f.set_jwks_down(false);
    f.wait_out_the_refetch_interval().await;
    check(&f, &token)
        .await
        .expect("accepted when the issuer is back");
    within(f.auth().ready())
        .await
        .expect("ready when the issuer is back");
}

/// No bearer is `Missing`; an identity header is not a credential for this authenticator; a
/// token that is not one, or is too large, is refused as malformed.
pub async fn a_missing_or_malformed_bearer_is_unauthenticated<F: TokenFixture>(f: F) {
    let none = within(f.auth().authenticate(&Credentials::default())).await;
    assert!(matches!(none, Err(AuthError::Missing)), "{none:?}");
    let header_only = Credentials {
        bearer: None,
        identity_header: Some("alice@example.com"),
        ..Credentials::default()
    };
    let header = within(f.auth().authenticate(&header_only)).await;
    assert!(matches!(header, Err(AuthError::Missing)), "{header:?}");

    let good = f.mint(&good_claims(&f), Signing::Published(Alg::Rs256));
    let mut garbage = vec![
        String::new(),
        " ".to_owned(),
        "garbage".to_owned(),
        "a.b".to_owned(),
        "a.b.c".to_owned(),
        "..".to_owned(),
        format!("Bearer {good}"),
        format!("{good}.extra"),
        format!("{good}\n"),
        "e30.e30.".to_owned(),
        "a".repeat(64 * 1024),
    ];
    // A header that is not JSON, and one that is not an object.
    garbage.push("bm9wZQ.e30.AAAA".to_owned());
    garbage.push("W10.e30.AAAA".to_owned());
    for token in garbage {
        let shown: String = token.chars().take(24).collect();
        assert_refused(check(&f, &token).await, &format!("malformed {shown:?}"));
    }
}

/// `email_verified: false` is refused, as a boolean and as the text some issuers send; `true` and
/// an absent claim are accepted.
pub async fn email_verified_false_is_refused<F: TokenFixture>(f: F) {
    for refused in [json!(false), json!("false")] {
        let mut claims = good_claims(&f);
        claims.insert("email_verified".into(), refused.clone());
        assert_not_accepted(&f, &claims, &format!("email_verified {refused}")).await;
    }
    let mut claims = good_claims(&f);
    claims.insert("email_verified".into(), json!(true));
    assert_accepted(&f, &claims, "verified").await;
    claims.remove("email_verified");
    assert_accepted(&f, &claims, "no claim about verification").await;
}

/// The user is the configured claim, trimmed and lower-cased, whatever the other claims say; a
/// token without it, or with something that is not text or is empty there, is refused.
pub async fn the_user_claim_is_the_configured_one<F: TokenFixture>(f: F) {
    let claim = f.user_claim();
    let mut claims = good_claims(&f);
    claims.insert("email".into(), json!("someone-else@example.com"));
    claims.insert("sub".into(), json!("subject-9"));
    claims.insert(claim.clone(), json!("  Carol@Example.com "));
    let token = f.mint(&claims, Signing::Published(Alg::Rs256));
    let principal = check(&f, &token).await.expect("a principal");
    assert_eq!(principal.user.as_str(), "carol@example.com");
    assert_eq!(principal.email.as_deref(), Some("someone-else@example.com"));

    for (what, value) in [
        ("a number", Some(json!(7))),
        ("an empty text", Some(json!("   "))),
        ("a list", Some(json!(["carol@example.com"]))),
        ("nothing", None),
    ] {
        let mut claims = good_claims(&f);
        match value {
            Some(v) => claims.insert(claim.clone(), v),
            None => claims.remove(&claim),
        };
        assert_not_accepted(&f, &claims, &format!("the user claim is {what}")).await;
    }
}

/// The roles are the strings at the configured path: a list of texts (others skipped), a single
/// text, none when the claim is absent or of another kind. A bad roles claim never refuses the
/// token.
pub async fn roles_come_from_the_roles_claim<F: TokenFixture>(f: F) {
    let path = f.roles_claim();
    let roles_of = |principal: Principal| -> Vec<String> {
        principal
            .roles
            .iter()
            .map(|r| r.as_str().to_owned())
            .collect()
    };
    for (what, value, expected) in [
        ("texts", json!(["b", "a", "b"]), vec!["a", "b"]),
        (
            "texts and others",
            json!(["a", 7, null, {"x": 1}, "c"]),
            vec!["a", "c"],
        ),
        ("a text", json!("solo"), vec!["solo"]),
        ("a number", json!(3), vec![]),
        ("an object", json!({"a": 1}), vec![]),
        ("an empty list", json!([]), vec![]),
    ] {
        let mut claims = good_claims(&f);
        set_path(&mut claims, &path, value);
        let token = f.mint(&claims, Signing::Published(Alg::Rs256));
        let principal = check(&f, &token)
            .await
            .unwrap_or_else(|e| panic!("roles as {what}: {e:?}"));
        assert_eq!(roles_of(principal), expected, "roles as {what}");
    }
    // No roles claim at all: no roles.
    let mut claims = good_claims(&f);
    let top = path.split('.').next().unwrap_or(&path).to_owned();
    claims.remove(&top);
    let token = f.mint(&claims, Signing::Published(Alg::Rs256));
    let principal = check(&f, &token).await.expect("a principal");
    assert!(principal.roles.is_empty());
}

/// No error, in its text or its debug text, holds the token or what is in it.
pub async fn the_token_is_never_in_an_error<F: TokenFixture>(f: F) {
    let canary = "leak-canary@example.com";
    let now = now();
    let mut tokens = Vec::new();
    let mut expired = good_claims(&f);
    expired.insert("exp".into(), json!(now - 3600));
    expired.insert("email".into(), json!(canary));
    tokens.push(f.mint(&expired, Signing::Published(Alg::Rs256)));
    let mut wrong_audience = good_claims(&f);
    wrong_audience.insert("aud".into(), json!("someone-else"));
    wrong_audience.insert("email".into(), json!(canary));
    tokens.push(f.mint(&wrong_audience, Signing::Published(Alg::Rs256)));
    let mut unpublished = good_claims(&f);
    unpublished.insert("email".into(), json!(canary));
    tokens.push(f.mint(&unpublished, Signing::Unpublished(Alg::Rs256)));
    tokens.push(f.mint(&unpublished, Signing::NoneAlg));
    for token in tokens {
        let err = check(&f, &token).await.expect_err("refused");
        let shown = format!("{err} | {err:?} | {}", orch_core::report(&err));
        assert!(!shown.contains(&token), "an error shows the token: {shown}");
        for part in token.split('.').filter(|p| p.len() > 8) {
            assert!(
                !shown.contains(part),
                "an error shows a part of the token: {shown}"
            );
        }
        assert!(!shown.contains(canary), "an error shows a claim: {shown}");
    }
    f.set_jwks_down(true);
    let good = f.mint(&good_claims(&f), Signing::Published(Alg::Rs256));
    // The first fetch may already have happened; either way no error shows the token.
    if let Err(err) = check(&f, &good).await {
        let shown = format!("{err} | {err:?} | {}", orch_core::report(&err));
        assert!(!shown.contains(&good), "{shown}");
    }
}
