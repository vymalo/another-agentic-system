//! What the broker keeps in the secret store, and how.
//!
//! The store keeps **opaque bytes**; these are the broker's records, serialised as JSON with a
//! version (`{"v":1,"d":…}`) and sealed by the adapter. Two slots per server
//! ([`SecretServer`](orch_ports::SecretServer)): the **credential** ([`Record`]) and the **pending
//! flow** ([`FlowRecord`]).
//!
//! Every secret in a record is a [`ToolSecret`]: wiped when dropped, printed as `<redacted>`. No
//! record has a `Debug` of its own, because a record holds URLs (a path can carry a secret). A
//! record that cannot be read is reported as that and nothing more: a `serde_json` error can quote
//! the text it could not read, so none is ever kept.

use orch_ports::{SecretMaterial, ToolSecret};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The version of the records. A record of another version cannot be read.
const VERSION: u32 = 1;

/// A `ToolSecret` as a JSON string.
pub(crate) mod secret {
    use orch_ports::ToolSecret;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(secret: &ToolSecret, out: S) -> Result<S::Ok, S::Error> {
        out.serialize_str(secret.expose())
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(input: D) -> Result<ToolSecret, D::Error> {
        Ok(ToolSecret::new(String::deserialize(input)?))
    }
}

/// An optional `ToolSecret` as a JSON string or `null`.
pub(crate) mod secret_opt {
    use orch_ports::ToolSecret;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(
        secret: &Option<ToolSecret>,
        out: S,
    ) -> Result<S::Ok, S::Error> {
        match secret {
            Some(secret) => out.serialize_some(secret.expose()),
            None => out.serialize_none(),
        }
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        input: D,
    ) -> Result<Option<ToolSecret>, D::Error> {
        Ok(Option::<String>::deserialize(input)?.map(ToolSecret::new))
    }
}

/// Why a record was not written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum KeepError {
    /// More than the secret store keeps (16 KiB).
    #[error("the credential is too large to keep")]
    TooLarge,
    /// The record could not be written as JSON.
    #[error("the credential could not be written")]
    Unwritable,
}

#[derive(Serialize)]
struct Out<'a, T> {
    v: u32,
    d: &'a T,
}

#[derive(Deserialize)]
struct In<T> {
    v: u32,
    d: T,
}

/// A record as the opaque material of the secret store.
pub(crate) fn seal<T: Serialize>(record: &T) -> Result<SecretMaterial, KeepError> {
    let bytes = serde_json::to_vec(&Out {
        v: VERSION,
        d: record,
    })
    .map_err(|_| KeepError::Unwritable)?;
    SecretMaterial::new(bytes).map_err(|_| KeepError::TooLarge)
}

/// The record in `material`, or `None` for anything that is not a record of this version.
pub(crate) fn open<T: DeserializeOwned>(material: &SecretMaterial) -> Option<T> {
    let parsed: In<T> = serde_json::from_slice(material.expose()).ok()?;
    (parsed.v == VERSION).then_some(parsed.d)
}

// ------------------------------------------------------------------------------- the credential

/// The credential of a server, bound to the URL it was issued for.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Record {
    /// A bearer token the person typed.
    Bearer {
        /// The server's normal-form URL when it was stored.
        url: String,
        #[serde(with = "secret")]
        token: ToolSecret,
    },
    /// An API key the person typed.
    ApiKey {
        url: String,
        #[serde(with = "secret")]
        value: ToolSecret,
    },
    /// The grant of an OAuth authorization.
    #[serde(rename = "oauth2")]
    OAuth2(Box<OAuthGrant>),
}

impl Record {
    /// The URL the credential was issued for.
    pub(crate) fn url(&self) -> &str {
        match self {
            Record::Bearer { url, .. } | Record::ApiKey { url, .. } => url,
            Record::OAuth2(grant) => &grant.url,
        }
    }
}

/// An OAuth grant: who the client is, where the authorization server's endpoints are (as it named
/// them when it issued the grant), and the tokens, if there are any left.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct OAuthGrant {
    /// The server's normal-form URL when the flow began: what a lease compares with the
    /// definition's now.
    pub(crate) url: String,
    /// The `resource` the tokens were requested for (RFC 8707).
    pub(crate) resource: String,
    /// The scopes of the definition when the flow began: a grant is for those, and no others.
    #[serde(default)]
    pub(crate) scopes: Vec<String>,
    /// The `client_id` of the definition when the flow began (`None`: the broker's own client).
    #[serde(default)]
    pub(crate) client_id: Option<String>,
    pub(crate) client: ClientRecord,
    pub(crate) endpoints: Endpoints,
    /// `None` once the grant is known to be gone (`invalid_grant`): the client stays, the tokens
    /// do not, and nobody asks the authorization server again.
    pub(crate) tokens: Option<Tokens>,
    /// While a replica is asking the authorization server to refresh: when its claim ends, in
    /// seconds since the epoch. The claim is written with `rotate` **before** the request, so of
    /// two replicas that need a refresh one asks and the other waits for its tokens: a refresh
    /// token that rotates must not be presented twice (RFC 9700, section 4.14.2). `None` when
    /// nobody is refreshing.
    #[serde(default)]
    pub(crate) refreshing_until: Option<i64>,
}

