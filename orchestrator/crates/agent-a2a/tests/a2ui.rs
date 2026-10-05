//! The A2UI extension through the A2A adapter (ADR 0013, ADR 0008), against an in-process A2A
//! agent over real HTTP: capability detection on and off (both URIs, read live, never cached),
//! surfaces from messages, status messages and artifacts, envelope rejection, and the user's
//! action going back as a data part of the same task.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::{
    A2UI_EXTENSION_V0_9_1, A2UI_EXTENSION_V1_0, AgentTaskState, AgentUpdate, MAX_OPERATIONS_BYTES,
    UiActionData, UiVersion,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentStream, IdemKey, SendContent, SendRequest,
    TaskHandle,
};
use orch_testsupport::{FakeAgent, FakeAgentOptions};
use serde_json::{Value, json};

fn client() -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        ..A2aConfig::default()
    })
    .unwrap()
}

async fn agent(ui: &[&str]) -> FakeAgent {
    FakeAgent::spawn(FakeAgentOptions {
        ui_extensions: ui.iter().map(|u| (*u).to_owned()).collect(),
        ..FakeAgentOptions::default()
    })
    .await
}

fn text(ep: &AgentEndpoint, text: &str, task_id: Option<String>) -> SendRequest {
    SendRequest {
        endpoint: ep.clone(),
        message_id: format!("msg-{}", text.replace(' ', "-")),
        context_id: "ctx-1".to_owned(),
        task_id,
        reference_task_ids: Vec::new(),
        content: SendContent::Text(text.to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
        steer: false,
        mentions: Vec::new(),
    }
}

fn go(surface: &str) -> UiActionData {
    let mut context = serde_json::Map::new();
    context.insert("choice".into(), json!("a"));
    UiActionData {
        surface_id: surface.into(),
        name: "go".into(),
        source_component_id: "go".into(),
        context,
        version: UiVersion::V0_9_1,
        run_id: Some("run-2".into()),
    }
}

async fn drain(mut stream: AgentStream) -> Vec<AgentEnvelope> {
    let mut out = Vec::new();
    loop {
        match tokio::time::timeout(Duration::from_secs(20), stream.next()).await {
            Ok(Some(item)) => out.push(item.unwrap()),
            Ok(None) => return out,
            Err(_) => panic!("stream did not end; got {out:?}"),
        }
    }
}

fn kinds(envs: &[AgentEnvelope]) -> Vec<String> {
    envs.iter()
        .map(|e| match &e.update {
            Some(AgentUpdate::Status { state, .. }) => format!("status:{state:?}"),
            Some(AgentUpdate::Artifact { name, .. }) => format!("artifact:{name}"),
            Some(AgentUpdate::Message { .. }) => "message".to_owned(),
            Some(AgentUpdate::Reasoning { message_id, .. }) => format!("reasoning:{message_id}"),
            Some(AgentUpdate::Ui { operations }) => format!("ui:{}", operations.len()),
            Some(AgentUpdate::UiRejected { .. }) => "ui-rejected".to_owned(),
            Some(AgentUpdate::Step(step)) => format!("step:{}", step.id),
            Some(AgentUpdate::File { name, .. }) => format!("file:{name}"),
            Some(AgentUpdate::FileKept { name, .. } | AgentUpdate::FileRefused { name, .. }) => {
                format!("file-logged:{name}")
            }
            None => "-".to_owned(),
        })
        .collect()
}

fn ui_ops(env: &AgentEnvelope) -> Vec<Value> {
    let Some(AgentUpdate::Ui { operations }) = &env.update else {
        panic!("not a surface: {env:?}");
    };
    operations.clone()
}

// ------------------------------------------------------------ capability detection

#[tokio::test]
async fn a_card_without_a2ui_has_no_ui_support() {
    let fake = agent(&[]).await;
    let card = client()
        .read_card(&fake.endpoint("plain", None))
        .await
        .unwrap();
    assert_eq!(card.ui, None);
}

#[tokio::test]
async fn each_uri_is_detected_from_the_card() {
    for (uris, want) in [
        (vec![A2UI_EXTENSION_V0_9_1], vec![UiVersion::V0_9_1]),
        (vec![A2UI_EXTENSION_V1_0], vec![UiVersion::V1_0]),
        (
            vec![A2UI_EXTENSION_V1_0, A2UI_EXTENSION_V0_9_1],
            vec![UiVersion::V0_9_1, UiVersion::V1_0],
        ),
    ] {
        let fake = agent(&uris).await;
        let card = client()
            .read_card(&fake.endpoint("ui", None))
            .await
            .unwrap();
        assert_eq!(card.ui.unwrap().versions, want, "{uris:?}");
    }
}

#[tokio::test]
async fn a_card_that_changes_is_noticed_by_the_next_read() {
    // Read live, never cached (ADR 0008): same client, same endpoint, the card changes.
    let fake = agent(&[A2UI_EXTENSION_V0_9_1]).await;
    let ep = fake.endpoint("ui", None);
    let c = client();
    assert!(c.read_card(&ep).await.unwrap().ui.is_some());
    fake.set_ui_extensions(&[]);
    assert!(c.read_card(&ep).await.unwrap().ui.is_none());
    fake.set_ui_extensions(&[A2UI_EXTENSION_V1_0]);
    let ui = c.read_card(&ep).await.unwrap().ui.unwrap();
    assert_eq!(ui.versions, vec![UiVersion::V1_0]);
}

#[tokio::test]
async fn capabilities_are_sent_only_when_the_live_card_lists_the_extension() {
    let fake = agent(&[]).await;
    let ep = fake.endpoint("ui", None);
    let c = client();

    // Plain A2A: nothing A2UI-specific is sent.
    drain(c.send_stream(text(&ep, "echo one", None)).await.unwrap()).await;
    let call = &fake.executions()[0];
    assert_eq!(call.a2ui_capabilities, None);
    assert!(!call.activates(A2UI_EXTENSION_V0_9_1) && !call.activates(A2UI_EXTENSION_V1_0));

    // The card gains the extension: the very next message carries the catalogs and activates it.
    fake.set_ui_extensions(&[A2UI_EXTENSION_V0_9_1]);
    drain(c.send_stream(text(&ep, "echo two", None)).await.unwrap()).await;
    let call = &fake.executions()[1];
    assert_eq!(
        call.a2ui_capabilities,
        Some(json!({"v0.9.1": {"supportedCatalogIds": [
            "https://a2ui.org/specification/v0_9_1/catalogs/basic/catalog.json",
            "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"]}}))
    );
    assert!(call.activates(A2UI_EXTENSION_V0_9_1));

    // The card loses it again: fail closed, the third message is plain again.
    fake.set_ui_extensions(&[]);
    drain(c.send_stream(text(&ep, "echo three", None)).await.unwrap()).await;
    let call = &fake.executions()[2];
    assert_eq!(call.a2ui_capabilities, None);
    assert!(!call.activates(A2UI_EXTENSION_V0_9_1));
}

#[tokio::test]
async fn the_candidate_version_speaks_its_own_dialect() {
    let fake = agent(&[A2UI_EXTENSION_V1_0]).await;
    let ep = fake.endpoint("ui", None);
    drain(
        client()
            .send_stream(text(&ep, "echo hi", None))
            .await
            .unwrap(),
    )
    .await;
    let call = &fake.executions()[0];
    assert_eq!(
        call.a2ui_capabilities,
        Some(json!({"v1.0": {"supportedCatalogIds": [
            "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json"]}}))
    );
    assert!(call.activates(A2UI_EXTENSION_V1_0));
    assert!(!call.activates(A2UI_EXTENSION_V0_9_1));
}

#[tokio::test]
async fn a_card_listing_both_gets_the_current_release() {
    let fake = agent(&[A2UI_EXTENSION_V1_0, A2UI_EXTENSION_V0_9_1]).await;
    let ep = fake.endpoint("ui", None);
    drain(
        client()
            .send_stream(text(&ep, "echo hi", None))
            .await
            .unwrap(),
    )
    .await;
    let call = &fake.executions()[0];
    assert!(
        call.a2ui_capabilities
            .as_ref()
            .unwrap()
            .get("v0.9.1")
            .is_some()
    );
    assert!(call.activates(A2UI_EXTENSION_V0_9_1));
    assert!(!call.activates(A2UI_EXTENSION_V1_0));
}

// ------------------------------------------------------------------------- surfaces

#[tokio::test]
async fn a_surface_in_an_artifact_then_a_question() {
    let fake = agent(&[A2UI_EXTENSION_V0_9_1]).await;
    let ep = fake.endpoint("ui", None);
    let envs = drain(
        client()
            .send_stream(text(&ep, "ui pick", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        kinds(&envs),
        ["status:Working", "ui:1", "ui:1", "status:InputRequired"],
        "the surface comes before the question, and no artifact carries its JSON"
    );
    let task = &envs[0].task_id;
    let IdemKey::Task(key) = &envs[1].key else {
        panic!("task scoped");
    };
    assert!(
        key.starts_with(&format!("a2a:{task}:artifact:")) && key.ends_with(":ui:0"),
        "{key}"
    );
    let (first, second) = (ui_ops(&envs[1]), ui_ops(&envs[2]));
    assert!(first[0].get("createSurface").is_some());
    assert_eq!(second[0]["updateComponents"]["surfaceId"], "s1");
    assert_eq!(envs[3].task_state, Some(AgentTaskState::InputRequired));
}

#[tokio::test]
async fn a_surface_in_an_agent_message() {
    let fake = agent(&[A2UI_EXTENSION_V0_9_1]).await;
    let ep = fake.endpoint("ui", None);
    let envs = drain(
        client()
            .send_stream(text(&ep, "ui-msg now", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        kinds(&envs),
        [
            "status:Working",
            "message",
            "ui:2",
            "artifact:result",
            "status:Completed"
        ]
    );
    let IdemKey::Task(key) = &envs[2].key else {
        panic!("task scoped");
    };
    assert!(key.ends_with("-ui-msg:ui:1"), "{key}");
}

#[tokio::test]
async fn a_surface_in_the_question_comes_before_the_status_and_the_detail_is_only_text() {
    let fake = agent(&[A2UI_EXTENSION_V0_9_1]).await;
    let ep = fake.endpoint("ui", None);
    let envs = drain(
        client()
            .send_stream(text(&ep, "ui-status now", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        kinds(&envs),
        ["status:Working", "ui:2", "status:InputRequired"]
    );
    assert_eq!(
        envs[2].update,
        Some(AgentUpdate::Status {
            state: AgentTaskState::InputRequired,
            detail: Some("Which one?".into())
        })
    );
}

#[tokio::test]
async fn a_surface_is_relayed_even_when_the_card_does_not_list_the_extension() {
    // Activation is optional in A2UI: a part is recognised by its media type (ADR 0013).
    let fake = agent(&[]).await;
    let ep = fake.endpoint("ui", None);
    let envs = drain(
        client()
            .send_stream(text(&ep, "ui pick", None))
            .await
            .unwrap(),
    )
    .await;
    assert!(kinds(&envs).contains(&"ui:1".to_owned()));
}

#[tokio::test]
async fn a_malformed_or_oversized_part_is_refused_and_the_turn_goes_on() {
    let fake = agent(&[A2UI_EXTENSION_V0_9_1]).await;
    let ep = fake.endpoint("ui", None);
    for (script, why) in [
        ("ui-bad now", "not an array"),
        ("ui-big now", "more than the limit"),
    ] {
        let envs = drain(client().send_stream(text(&ep, script, None)).await.unwrap()).await;
        assert_eq!(
            kinds(&envs),
            ["status:Working", "ui-rejected", "status:Completed"],
            "{script}"
        );
        let Some(AgentUpdate::UiRejected { reason }) = &envs[1].update else {
            panic!("{envs:?}");
        };
        assert!(reason.contains(why), "{reason}");
        assert!(
            !reason.contains("xxxx"),
            "the refusal never repeats what the agent sent"
        );
    }
    const { assert!(MAX_OPERATIONS_BYTES < 70 * 1024) };
}

#[tokio::test]
async fn a_polled_task_carries_the_surface_under_the_stream_keys() {
    let fake = agent(&[A2UI_EXTENSION_V0_9_1]).await;
    let ep = fake.endpoint("ui", None);
    let c = client();
    let live = drain(c.send_stream(text(&ep, "ui pick", None)).await.unwrap()).await;
    let handle = TaskHandle {
        endpoint: ep.clone(),
        task_id: live[0].task_id.clone(),
    };
    let snapshot = c.get_task(&handle).await.unwrap();
    let live_ui = live
        .iter()
        .find(|e| kinds(std::slice::from_ref(e))[0].starts_with("ui"))
        .unwrap();
    let polled_ui = snapshot
        .envelopes
        .iter()
        .find(|e| kinds(std::slice::from_ref(e))[0].starts_with("ui"))
        .unwrap();
    assert_eq!(
        polled_ui.key, live_ui.key,
        "a poll and the stream collapse into one event"
    );
    assert_eq!(ui_ops(polled_ui), ui_ops(live_ui));
}

// --------------------------------------------------------------------------- actions

#[tokio::test]
async fn an_action_goes_back_as_a_data_part_of_the_same_task() {
    let fake = agent(&[A2UI_EXTENSION_V0_9_1]).await;
    let ep = fake.endpoint("ui", None);
    let c = client();
    let first = drain(c.send_stream(text(&ep, "ui pick", None)).await.unwrap()).await;
    let task = first[0].task_id.clone();

    let at = "2026-09-29T12:00:00Z".parse().unwrap();
    let request = SendRequest {
        endpoint: ep.clone(),
        message_id: "msg-action".into(),
        context_id: "ctx-1".into(),
        task_id: Some(task.clone()),
        reference_task_ids: Vec::new(),
        content: SendContent::UiAction {
            action: go("s1"),
            at,
        },
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
        steer: false,
        mentions: Vec::new(),
    };
    let second = drain(c.send_stream(request).await.unwrap()).await;
    assert_eq!(
        statuses_of(&second),
        [AgentTaskState::Working, AgentTaskState::Completed]
    );
    let artifact = second
        .iter()
        .find_map(|e| match &e.update {
            Some(AgentUpdate::Artifact { text, .. }) => text.clone(),
            _ => None,
        })
        .unwrap();
    assert_eq!(artifact, "answered: ui-action go");

    let call = &fake.executions()[1];
    assert_eq!(call.task_id, task, "the same task");
    assert!(call.resuming);
    assert_eq!(
        call.actions,
        [json!({
            "version": "v0.9.1",
            "action": {"name": "go", "surfaceId": "s1", "sourceComponentId": "go",
                       "timestamp": "2026-09-29T12:00:00Z", "context": {"choice": "a"}}
        })]
    );
    assert!(
        call.a2ui_capabilities.is_some(),
        "the capabilities go with every message, actions included"
    );
    assert!(call.activates(A2UI_EXTENSION_V0_9_1));
}

#[tokio::test]
async fn an_action_speaks_the_version_of_its_surface_whatever_the_card_says() {
    let fake = agent(&[]).await;
    let ep = fake.endpoint("ui", None);
    let c = client();
    let first = drain(c.send_stream(text(&ep, "ui pick", None)).await.unwrap()).await;
    let mut action = go("s1");
    action.version = UiVersion::V1_0;
    let request = SendRequest {
        endpoint: ep.clone(),
        message_id: "msg-action".into(),
        context_id: "ctx-1".into(),
        task_id: Some(first[0].task_id.clone()),
        reference_task_ids: Vec::new(),
        content: SendContent::UiAction {
            action,
            at: "2026-09-29T12:00:00Z".parse().unwrap(),
        },
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
        steer: false,
        mentions: Vec::new(),
    };
    drain(c.send_stream(request).await.unwrap()).await;
    let call = &fake.executions()[1];
    assert_eq!(call.actions[0]["version"], "v1.0");
    assert_eq!(
        call.a2ui_capabilities, None,
        "no advertisement, no capabilities"
    );
}

fn statuses_of(envs: &[AgentEnvelope]) -> Vec<AgentTaskState> {
    envs.iter()
        .filter_map(|e| match &e.update {
            Some(AgentUpdate::Status { state, .. }) => Some(*state),
            _ => None,
        })
        .collect()
}
