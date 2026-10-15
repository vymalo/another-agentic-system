//! The deployment that keeps nothing for people's own MCP servers (ADR 0046): `NoUserToolServers`,
//! `NoSecrets` and `NoBroker` refuse every call, say why in a fixed sentence that holds no secret,
//! owner or URL, and `NoUserTools` is the bundle of the three. The model is `NoArtifacts`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::{Classify, ErrorClass, UserId};
use orch_ports::memory::MemoryUserToolServers;
use orch_ports::{
    CredentialBroker, CredentialError, CredentialLease, CredentialRef, EgressPolicy,
    KeepsCredentials, NoBroker, NoSecrets, NoUserToolServers, NoUserTools, OAuthStart, OAuthState,
    Ports, RedirectUri, SecretError, SecretMaterial, SecretServer, SecretStore, SecretVersion,
    ServerRevision, ToolSecret, UserAuth, UserToolPorts, UserToolServer, UserToolServerId,
    UserToolServerState, UserToolServerStore, UserToolServerStoreError, UserTools, UserToolsMode,
};

const SENTINEL: &str = "sentinel-7c41a0-do-not-print";

fn owner() -> UserId {
    UserId::new("alice-sentinel-owner@example.test")
}

fn id() -> UserToolServerId {
    UserToolServerId::parse("u-0123456789").unwrap()
}

fn definition() -> UserToolServer {
    let target = EgressPolicy::new()
        .check_url(&format!("https://mcp.example.com/{SENTINEL}"))
        .unwrap();
    UserToolServer::new(id(), "Name", &target, UserAuth::Bearer).unwrap()
}

/// What an error shows: `Display`, `Debug` and the sources.
fn shown(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = format!("{error} {error:?}");
    let mut source = error.source();
    while let Some(next) = source {
        text.push_str(&format!(" {next} {next:?}"));
        source = next.source();
    }
    text
}