/// Whether two lists name the same scopes: as sets, in any order, a repeat counting once.
fn same_scopes(a: &[String], b: &[String]) -> bool {
    let sorted = |scopes: &[String]| {
        let mut scopes = scopes.to_vec();
        scopes.sort();
        scopes.dedup();
        scopes
    };
    sorted(a) == sorted(b)
}

impl OAuthGrant {
    /// Whether the grant was made for these scopes (in any order) and this `client_id` of the
    /// definition. A definition edited since is not the one the person authorized.
    pub(crate) fn issued_for(&self, scopes: &[String], client_id: Option<&str>) -> bool {
        self.client_id.as_deref() == client_id && same_scopes(&self.scopes, scopes)
    }

    /// Whether a replica holds the claim to refresh at `now`.
    pub(crate) fn claimed_at(&self, now: jiff::Timestamp) -> bool {
        self.refreshing_until
            .is_some_and(|until| until > now.as_second())
    }
}

/// How the client authenticates at the token endpoint.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuthMethod {
    /// A public client: `client_id` in the body, no secret.
    None,
    /// `client_secret_basic`.
    Basic,
    /// `client_secret_post`.
    Post,
}

/// The client of the flow.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ClientRecord {
    pub(crate) id: String,
    #[serde(default, with = "secret_opt")]
    pub(crate) secret: Option<ToolSecret>,
    pub(crate) method: AuthMethod,
    /// For a client the broker registered itself (RFC 7591): where and when, so that the next
    /// `begin_oauth` for the same authorization server and redirect URI uses it instead of
    /// registering another. `None` for a client the person gave and for a metadata document.
    #[serde(default)]
    pub(crate) registration: Option<Registration>,
}

/// What a dynamic registration was for.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Registration {
    /// The issuer of the authorization server it was made at.
    pub(crate) issuer: String,
    /// The one redirect URI it was registered with.
    pub(crate) redirect_uri: String,
    /// When, in seconds since the epoch.
    pub(crate) at: i64,
}

/// The most bytes of a client id the broker keeps (a registration's, or a metadata document's URL).
pub(crate) const MAX_CLIENT_ID_BYTES: usize = 2048;

/// The most bytes of a client secret the broker keeps.
pub(crate) const MAX_CLIENT_SECRET_BYTES: usize = 1024;

impl ClientRecord {
    /// A public client (pre-registered, or named by its metadata document).
    pub(crate) fn public(id: impl Into<String>) -> Self {
        ClientRecord {
            id: id.into(),
            secret: None,
            method: AuthMethod::None,
            registration: None,
        }
    }

    /// The largest client a registration can give, made at `registration`: stands in for the real
    /// one when the flow is sized, before anything is registered.
    pub(crate) fn largest(registration: Registration) -> Self {
        ClientRecord {
            id: "x".repeat(MAX_CLIENT_ID_BYTES),
            secret: Some(ToolSecret::new("x".repeat(MAX_CLIENT_SECRET_BYTES))),
            method: AuthMethod::Basic,
            registration: Some(registration),
        }
    }
}

/// The endpoints the broker uses after the flow began, as the authorization server named them.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Endpoints {
    pub(crate) token: String,
    pub(crate) revocation: Option<String>,
}

/// The tokens of a grant.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Tokens {
    #[serde(with = "secret")]
    pub(crate) access: ToolSecret,
    /// When the access token ends, in seconds since the epoch; `None` when the authorization
    /// server did not say and there is no refresh token to ask again with.
    pub(crate) expires_at: Option<i64>,
    #[serde(default, with = "secret_opt")]
    pub(crate) refresh: Option<ToolSecret>,
}

// ---------------------------------------------------------------------------- the pending flow

/// The pending flow of a server: begun, or already claimed by a callback.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FlowRecord {
    Pending(Box<PendingFlow>),
    /// A callback took the `state`: nobody else can.
    Used,
}

