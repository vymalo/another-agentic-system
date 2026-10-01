//! The UI catalog through the A2A adapter (ADR 0023, ADR 0008), against an in-process A2A agent
//! over real HTTP: the catalog is sent only to an agent whose live card lists `ui-catalog/v1`, as
//! metadata and in the renderer's capabilities, inline only to an agent that takes it, and the card
//! is read for every message, never remembered.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{
    A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0, KnownExtension, THREAD_TOOLS_EXTENSION,
    UI_CATALOG_EXTENSION, UiCatalogData, UiDelivery, UiVersion, catalog_digest,
};
use orch_ports::{AgentClient, AgentEndpoint, AgentStream, SendContent, SendRequest};
use orch_testsupport::{Call, FakeAgent, FakeAgentOptions, integral_numbers};
use serde_json::{Value, json};

const ID: &str = "https://agents.vymalo.com/a2ui/catalogs/chat";

fn client() -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap()
}

/// A card that lists `ui` (A2UI), `extensions` (ours) and says whether it takes a catalog inline.
async fn agent(ui: &[&str], extensions: &[&str], accepts_inline: bool) -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        ui_extensions: ui.iter().map(|u| (*u).to_owned()).collect(),
        extensions: extensions.iter().map(|u| (*u).to_owned()).collect(),
        accepts_inline_catalogs: accepts_inline,
        ..FakeAgentOptions::default()
    })
    .await
}

fn catalog(version: u32) -> UiCatalogData {
    let catalog = json!({"catalogId": ID, "components": {"Note": {
        "type": "object",
        "title": format!("version {version}"),
        "properties": {"component": {"const": "Note"}},
    }}});
    UiCatalogData {
        catalog_id: ID.to_owned(),
        version,
        digest: catalog_digest(&catalog).unwrap(),
        catalog,
    }
}

async fn drain(mut stream: AgentStream) {
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
            Ok(Some(item)) => {
                item.unwrap();
            }
            Ok(None) => return,
            Err(_) => panic!("the stream did not end"),
        }
    }
}

/// Sends `echo` to `fake` carrying `delivery`, and returns what the agent saw of it.
async fn send(client: &A2aAgentClient, fake: &FakeAgent, delivery: Option<UiDelivery>) -> Call {
    let ep: AgentEndpoint = fake.endpoint("ui", None);
    let n = fake.executions().len();
    let request = SendRequest {
        endpoint: ep,
        message_id: format!("msg-{n}"),
        context_id: format!("ctx-{n}"),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text("echo hi".to_owned()),
        release: None,
        ui_catalog: delivery,
        thread: None,
    };
    drain(client.send_stream(request).await.unwrap()).await;
    fake.executions().pop().unwrap()
}

/// `metadata[ui-catalog/v1]` as the agent saw it, with `version` read as the number it is: a
/// receiver built on protobuf `Struct` (as `a2a-server-lf` is) holds every JSON number as a
/// double, so `2` arrives as `2.0`.
fn told(call: &Call) -> Option<Value> {
    call.ui_catalog.clone().map(|mut metadata| {
        let version = metadata["version"].as_f64().unwrap();
        assert_eq!(version.fract(), 0.0, "an integer, as sent");
        metadata["version"] = json!(version as u64);
        metadata
    })
}

/// The catalogs the message carried inline, with whole numbers read as the integers they are.
fn inline_of(call: &Call) -> Vec<Value> {
    call.inline_catalogs.iter().map(integral_numbers).collect()
}

