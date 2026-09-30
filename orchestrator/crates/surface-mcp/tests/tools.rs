//! The tools, through an in-process rmcp client over streamable HTTP, against the in-memory stack.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::{AgentId, AgentUpdate, EventBody, EventKind, Input, Origin};
use orch_ports::ThreadStore;
use serde_json::{Value, json};
use support::*;

async fn events(h: &Harness, job: &str) -> Vec<orch_core::Event> {
    h.store.list_events(thread(job), 0, 100).await.unwrap()
}

#[tokio::test]
async fn list_agents_gives_the_configured_agents_in_order() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    let out = call(&client, "list_agents", json!({})).await;
    assert!(!out.is_error);
    let ids: Vec<&str> = out.value["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["plain", "coder"]);
    assert_eq!(out.value["agents"][0]["name"], "Plain");
    assert_eq!(out.value["agents"][0]["description"], "scripted agent");
    // The result is text for the model as well as structured.
    assert!(out.text.contains("\"plain\""), "{}", out.text);
}

#[tokio::test]
async fn start_job_returns_at_once_and_the_log_says_it_came_from_mcp() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    let out = call(
        &client,
        "start_job",
        json!({"text": "gate please", "title": "A title"}),
    )
    .await;
    assert!(!out.is_error, "{out:?}");
    let job = out.value["job_id"].as_str().unwrap().to_owned();
    assert_eq!(out.value["created"], true);
    assert!(
        ["queued", "working"].contains(&out.value["state"].as_str().unwrap()),
        "{out:?}"
    );
    assert!(
        out.value.get("web_url").is_none(),
        "no public URL is configured"
    );

    // The default agent is the first configured one.
    let record = h
        .app
        .get_thread(&orch_core::UserId::new(ALICE), thread(&job))
        .await
        .unwrap();
    assert_eq!(record.target.agent_id, AgentId::new("plain"));
    assert_eq!(record.title, "A title");
    let log = events(&h, &job).await;
    assert_eq!(log[0].kind(), EventKind::UserMessage);
    let EventBody::UserMessage(m) = &log[0].body else {
        panic!("not a user message")
    };
    assert_eq!((m.text.as_str(), m.origin), ("gate please", Origin::Mcp));
    // The wire form spells it; a message from the chat leaves it out.
    assert_eq!(log[0].body.data_value()["origin"], "mcp");
    assert_eq!(log[0].actor.name, ALICE);

    h.agent.release_gate();
    wait_state(&client, &job, "done").await;
}

#[tokio::test]
async fn start_job_can_name_the_agent_and_refuses_an_unknown_one() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    let out = call(
        &client,
        "start_job",
        json!({"text": "echo hi", "agent": "coder"}),
    )
    .await;
    let job = out.value["job_id"].as_str().unwrap().to_owned();
    let record = h
        .app
        .get_thread(&orch_core::UserId::new(ALICE), thread(&job))
        .await
        .unwrap();
    assert_eq!(record.target.agent_id, AgentId::new("coder"));

    let out = call(
        &client,
        "start_job",
        json!({"text": "echo hi", "agent": "ghost"}),
    )
    .await;
    assert!(out.is_error);
    assert!(out.text.contains("unknown agent"), "{}", out.text);
    assert_eq!(
        h.threads_of(ALICE).await.len(),
        1,
        "the refused call wrote nothing"
    );
}