/// An authorization that has begun and not ended.
#[derive(Serialize, Deserialize)]
pub(crate) struct PendingFlow {
    /// The `state` that comes back on the redirect.
    pub(crate) state: String,
    /// The issuer identifier of the authorization server the person was sent to: what the `iss`
    /// of the callback is compared with (RFC 9207, section 2.4; RFC 9700, section 4.4.2).
    pub(crate) issuer: String,
    /// Whether that server advertised `authorization_response_iss_parameter_supported`: when it
    /// did, a callback without `iss` is refused, and when it did not, one with `iss` is.
    pub(crate) iss_advertised: bool,
    #[serde(with = "secret")]
    pub(crate) verifier: ToolSecret,
    /// When the flow ends, in seconds since the epoch.
    pub(crate) expires_at: i64,
    /// The server's normal-form URL when the flow began.
    pub(crate) url: String,
    pub(crate) resource: String,
    /// The scopes and the `client_id` of the definition, which the grant will be for.
    pub(crate) scopes: Vec<String>,
    pub(crate) client_id: Option<String>,
    /// The `redirect_uri` of the authorization request, sent again byte for byte.
    pub(crate) redirect_uri: String,
    pub(crate) client: ClientRecord,
    pub(crate) endpoints: Endpoints,
}

impl PendingFlow {
    /// Whether the flow began for this definition: the **URL** (normal form), the **scopes** (a set)
    /// and the **client id** it holds are the ones the definition has now. What is not part of a
    /// grant (the name) is not compared, so a rename does not end a flow.
    pub(crate) fn is_for(&self, url: &str, scopes: &[String], client_id: Option<&str>) -> bool {
        self.url == url
            && self.client_id.as_deref() == client_id
            && same_scopes(&self.scopes, scopes)
    }