fn assert_says_why_and_nothing_else(error: &(dyn std::error::Error + 'static), because: &str) {
    let text = shown(error);
    assert!(text.contains(because), "{text}");
    for private in [
        SENTINEL,
        "alice-sentinel-owner",
        "u-0123456789",
        "mcp.example.com",
    ] {
        assert!(!text.contains(private), "{text}");
    }
}

#[tokio::test]
async fn the_definitions_of_a_deployment_with_the_feature_off_refuse_every_call() {
    let store = NoUserToolServers;
    let server = definition();
    let results = [
        store.insert(&owner(), &server).await.map(|_| ()),
        store.get(&owner(), &id()).await.map(|_| ()),
        store.list(&owner()).await.map(|_| ()),
        store
            .replace(&owner(), &server, ServerRevision::new(1))
            .await
            .map(|_| ()),
        store
            .set_state(
                &owner(),
                &id(),
                UserToolServerState::Connected,
                ServerRevision::new(1),
            )
            .await
            .map(|_| ()),
        store.delete(&owner(), &id()).await,
    ];
    for result in results {
        let error = result.expect_err("a store that is off refuses");
        assert!(
            matches!(error, UserToolServerStoreError::Unavailable { .. }),
            "{error:?}"
        );
        assert_eq!(error.class(), ErrorClass::Transient);
        assert_says_why_and_nothing_else(&error, "userToolServers.enabled");
    }
}

#[tokio::test]
async fn the_secret_store_of_a_deployment_without_secrets_refuses_every_call() {
    let store = NoSecrets;
    let server = SecretServer::credential(&id());
    let material = SecretMaterial::new(SENTINEL.as_bytes().to_vec()).unwrap();
    let results = [
        store.put(&owner(), &server, &material).await.map(|_| ()),
        store.get(&owner(), &server).await.map(|_| ()),
        store.delete(&owner(), &server).await,
        store
            .rotate(&owner(), &server, SecretVersion::new(1), &material)
            .await
            .map(|_| ()),
    ];
    for result in results {
        let error = result.expect_err("a store that keeps nothing refuses");
        assert!(
            matches!(error, SecretError::Unavailable { .. }),
            "{error:?}"
        );
        assert_says_why_and_nothing_else(&error, "secrets");
    }
}

#[tokio::test]
async fn the_broker_of_a_deployment_without_secrets_refuses_every_call() {
    let broker = NoBroker;
    let secret = ToolSecret::new(SENTINEL);
    let redirect =
        RedirectUri::parse("https://chat.example.com/api/user-tool-servers/oauth/callback")
            .unwrap();
    let state = OAuthState::new("0123456789abcdef0123456789abcdef").unwrap();
    let results = [
        broker.store(&owner(), &id(), &secret).await.map(|_| ()),
        broker
            .begin_oauth(&owner(), &id(), &redirect)
            .await
            .map(|_| ()),
        broker
            .complete_oauth(
                &owner(),
                &id(),
                &state,
                &secret,
                Some("https://issuer.example"),
            )
            .await
            .map(|_| ()),
        broker.lease(&owner(), &id()).await.map(|_| ()),
        broker.revoke(&owner(), &id()).await,
    ];
    for result in results {
        let error = result.expect_err("a broker that keeps nothing refuses");
        assert!(
            matches!(error, CredentialError::Unavailable { .. }),
            "{error:?}"
        );
        // a server that cannot be reached for want of a place to keep its credential is
        // unavailable, which is what the person is shown ("broker_down"), never "sign in again"
        assert_eq!(error.class(), ErrorClass::Transient);
        assert_says_why_and_nothing_else(&error, "secrets");
        assert!(
            !shown(&error).contains("0123456789abcdef"),
            "the state is not echoed"
        );
    }
}

#[test]
fn the_bundle_of_nothing_is_off_and_keeps_no_credential() {
    let none = NoUserTools::off();
    assert_eq!(none.mode(), UserToolsMode::Off);
    assert!(!none.mode().is_enabled());
    assert!(!none.mode().keeps_credentials());
    assert_eq!(*none.egress(), EgressPolicy::new());
    // the two are reachable through it, as the refusing ones
    let _: &NoUserToolServers = none.servers();
    let _: &NoBroker = none.broker();
}

/// A broker that stands for one that keeps credentials: it says so with the marker and does nothing
/// else (a real one lives in `orch-credential-broker`).
#[derive(Debug, Clone, Copy)]
struct Lending;

impl CredentialBroker for Lending {
    async fn store(
        &self,
        _: &UserId,
        _: &UserToolServerId,
        _: &ToolSecret,
    ) -> Result<CredentialRef, CredentialError> {
        Err(CredentialError::unavailable("a stand-in"))
    }
    async fn begin_oauth(
        &self,
        _: &UserId,
        _: &UserToolServerId,
        _: &RedirectUri,
    ) -> Result<OAuthStart, CredentialError> {
        Err(CredentialError::unavailable("a stand-in"))
    }
    async fn complete_oauth(
        &self,
        _: &UserId,
        _: &UserToolServerId,
        _: &OAuthState,
        _: &ToolSecret,
        _: Option<&str>,
    ) -> Result<CredentialRef, CredentialError> {
        Err(CredentialError::unavailable("a stand-in"))
    }
    async fn lease(
        &self,
        _: &UserId,
        _: &UserToolServerId,
    ) -> Result<CredentialLease, CredentialError> {
        Err(CredentialError::unavailable("a stand-in"))
    }
    async fn revoke(&self, _: &UserId, _: &UserToolServerId) -> Result<(), CredentialError> {
        Ok(())
    }
}

impl KeepsCredentials for Lending {}

/// The mode of a bundle is what its constructor built, not a second statement beside the ports: each
/// of the three constructors fixes it, and the ports it holds are the ones that mode means.
#[tokio::test]
async fn the_mode_of_a_bundle_is_derived_from_the_constructor_that_built_it() {
    // off: nothing is kept, everything refuses
    let off = NoUserTools::off();
    assert_eq!(off.mode(), UserToolsMode::Off);
    assert!(off.servers().list(&owner()).await.is_err());
    assert!(off.broker().lease(&owner(), &id()).await.is_err());

    // enabled with no secrets section: the definitions work, the broker refuses
    let egress = EgressPolicy::new().allowing_http();
    let without: UserTools<MemoryUserToolServers, NoBroker> =
        UserTools::without_credentials(egress.clone(), MemoryUserToolServers::new());
    assert_eq!(without.mode(), UserToolsMode::WithoutCredentials);
    assert!(without.mode().is_enabled() && !without.mode().keeps_credentials());
    assert_eq!(
        *without.egress(),
        egress,
        "the bundle exposes the policy it was given"
    );
    without
        .servers()
        .insert(&owner(), &definition())
        .await
        .expect("the definitions are kept");
    let error = without.broker().lease(&owner(), &id()).await.unwrap_err();
    assert!(
        matches!(error, CredentialError::Unavailable { .. }),
        "{error:?}"
    );

    // enabled with a broker that keeps credentials
    let full: UserTools<MemoryUserToolServers, Lending> =
        UserTools::full(egress.clone(), MemoryUserToolServers::new(), Lending);
    assert_eq!(full.mode(), UserToolsMode::Full);
    assert!(full.mode().keeps_credentials());
    assert!(
        full.broker().revoke(&owner(), &id()).await.is_ok(),
        "the broker it was given is the one it holds"
    );
}

/// What a bundle owes is the definitions, the broker, the policy and the mode, **and no secret store**:
/// a handler that is given the bundle cannot read secret material through it. The broker holds its
/// own handle on the store; the API never needs one. This implements the trait with exactly those
/// four, which would not compile if the trait still asked for a `Secrets` type and a `secrets()`.
#[test]
fn a_bundle_gives_out_no_secret_store() {
    struct Four {
        egress: EgressPolicy,
        servers: NoUserToolServers,
        broker: NoBroker,
    }
    impl UserToolPorts for Four {
        type Servers = NoUserToolServers;
        type Broker = NoBroker;
        fn mode(&self) -> UserToolsMode {
            UserToolsMode::Off
        }
        fn egress(&self) -> &EgressPolicy {
            &self.egress
        }
        fn servers(&self) -> &NoUserToolServers {
            &self.servers
        }
        fn broker(&self) -> &NoBroker {
            &self.broker
        }
    }
    let four = Four {
        egress: EgressPolicy::new(),
        servers: NoUserToolServers,
        broker: NoBroker,
    };
    assert_eq!(four.mode(), UserToolsMode::Off);
}

#[test]
fn the_modes_say_what_is_enabled_and_what_can_be_kept() {
    assert!(!UserToolsMode::Off.is_enabled());
    assert!(!UserToolsMode::Off.keeps_credentials());
    // enabled with no secrets: only servers that want nothing can be added
    assert!(UserToolsMode::WithoutCredentials.is_enabled());
    assert!(!UserToolsMode::WithoutCredentials.keeps_credentials());
    assert!(UserToolsMode::Full.is_enabled());
    assert!(UserToolsMode::Full.keeps_credentials());
}

#[tokio::test]
async fn a_port_set_that_names_no_user_tools_has_them_off() {
    // the default of the bundle, as the artifact store's is `NoArtifacts`: a `PortSet` of six types
    // still names a complete bundle
    fn off<P: Ports>(ports: &P) -> UserToolsMode {
        ports.user_tools().mode()
    }
    let ports = orch_ports::PortSet {
        store: orch_ports::memory::MemoryStore::new(),
        wakeup: orch_ports::memory::MemoryWakeup::new(),
        agents: orch_ports::memory::ScriptedAgent::new(),
        clock: orch_ports::SystemClock,
        ids: orch_ports::UuidV7Ids,
        model: orch_ports::NoModel,
        auth: orch_ports::RefuseAll,
        registry: orch_ports::FixedRegistry::default(),
        artifacts: orch_ports::NoArtifacts,
        user_tools: NoUserTools::off(),
    };
    assert_eq!(off(&ports), UserToolsMode::Off);
    let lease = ports.user_tools().broker().lease(&owner(), &id()).await;
    assert!(lease.is_err());
}