#[tokio::test]
async fn start_job_refuses_bad_arguments_before_writing_anything() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    // Text the App refuses.
    for text in ["", "   \n"] {
        let out = call(&client, "start_job", json!({"text": text})).await;
        assert!(out.is_error, "{text:?}");
        assert!(out.text.contains("text must not be empty"), "{}", out.text);
    }
    let out = call(
        &client,
        "start_job",
        json!({"text": "hi", "client_request_id": ""}),
    )
    .await;
    assert!(out.is_error);
    let out = call(
        &client,
        "start_job",
        json!({"text": "hi", "client_request_id": "x".repeat(257)}),
    )
    .await;
    assert!(out.is_error);
    assert!(out.text.contains("at most 256"), "{}", out.text);

    // A protocol error for arguments that are not the tool's: unknown members and wrong types,
    // a gate that is not `{require?, maxAttempts?}` included, none silently dropped.
    for args in [
        json!({"text": "hi", "gate": {"retries": 3}}),
        json!({"text": "hi", "gate": {"require": "agent-checks"}}),
        json!({"text": "hi", "surprise": true}),
        json!({"text": 7}),
        json!({}),
    ] {
        let err = try_call(&client, "start_job", args.clone())
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid arguments"),
            "{args}: {err}"
        );
    }
    let err = try_call(&client, "no_such_tool", json!({}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown tool"), "{err}");
    h.assert_nothing_written().await;
}

#[tokio::test]
async fn a_retried_start_job_is_the_same_job() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    let args = json!({"text": "gate once", "client_request_id": "req-1"});
    let first = call(&client, "start_job", args.clone()).await;
    let second = call(&client, "start_job", args.clone()).await;
    assert!(!first.is_error && !second.is_error, "{first:?} {second:?}");
    assert_eq!(first.value["job_id"], second.value["job_id"]);
    assert_eq!(
        (&first.value["created"], &second.value["created"]),
        (&json!(true), &json!(false))
    );
    // One thread, one message, one delivery.
    assert_eq!(h.threads_of(ALICE).await.len(), 1);
    let job = first.value["job_id"].as_str().unwrap();
    assert_eq!(
        events(&h, job)
            .await
            .iter()
            .filter(|e| e.kind() == EventKind::UserMessage)
            .count(),
        1
    );
    wait_state(&client, job, "working").await;
    assert_eq!(h.agent.sends().len(), 1);

    // A retry may leave out what it does not want to change, but not say something else.
    let third = call(
        &client,
        "start_job",
        json!({"text": "something else", "client_request_id": "req-1"}),
    )
    .await;
    assert!(third.is_error, "{third:?}");
    assert!(third.text.contains("the text"), "{}", third.text);
    assert!(third.text.contains("client_request_id"), "{}", third.text);
    assert_eq!(h.threads_of(ALICE).await.len(), 1);
    assert_eq!(h.agent.sends().len(), 1, "no second delivery");

    // Another id is another job; no id is never a retry; another user's same id is theirs.
    let other = call(
        &client,
        "start_job",
        json!({"text": "gate two", "client_request_id": "req-2"}),
    )
    .await;
    assert_ne!(other.value["job_id"], first.value["job_id"]);
    let a = call(&client, "start_job", json!({"text": "gate a"})).await;
    let b = call(&client, "start_job", json!({"text": "gate a"})).await;
    assert_ne!(a.value["job_id"], b.value["job_id"]);
    let bob = h.client(BOB_TOKEN).await;
    let theirs = call(&bob, "start_job", args).await;
    assert_ne!(theirs.value["job_id"], first.value["job_id"]);
    assert_eq!(h.threads_of(BOB).await.len(), 1);
    h.agent.release_gate();
}

