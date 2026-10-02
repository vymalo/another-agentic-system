//! Files from an agent (ADR 0032) through the A2A adapter, over real HTTP: a `raw` part reaches the
//! worker as `AgentUpdate::File`; a `url` part is a link, unless its host is on the list, and then
//! it is read with no redirect followed and within the cap.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use futures::StreamExt;
use orch_agent_a2a::{A2aAgentClient, A2aConfig, FileFetch};
use orch_core::{AgentUpdate, FileRefusal};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentStream, IdemKey, SendContent, SendRequest,
    TaskHandle,
};
use orch_testsupport::fake::PNG;
use orch_testsupport::{FakeAgent, FakeAgentOptions};

fn client(fetch: Option<FileFetch>) -> A2aAgentClient {
    A2aAgentClient::new(A2aConfig {
        use_system_proxy: false,
        fetch_files: fetch,
        ..A2aConfig::default()
    })
    .unwrap()
}

/// The fake agent's own `host:port`, which serves `/files/chart.png`.
fn own_host(fake: &FakeAgent) -> String {
    fake.base_url().trim_start_matches("http://").to_owned()
}

fn request(ep: &AgentEndpoint, text: &str) -> SendRequest {
    SendRequest {
        endpoint: ep.clone(),
        message_id: format!("msg-{}", text.replace(' ', "-")),
        context_id: "ctx-1".to_owned(),
        task_id: None,
        reference_task_ids: Vec::new(),
        content: SendContent::Text(text.to_owned()),
        release: None,
        ui_catalog: None,
        thread_tools: None,
        history: None,
        steer: false,
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

fn updates(envs: &[AgentEnvelope]) -> Vec<&AgentUpdate> {
    envs.iter()
        .filter_map(|e| e.update.as_ref())
        .filter(|u| !matches!(u, AgentUpdate::Status { .. }))
        .collect()
}

#[tokio::test]
async fn a_raw_part_arrives_as_a_file_with_its_bytes_and_what_the_agent_said_of_it() {
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("files", None);
    let envs = drain(
        client(None)
            .send_stream(request(&ep, "file"))
            .await
            .unwrap(),
    )
    .await;
    let file = envs
        .iter()
        .find(|e| matches!(e.update, Some(AgentUpdate::File { .. })))
        .expect("a file");
    let IdemKey::Task(key) = &file.key else {
        panic!("task scoped")
    };
    assert!(key.ends_with(":file:0"), "{key}");
    assert_eq!(
        file.update,
        Some(AgentUpdate::File {
            name: "chart".into(),
            media_type: Some("image/png".into()),
            filename: Some("chart.png".into()),
            bytes: PNG.to_vec(),
        })
    );
    // the file is the artifact: no artifact of text or a link beside it
    assert!(
        !envs
            .iter()
            .any(|e| matches!(e.update, Some(AgentUpdate::Artifact { .. })))
    );
}

#[tokio::test]
async fn a_url_part_stays_a_link_without_a_list_and_off_it() {
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("files", None);
    let own = format!("{}/files/chart.png", fake.base_url());
    // no list at all
    let envs = drain(
        client(None)
            .send_stream(request(&ep, "file-url"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        updates(&envs),
        [&AgentUpdate::Artifact {
            name: "chart".into(),
            mime_type: None,
            uri: Some(own),
            text: None,
        }]
    );
    // a list that does not name the host, and a host the agent never reaches
    let other = Some(FileFetch::new(vec!["files.example.com".into()], 1 << 20));
    let envs = drain(
        client(other)
            .send_stream(request(&ep, "file-url"))
            .await
            .unwrap(),
    )
    .await;
    assert!(matches!(
        updates(&envs)[..],
        [AgentUpdate::Artifact { uri: Some(_), .. }]
    ));
    let listed = Some(FileFetch::new(vec![own_host(&fake)], 1 << 20));
    let envs = drain(
        client(listed)
            .send_stream(request(&ep, "file-url-other"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        updates(&envs),
        [&AgentUpdate::Artifact {
            name: "chart".into(),
            mime_type: None,
            uri: Some("https://other.example.com/chart.png".into()),
            text: None,
        }]
    );
}

#[tokio::test]
async fn a_url_on_the_list_is_fetched_and_arrives_like_a_raw_part() {
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("files", None);
    let listed = Some(FileFetch::new(vec![own_host(&fake)], 1 << 20));
    let c = client(listed);
    let envs = drain(c.send_stream(request(&ep, "file-url")).await.unwrap()).await;
    assert_eq!(
        updates(&envs),
        [&AgentUpdate::File {
            name: "chart".into(),
            media_type: Some("image/png".into()),
            filename: Some("chart.png".into()),
            bytes: PNG.to_vec(),
        }]
    );
    // a poll of the finished task gives the same file under the same key
    let task = envs[0].task_id.clone();
    let snap = c
        .get_task(&TaskHandle {
            endpoint: ep,
            task_id: task,
        })
        .await
        .unwrap();
    let polled = snap
        .envelopes
        .iter()
        .find(|e| matches!(e.update, Some(AgentUpdate::File { .. })))
        .expect("the poll fetched it too");
    let live = envs
        .iter()
        .find(|e| matches!(e.update, Some(AgentUpdate::File { .. })))
        .unwrap();
    assert_eq!(polled.key, live.key);
    assert_eq!(polled.update, live.update);
}

#[tokio::test]
async fn a_redirect_is_never_followed() {
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("files", None);
    let listed = Some(FileFetch::new(vec![own_host(&fake)], 1 << 20));
    let envs = drain(
        client(listed)
            .send_stream(request(&ep, "file-url-redirect"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        updates(&envs),
        [&AgentUpdate::FileRefused {
            name: "chart".into(),
            mime_type: None,
            reason: FileRefusal::NotKept,
        }]
    );
}

#[tokio::test]
async fn a_file_over_the_cap_is_too_large() {
    let fake = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let ep = fake.endpoint("files", None);
    let small = Some(FileFetch::new(vec![own_host(&fake)], 10));
    let envs = drain(
        client(small)
            .send_stream(request(&ep, "file-url"))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        updates(&envs),
        [&AgentUpdate::FileRefused {
            name: "chart".into(),
            mime_type: None,
            reason: FileRefusal::TooLarge,
        }]
    );
}