    /// RFC 9207, section 2.4, for the `iss` a callback carried: `Err` says why it is refused, in a
    /// fixed word for the log (never the value, which a stranger chose).
    ///
    /// | advertised | `iss` | |
    /// |---|---|---|
    /// | yes | equal to the issuer | accepted |
    /// | yes | another | `mismatch` |
    /// | yes | none | `missing` |
    /// | no | one | `unexpected` |
    /// | no | none | accepted |
    ///
    /// The comparison is `==` on the strings: RFC 9207 asks for simple string comparison
    /// (RFC 3986, section 6.2.1), and a lookalike issuer is a different issuer.
    pub(crate) fn check_issuer(&self, iss: Option<&str>) -> Result<(), &'static str> {
        match (self.iss_advertised, iss) {
            (true, Some(iss)) if iss == self.issuer => Ok(()),
            (true, Some(_)) => Err("mismatch"),
            (true, None) => Err("missing"),
            (false, Some(_)) => Err("unexpected"),
            (false, None) => Ok(()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn flow(iss_advertised: bool) -> PendingFlow {
        PendingFlow {
            state: "s".repeat(32),
            issuer: "https://as.example.com".to_owned(),
            iss_advertised,
            verifier: ToolSecret::new("v"),
            expires_at: 0,
            url: "https://mcp.example.com/mcp".to_owned(),
            resource: "https://mcp.example.com/mcp".to_owned(),
            scopes: Vec::new(),
            client_id: None,
            redirect_uri: "https://app.example.com/cb".to_owned(),
            client: ClientRecord::public("client-1"),
            endpoints: Endpoints {
                token: "https://as.example.com/token".to_owned(),
                revocation: None,
            },
        }
    }

    #[test]
    fn the_issuer_of_a_callback_is_checked_as_rfc_9207_says() {
        let issuer = "https://as.example.com";
        let advertised = flow(true);
        assert_eq!(advertised.check_issuer(Some(issuer)), Ok(()));
        assert_eq!(
            advertised.check_issuer(Some("https://evil.example")),
            Err("mismatch")
        );
        assert_eq!(
            advertised.check_issuer(Some("https://as.example.com/")),
            Err("mismatch")
        );
        assert_eq!(advertised.check_issuer(Some("")), Err("mismatch"));
        assert_eq!(advertised.check_issuer(None), Err("missing"));

        let silent = flow(false);
        assert_eq!(silent.check_issuer(None), Ok(()));
        assert_eq!(silent.check_issuer(Some(issuer)), Err("unexpected"));
        assert_eq!(silent.check_issuer(Some("")), Err("unexpected"));
    }

    fn grant() -> OAuthGrant {
        OAuthGrant {
            url: "https://mcp.example.com/mcp".to_owned(),
            resource: "https://mcp.example.com/mcp".to_owned(),
            scopes: vec!["read".to_owned(), "write".to_owned()],
            client_id: None,
            client: ClientRecord {
                id: "client-1".to_owned(),
                secret: Some(ToolSecret::new("client-secret-1")),
                method: AuthMethod::Basic,
                registration: None,
            },
            endpoints: Endpoints {
                token: "https://as.example.com/token".to_owned(),
                revocation: None,
            },
            tokens: Some(Tokens {
                access: ToolSecret::new("access-1"),
                expires_at: Some(1_700_000_000),
                refresh: Some(ToolSecret::new("refresh-1")),
            }),
            refreshing_until: None,
        }
    }

    #[test]
    fn a_claim_holds_until_its_second_and_not_after() {
        let at = |s| jiff::Timestamp::from_second(s).unwrap();
        let mut claimed = grant();
        assert!(!claimed.claimed_at(at(100)), "no claim");
        claimed.refreshing_until = Some(130);
        assert!(claimed.claimed_at(at(129)));
        assert!(!claimed.claimed_at(at(130)), "it ends at its second");
        assert!(!claimed.claimed_at(at(131)));
    }

    #[test]
    fn a_grant_is_for_its_scopes_in_any_order_and_its_client_id() {
        let g = grant();
        let scopes = |names: &[&str]| names.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(g.issued_for(&scopes(&["read", "write"]), None));
        assert!(g.issued_for(&scopes(&["write", "read", "read"]), None));
        assert!(!g.issued_for(&scopes(&["read"]), None));
        assert!(!g.issued_for(&scopes(&["read", "write", "admin"]), None));
        assert!(!g.issued_for(&[], None));
        assert!(!g.issued_for(&scopes(&["read", "write"]), Some("client-2")));
    }

    #[test]
    fn a_grant_kept_before_claims_existed_has_none() {
        let text = br#"{"v":1,"d":{"oauth2":{"url":"u","resource":"r","client":{"id":"c","secret":null,"method":"none"},"endpoints":{"token":"t","revocation":null},"tokens":null}}}"#;
        let read = open::<Record>(&SecretMaterial::new(text.to_vec()).unwrap()).unwrap();
        let Record::OAuth2(grant) = read else {
            panic!("a grant");
        };
        assert_eq!(grant.refreshing_until, None);
    }

    #[test]
    fn a_grant_reads_back_whole() {
        let sealed = seal(&Record::OAuth2(Box::new(grant()))).unwrap();
        let Some(Record::OAuth2(read)) = open::<Record>(&sealed) else {
            panic!("an OAuth grant");
        };
        let tokens = read.tokens.as_ref().unwrap();
        assert_eq!(tokens.access.expose(), "access-1");
        assert_eq!(tokens.refresh.as_ref().unwrap().expose(), "refresh-1");
        assert_eq!(tokens.expires_at, Some(1_700_000_000));
        assert_eq!(
            read.client.secret.as_ref().unwrap().expose(),
            "client-secret-1"
        );
        assert!(read.client.method == AuthMethod::Basic);
        assert_eq!(read.url, "https://mcp.example.com/mcp");
    }

    #[test]
    fn a_bearer_and_a_key_read_back_with_their_url() {
        for record in [
            Record::Bearer {
                url: "https://a.example/".to_owned(),
                token: ToolSecret::new("t"),
            },
            Record::ApiKey {
                url: "https://b.example/".to_owned(),
                value: ToolSecret::new("k"),
            },
        ] {
            let url = record.url().to_owned();
            let read = open::<Record>(&seal(&record).unwrap()).unwrap();
            assert_eq!(read.url(), url);
        }
    }

    #[test]
    fn a_record_of_another_version_or_shape_is_not_read() {
        let other =
            SecretMaterial::new(br#"{"v":2,"d":{"bearer":{"url":"u","token":"t"}}}"#.to_vec())
                .unwrap();
        assert!(open::<Record>(&other).is_none());
        let junk = SecretMaterial::new(b"not json at all".to_vec()).unwrap();
        assert!(open::<Record>(&junk).is_none());
        let wrong = SecretMaterial::new(br#"{"v":1,"d":{"pending":{}}}"#.to_vec()).unwrap();
        assert!(open::<Record>(&wrong).is_none());
    }

    #[test]
    fn a_record_over_the_stores_limit_is_too_large() {
        let record = Record::Bearer {
            url: "https://a.example/".to_owned(),
            token: ToolSecret::new("x".repeat(orch_ports::MAX_SECRET_BYTES)),
        };
        assert_eq!(seal(&record).err(), Some(KeepError::TooLarge));
    }
}