fn supported(call: &Call, version: &str) -> Vec<String> {
    call.a2ui_capabilities.as_ref().unwrap()[version]["supportedCatalogIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_owned())
        .collect()
}

fn basic() -> Vec<String> {
    UiVersion::V0_9_1
        .basic_catalog_ids()
        .iter()
        .map(|id| (*id).to_owned())
        .collect()
}

/// What an agent without the extension is sent: the basic catalogs, no catalog metadata, and the
/// extension is not activated.
fn is_todays_message(call: &Call) {
    assert_eq!(told(call), None);
    assert!(call.inline_catalogs.is_empty());
    assert!(!call.activates(UI_CATALOG_EXTENSION));
    assert_eq!(supported(call, "v0.9.1"), basic());
}

#[tokio::test]
async fn an_inline_catalog_goes_inline_to_an_agent_that_lists_the_extension_and_takes_it() {
    let fake = agent(&[A2UI_EXTENSION_V0_9_1], &[UI_CATALOG_EXTENSION], true).await;
    let v2 = catalog(2);
    let call = send(&client(), &fake, Some(UiDelivery::Inline(v2.clone()))).await;

    assert_eq!(
        told(&call),
        Some(json!({"catalogId": ID, "version": 2, "digest": v2.digest, "inline": true}))
    );
    assert_eq!(inline_of(&call), vec![v2.catalog.clone()]);
    // and an agent can check the digest of what it received
    assert_eq!(catalog_digest(&inline_of(&call)[0]).unwrap(), v2.digest);
    // ours first, then the basic catalogs, as the A2UI spec lists them
    let mut want = vec![ID.to_owned()];
    want.extend(basic());
    assert_eq!(supported(&call, "v0.9.1"), want);
    // both extensions are activated, in the header
    assert!(call.activates(UI_CATALOG_EXTENSION));
    assert!(call.activates(A2UI_EXTENSION_V0_9_1));
}

#[tokio::test]
async fn an_agent_that_does_not_take_a_catalog_inline_is_told_which_one_and_asks_for_it() {
    let fake = agent(&[A2UI_EXTENSION_V0_9_1], &[UI_CATALOG_EXTENSION], false).await;
    let v2 = catalog(2);
    let call = send(&client(), &fake, Some(UiDelivery::Inline(v2.clone()))).await;

    assert_eq!(
        told(&call),
        Some(json!({"catalogId": ID, "version": 2, "digest": v2.digest, "inline": false}))
    );
    assert!(call.inline_catalogs.is_empty(), "the catalog is not sent");
    assert_eq!(
        supported(&call, "v0.9.1")[0],
        ID,
        "ours is listed all the same"
    );
}

#[tokio::test]
async fn a_reference_is_a_reference_and_ours_is_still_listed_first() {
    let fake = agent(&[A2UI_EXTENSION_V0_9_1], &[UI_CATALOG_EXTENSION], true).await;
    let v2 = catalog(2);
    let call = send(&client(), &fake, Some(UiDelivery::Ref(v2.reference()))).await;

    assert_eq!(
        told(&call),
        Some(json!({"catalogId": ID, "version": 2, "digest": v2.digest, "inline": false}))
    );
    assert!(call.inline_catalogs.is_empty());
    assert_eq!(supported(&call, "v0.9.1")[0], ID);
}

#[tokio::test]
async fn a_card_without_the_uri_gets_exactly_the_message_it_got_before() {
    let v2 = catalog(2);
    // a card that speaks A2UI, takes catalogs inline, and does not list ui-catalog/v1
    let fake = agent(&[A2UI_EXTENSION_V0_9_1], &[THREAD_TOOLS_EXTENSION], true).await;
    let call = send(&client(), &fake, Some(UiDelivery::Inline(v2.clone()))).await;
    is_todays_message(&call);

    // near misses are not the extension
    for near in [
        "https://agents.vymalo.com/a2a/extensions/ui-catalog/v2",
        "https://agents.vymalo.com/a2a/extensions/ui-catalog/v1/",
        "http://agents.vymalo.com/a2a/extensions/ui-catalog/v1",
        "https://agents.vymalo.com/a2a/extensions/UI-catalog/v1",
    ] {
        let fake = agent(&[A2UI_EXTENSION_V0_9_1], &[near], true).await;
        let call = send(&client(), &fake, Some(UiDelivery::Inline(v2.clone()))).await;
        is_todays_message(&call);
    }

    // and it is the same message as one that carries no catalog at all
    let plain = send(&client(), &fake, None).await;
    assert_eq!(plain.a2ui_capabilities, call.a2ui_capabilities);
}

#[tokio::test]
async fn a_message_with_no_delivery_carries_no_catalog_even_to_an_agent_that_lists_the_extension() {
    let fake = agent(&[A2UI_EXTENSION_V0_9_1], &[UI_CATALOG_EXTENSION], true).await;
    let call = send(&client(), &fake, None).await;
    is_todays_message(&call);
}

#[tokio::test]
async fn the_card_is_read_for_every_message() {
    let fake = agent(&[A2UI_EXTENSION_V0_9_1], &[UI_CATALOG_EXTENSION], true).await;
    let c = client();
    let v2 = catalog(2);
    let delivery = || Some(UiDelivery::Inline(v2.clone()));

    assert!(send(&c, &fake, delivery()).await.ui_catalog.is_some());
    // the agent drops the extension: the next message is plain
    fake.set_extensions(&[]);
    is_todays_message(&send(&c, &fake, delivery()).await);
    // it comes back, and stops taking catalogs inline
    fake.set_extensions(&[UI_CATALOG_EXTENSION]);
    fake.set_accepts_inline_catalogs(false);
    let call = send(&c, &fake, delivery()).await;
    assert_eq!(told(&call).unwrap()["inline"], false);
    assert!(call.inline_catalogs.is_empty());
    fake.set_accepts_inline_catalogs(true);
    assert_eq!(send(&c, &fake, delivery()).await.inline_catalogs.len(), 1);
}

#[tokio::test]
async fn an_agent_that_lists_the_extension_but_not_a2ui_is_told_which_catalog_and_nothing_more() {
    let fake = agent(&[], &[UI_CATALOG_EXTENSION], true).await;
    let v2 = catalog(2);
    let call = send(&client(), &fake, Some(UiDelivery::Inline(v2.clone()))).await;

    assert_eq!(
        told(&call),
        Some(json!({"catalogId": ID, "version": 2, "digest": v2.digest, "inline": false}))
    );
    assert_eq!(
        call.a2ui_capabilities, None,
        "no renderer capabilities without A2UI"
    );
    assert!(call.activates(UI_CATALOG_EXTENSION));
    assert!(!call.activates(A2UI_EXTENSION_V0_9_1));
}

#[tokio::test]
async fn the_other_dialect_carries_the_catalog_the_same_way() {
    let fake = agent(&[A2UI_EXTENSION_V1_0], &[UI_CATALOG_EXTENSION], true).await;
    let v2 = catalog(2);
    let call = send(&client(), &fake, Some(UiDelivery::Inline(v2.clone()))).await;

    assert_eq!(inline_of(&call), vec![v2.catalog.clone()]);
    assert_eq!(supported(&call, "v1.0")[0], ID);
    assert_eq!(told(&call).unwrap()["inline"], true);
    assert!(call.activates(A2UI_EXTENSION_V1_0));
}

#[tokio::test]
async fn the_card_says_which_extensions_it_lists_and_whether_it_takes_a_catalog_inline() {
    let fake = agent(
        &[A2UI_EXTENSION_V0_9_1],
        &[
            UI_CATALOG_EXTENSION,
            THREAD_TOOLS_EXTENSION,
            "https://example.com/other/v1",
        ],
        true,
    )
    .await;
    let card = client()
        .read_card(&fake.endpoint("ui", None))
        .await
        .unwrap();
    assert!(card.offers(KnownExtension::UiCatalog));
    assert!(card.offers(KnownExtension::ThreadTools));
    assert!(!card.offers(KnownExtension::Steps));
    assert_eq!(
        card.extensions.len(),
        2,
        "a URI that is not ours is not listed"
    );
    assert!(card.ui.as_ref().unwrap().accepts_inline_catalogs);

    let bare = agent(&[], &[], false).await;
    let card = client()
        .read_card(&bare.endpoint("plain", None))
        .await
        .unwrap();
    assert!(card.extensions.is_empty());
    assert_eq!(card.ui, None);
}