#[tokio::test]
async fn concurrent_starts_with_one_request_id_make_one_job() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    let args = json!({"text": "echo race", "client_request_id": "race"});
    let calls = (0..6).map(|_| call(&client, "start_job", args.clone()));
    let outs = futures_join(calls).await;
    let ids: std::collections::HashSet<&str> = outs
        .iter()
        .map(|o| o.value["job_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 1, "{outs:?}");
    assert_eq!(
        outs.iter().filter(|o| o.value["created"] == true).count(),
        1,
        "exactly one of them created it"
    );
    assert_eq!(h.threads_of(ALICE).await.len(), 1);
}

async fn futures_join(
    calls: impl Iterator<Item = impl std::future::Future<Output = Outcome>>,
) -> Vec<Outcome> {
    let handles: Vec<_> = calls.map(Box::pin).collect();
    futures::future::join_all(handles).await
}

#[tokio::test]
async fn web_url_is_given_when_the_public_url_is_configured() {
    let h = Harness::start_with(Options {
        public_url: Some("https://chat.example.com/"),
        ..Options::default()
    })
    .await;
    let client = h.client(ALICE_TOKEN).await;
    let out = call(&client, "start_job", json!({"text": "echo hi"})).await;
    let job = out.value["job_id"].as_str().unwrap();
    assert_eq!(
        out.value["web_url"],
        format!("https://chat.example.com/threads/{job}")
    );
}

#[tokio::test]
async fn get_job_follows_a_job_to_done_and_shows_what_the_agent_made() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    let job = start(&client, "gate now").await;
    let working = wait_state(&client, &job, "working").await;
    assert_eq!(working["finished"], false);
    assert_eq!(working["agent"], "plain");
    assert!(working.get("pull_request").is_none());

    // The agent reports a pull request while it works.
    h.app
        .apply(
            thread(&job),
            Input::Agent {
                agent: AgentId::new("plain"),
                revision: None,
                update: AgentUpdate::Artifact {
                    name: "pull_request".to_owned(),
                    mime_type: None,
                    uri: None,
                    text: Some(r#"{"url":"https://github.com/acme/demo/pull/7"}"#.to_owned()),
                },
            },
            None,
            None,
            None,
        )
        .await
        .unwrap();
    h.agent.release_gate();
    let done = wait_state(&client, &job, "done").await;
    assert_eq!(done["finished"], true);
    assert_eq!(
        done["pull_request"]["url"],
        "https://github.com/acme/demo/pull/7"
    );
    // The cursor for wait_for_job's `after_seq` is the last event.
    assert_eq!(done["last_seq"], events(&h, &job).await.last().unwrap().seq);
}

#[tokio::test]
async fn a_job_of_someone_else_is_not_found_never_forbidden() {
    let h = Harness::start().await;
    let alice = h.client(ALICE_TOKEN).await;
    let bob = h.client(BOB_TOKEN).await;
    let job = start(&alice, "gate secret").await;
    wait_state(&alice, &job, "working").await;
    let before = events(&h, &job).await.len();

    let unknown = "00000000-0000-7000-8000-00000000ffff";
    for (tool, extra) in [
        ("get_job", json!({})),
        ("wait_for_job", json!({"timeout_secs": 1, "after_seq": 0})),
        ("answer", json!({"text": "hello"})),
        ("cancel_job", json!({})),
    ] {
        let mut args = extra;
        args["job_id"] = json!(job);
        let theirs = call(&bob, tool, args.clone()).await;
        // The same answer as for a job that does not exist, or an id that is no id.
        for missing in [unknown, "not-a-uuid"] {
            args["job_id"] = json!(missing);
            let nothing = call(&bob, tool, args.clone()).await;
            assert!(theirs.is_error && nothing.is_error, "{tool}");
            assert_eq!(theirs.text, nothing.text, "{tool}");
            assert_eq!(theirs.text, "no such job");
        }
    }
    // Nothing happened to Alice's job.
    assert_eq!(events(&h, &job).await.len(), before);
    let still = call(&alice, "get_job", json!({"job_id": job})).await;
    assert_eq!(still.value["state"], "working");
    h.agent.release_gate();
}

#[tokio::test]
async fn answer_continues_a_blocked_job_and_says_it_came_from_mcp() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    let job = start(&client, "ask me").await;
    wait_state(&client, &job, "blocked").await;

    let out = call(&client, "answer", json!({"job_id": job, "text": "main"})).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.value["job_id"], job.as_str());
    assert_eq!(out.value["state"], "queued");
    wait_state(&client, &job, "done").await;

    let messages: Vec<_> = events(&h, &job)
        .await
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::UserMessage(m) => Some((m.text, m.origin)),
            _ => None,
        })
        .collect();
    assert_eq!(
        messages,
        [
            ("ask me".to_owned(), Origin::Mcp),
            ("main".to_owned(), Origin::Mcp)
        ]
    );

    // A message to a finished job is refused, as in the chat, and writes nothing.
    let count = events(&h, &job).await.len();
    let late = call(&client, "answer", json!({"job_id": job, "text": "more"})).await;
    assert!(late.is_error);
    assert!(late.text.contains("finished"), "{}", late.text);
    assert_eq!(events(&h, &job).await.len(), count);
    // Empty text is refused too.
    let empty = call(&client, "answer", json!({"job_id": job, "text": " "})).await;
    assert!(empty.is_error);
}

#[tokio::test]
async fn cancel_job_stops_a_running_job_and_is_a_no_op_on_a_finished_one() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;

    let running = start(&client, "slow forever").await;
    wait_state(&client, &running, "working").await;
    let out = call(&client, "cancel_job", json!({"job_id": running})).await;
    assert!(!out.is_error, "{out:?}");
    wait_state(&client, &running, "cancelled").await;
    // Again: nothing to cancel any more, and nothing is written.
    let count = events(&h, &running).await.len();
    let again = call(&client, "cancel_job", json!({"job_id": running})).await;
    assert!(!again.is_error, "{again:?}");
    assert_eq!(again.value["state"], "cancelled");
    assert_eq!(again.value["finished"], true);
    assert_eq!(events(&h, &running).await.len(), count);

    let finished = start(&client, "echo quick").await;
    wait_state(&client, &finished, "done").await;
    let count = events(&h, &finished).await.len();
    let out = call(&client, "cancel_job", json!({"job_id": finished})).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.value["state"], "done");
    assert_eq!(events(&h, &finished).await.len(), count);
}

#[tokio::test]
async fn a_client_that_negotiates_the_stateless_lifecycle_works_too() {
    use rmcp::model::ProtocolVersion;
    use rmcp::transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    };
    use rmcp::{ClientLifecycleMode, ClientServiceExt};

    let h = Harness::start().await;
    let config =
        StreamableHttpClientTransportConfig::with_uri(h.mcp_url.clone()).auth_header(ALICE_TOKEN);
    let transport = StreamableHttpClientTransport::from_config(config);
    let client = Progress::default()
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover {
                preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            },
        )
        .await
        .expect("a discover handshake with no session");
    let out = call(&client, "start_job", json!({"text": "echo hi"})).await;
    assert!(!out.is_error, "{out:?}");
    let job = out.value["job_id"].as_str().unwrap();
    wait_state(&client, job, "done").await;
    let _: Value = out.value;
}

#[tokio::test]
async fn a_retry_that_says_something_else_is_refused_not_answered_with_the_first_job() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    let first = call(
        &client,
        "start_job",
        json!({"text": "echo one", "client_request_id": "same", "title": "One", "agent": "plain"}),
    )
    .await;
    assert!(!first.is_error, "{first:?}");
    let job = first.value["job_id"].as_str().unwrap().to_owned();
    let before = events(&h, &job).await.len();

    for (what, args) in [
        (
            "the text",
            json!({"text": "echo two", "client_request_id": "same"}),
        ),
        (
            "the agent",
            json!({"text": "echo one", "client_request_id": "same", "agent": "coder"}),
        ),
        (
            "the title",
            json!({"text": "echo one", "client_request_id": "same", "title": "Two"}),
        ),
    ] {
        let out = call(&client, "start_job", args).await;
        assert!(out.is_error, "{what}: {out:?}");
        assert!(out.text.contains(what), "{what}: {}", out.text);
        assert!(out.text.contains("new client_request_id"), "{}", out.text);
    }
    // The same request, and a retry that leaves out the optional parts, are the job.
    for args in [
        json!({"text": "echo one", "client_request_id": "same", "title": "One", "agent": "plain"}),
        json!({"text": "echo one", "client_request_id": "same"}),
    ] {
        let out = call(&client, "start_job", args).await;
        assert!(!out.is_error, "{out:?}");
        assert_eq!(out.value["job_id"], first.value["job_id"]);
        assert_eq!(out.value["created"], false);
    }
    assert_eq!(h.threads_of(ALICE).await.len(), 1);
    assert_eq!(events(&h, &job).await.len(), before, "nothing was added");
}

#[tokio::test]
async fn start_job_takes_a_gate_and_refuses_one_it_cannot_honour() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    let out = call(
        &client,
        "start_job",
        json!({"text": "echo gated", "gate": {"require": ["agent-checks"], "maxAttempts": 2}}),
    )
    .await;
    assert!(!out.is_error, "{out:?}");
    let threads = h.threads_of(ALICE).await;
    assert_eq!(threads.len(), 1);
    let gate = &threads[0].job.gate;
    assert!(
        gate.require.contains(&orch_core::CheckSource::AgentChecks),
        "{gate:?}"
    );
    assert_eq!(gate.max_attempts, 2);

    // Without a gate the deployment's applies.
    let plain = call(&client, "start_job", json!({"text": "echo plain"})).await;
    assert!(!plain.is_error, "{plain:?}");
    let threads = h.threads_of(ALICE).await;
    assert!(
        threads
            .iter()
            .find(|t| t.id.to_string() == plain.value["job_id"].as_str().unwrap())
            .unwrap()
            .job
            .gate
            .require
            .is_empty()
    );

    // A gate that cannot be honoured is a tool error naming why, and nothing is written.
    let written = h.threads_of(ALICE).await.len();
    for (gate, reason) in [
        (json!({"require": ["ci"]}), "ci"),
        (json!({"require": ["verifier"]}), "verifier"),
        (json!({"require": ["magic"]}), "magic"),
        (json!({"maxAttempts": 0}), "attempts"),
        (json!({"maxAttempts": 100_000}), "attempts"),
    ] {
        let out = try_call(
            &client,
            "start_job",
            json!({"text": "echo no", "gate": gate}),
        )
        .await;
        let out = out.unwrap_or_else(|e| panic!("{gate}: {e}"));
        assert!(out.is_error, "{gate}: {out:?}");
        assert!(out.text.contains("gate"), "{gate}: {}", out.text);
        assert!(
            out.text.to_lowercase().contains(reason),
            "{gate}: {}",
            out.text
        );
    }
    assert_eq!(h.threads_of(ALICE).await.len(), written);
}

#[tokio::test]
async fn a_retry_with_another_gate_is_refused_and_one_with_the_same_gate_is_the_job() {
    let h = Harness::start().await;
    let client = h.client(ALICE_TOKEN).await;
    let args = |gate: Value| json!({"text": "echo g", "client_request_id": "gated", "gate": gate});
    let first = call(
        &client,
        "start_job",
        args(json!({"require": ["agent-checks"], "maxAttempts": 2})),
    )
    .await;
    assert!(!first.is_error, "{first:?}");

    // The same gate, or no gate at all, is a retry.
    let same = call(
        &client,
        "start_job",
        args(json!({"require": ["agent-checks"], "maxAttempts": 2})),
    )
    .await;
    assert_eq!(same.value["job_id"], first.value["job_id"], "{same:?}");
    let none = call(
        &client,
        "start_job",
        json!({"text": "echo g", "client_request_id": "gated"}),
    )
    .await;
    assert_eq!(none.value["job_id"], first.value["job_id"], "{none:?}");

    // A different gate would be a different job: refused, and the gate stays what it was.
    let other = call(&client, "start_job", args(json!({"maxAttempts": 3}))).await;
    assert!(other.is_error, "{other:?}");
    assert!(other.text.contains("the gate"), "{}", other.text);
    let threads = h.threads_of(ALICE).await;
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].job.gate.max_attempts, 2);
}

#[tokio::test]
async fn a_job_id_that_another_users_thread_has_is_a_clear_error_and_leaks_nothing() {
    use orch_app::{Inbound, NewThread};
    use orch_core::{AgentTarget, UserId};

    let h = Harness::start().await;
    // Bob's own thread sits on the id Alice's request would derive (only a thread made before
    // the ids were reserved, or a hash collision, can).
    let id = orch_surface_mcp::job_id_for(&UserId::new(ALICE), "taken");
    h.app
        .create_thread_as(
            &UserId::new(BOB),
            id,
            NewThread {
                title: Some("bob's secret".to_owned()),
                target: AgentTarget {
                    agent_id: AgentId::new("plain"),
                    release: None,
                },
                text: "gate bob".to_owned(),
            },
            Inbound::default(),
        )
        .await
        .unwrap();

    let client = h.client(ALICE_TOKEN).await;
    let out = tokio::time::timeout(
        T,
        call(
            &client,
            "start_job",
            json!({"text": "echo mine", "client_request_id": "taken"}),
        ),
    )
    .await
    .expect("no hang");
    assert!(out.is_error, "{out:?}");
    assert!(out.text.contains("client_request_id"), "{}", out.text);
    assert!(
        !out.text.contains("secret") && !out.text.contains("bob"),
        "{}",
        out.text
    );
    assert!(out.value.is_null());
    assert!(h.threads_of(ALICE).await.is_empty());
    // Bob's thread is untouched: still the one message.
    let bobs = h.store.list_events(id, 0, 100).await.unwrap();
    assert_eq!(
        bobs.iter()
            .filter(|e| e.kind() == EventKind::UserMessage)
            .count(),
        1
    );
    h.agent.release_gate();
}
