//! ADR 0087's **presence at intake** on the cloud path: a prompt from a signing client
//! (`auth.signingClients`) must carry `metadata["vymalo.promptSignature"]`, and an unsigned or
//! malformed one is a `403 prompt_signature_invalid` before anything is written or any agent is
//! called. A client that is not listed, and a deployment that lists none, is never checked.
//!
//! The signature is **never verified here** (that is the verifier's, behind the model gateway) and
//! never written to the log: the tests below pin the refusal, the pass-through byte for byte, and
//! the absence from the events, the export and every `tracing` line.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::io::Write;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use support::*;
use tracing_subscriber::fmt::MakeWriter;

/// A compact JWS of three base64url parts with a **detached** payload: what a signing app sends.
const ENVELOPE: &str =
    "eyJhbGciOiJFZERTQSIsImtpZCI6ImRldmljZS0xI3YxIn0..c2lnbmF0dXJlLW9mLXRoZS1wcm9tcHQ";

fn signed(thread: &str, run: &str, id: &str, text: &str, envelope: &str) -> Value {
    json!({"threadId": thread, "runId": run, "messages": [{
        "id": id, "role": "user", "content": text,
        "metadata": {"vymalo.promptSignature": envelope}
    }]})
}

async fn no_thread(h: &Harness, id: &str) {
    let r = h.get(&format!("/api/threads/{id}"), Some(ALICE)).await;
    assert_eq!(r.status, 404, "the refused request wrote a thread");
}

/// A `tracing` writer that keeps every line in one buffer.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap().clone()).to_string()
    }
}

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

#[tokio::test]
async fn a_signing_clients_unsigned_prompt_is_refused_and_nothing_is_written() {
    let h = Harness::start_signing(&["desktop"]).await;
    let thread = new_thread_id();
    let r = h
        .refused_as_client(
            "plain",
            ALICE,
            "desktop",
            &input(&thread, "r", &[("m", "echo hi")]),
        )
        .await;
    let p = r.problem(403);
    assert_eq!(p["code"], "prompt_signature_invalid", "{p}");
    assert_eq!(p["reason"], "invalid", "{p}");
    // The person is told what ADR 0087 says, and the body holds no envelope and no finer reason.
    assert!(
        p["detail"]
            .as_str()
            .unwrap()
            .contains("couldn't be verified"),
        "{p}"
    );
    no_thread(&h, &thread).await;
    assert!(
        h.agent.sends().is_empty(),
        "an agent was sent a refused prompt"
    );
}

#[tokio::test]
async fn a_malformed_envelope_is_refused_too() {
    let h = Harness::start_signing(&["desktop"]).await;
    let too_long = format!("a..{}", "c".repeat(16 * 1024));
    for (why, envelope) in [
        ("two parts", "a.b".to_string()),
        ("four parts", "a.b.c.d".to_string()),
        ("an empty header", ".b.c".to_string()),
        ("an empty signature", "a.b.".to_string()),
        ("not base64url", "a+b.c.d".to_string()),
        ("over 16 KiB", too_long),
    ] {
        let thread = new_thread_id();
        let r = h
            .refused_as_client(
                "plain",
                ALICE,
                "desktop",
                &signed(&thread, "r", "m", "echo hi", &envelope),
            )
            .await;
        let p = r.problem(403);
        assert_eq!(p["code"], "prompt_signature_invalid", "{why}: {p}");
        assert_eq!(p["reason"], "invalid", "{why}: {p}");
        assert!(!p.to_string().contains("a+b"), "{why}: {p}");
        no_thread(&h, &thread).await;
    }
    // A member that is not a string is not an envelope either: fail closed.
    let thread = new_thread_id();
    let mut body = input(&thread, "r", &[("m", "echo hi")]);
    body["messages"][0]["metadata"] = json!({"vymalo.promptSignature": 5});
    let p = h
        .refused_as_client("plain", ALICE, "desktop", &body)
        .await
        .problem(403);
    assert_eq!(p["code"], "prompt_signature_invalid", "{p}");
    no_thread(&h, &thread).await;
    assert!(h.agent.sends().is_empty());
}

#[tokio::test]
async fn a_signed_prompt_is_accepted_and_reaches_the_agent_byte_for_byte() {
    let h = Harness::start_signing(&["desktop"]).await;
    let thread = new_thread_id();
    let stream = h
        .run_as_client(
            "plain",
            ALICE,
            "desktop",
            &signed(&thread, "r", "m", "echo hi", ENVELOPE),
        )
        .await;
    let frames = stream.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    // The adapter's request carries the very bytes the client sent: no decode, no re-encode.
    let sent: Vec<Option<String>> = h
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            orch_ports::memory::Call::Send(call) => Some(call.prompt_signature.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [Some(ENVELOPE.to_owned())]);
}

#[tokio::test]
async fn a_client_that_is_not_listed_is_never_checked() {
    // The negative control: the list names `desktop`, the request comes from `web-client` (which the
    // web would), unsigned, and it is served exactly as before.
    let h = Harness::start_signing(&["desktop"]).await;
    let thread = new_thread_id();
    let stream = h
        .run_as_client(
            "plain",
            ALICE,
            "web-client",
            &input(&thread, "r", &[("m", "echo hi")]),
        )
        .await;
    let frames = stream.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    assert_eq!(h.thread(ALICE, &thread).await["state"], "done");
    let sent: Vec<Option<String>> = h
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            orch_ports::memory::Call::Send(call) => Some(call.prompt_signature.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [None]);
}

#[tokio::test]
async fn a_deployment_that_lists_no_signing_client_asks_nobody() {
    // The default list is empty: even a token that names a client of ours is never asked, and a
    // message without an envelope is served.
    let h = Harness::start().await;
    let thread = new_thread_id();
    let stream = h
        .run_as_client(
            "plain",
            ALICE,
            "desktop",
            &input(&thread, "r", &[("m", "echo hi")]),
        )
        .await;
    let frames = stream.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
}

#[tokio::test]
async fn no_word_of_the_envelope_is_in_the_log_or_in_a_tracing_line() {
    // The decisive one (ADR 0087, decision 2 and the card): the envelope goes to the agent and is
    // nowhere else. A subscriber of its own catches every line of this process, the export is the
    // document a person downloads, and the events are what every frame is folded from.
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("no other global subscriber");

    let h = Harness::start_signing(&["desktop"]).await;
    let thread = new_thread_id();
    let stream = h
        .run_as_client(
            "plain",
            ALICE,
            "desktop",
            &signed(&thread, "r", "m", "echo hi", ENVELOPE),
        )
        .await;
    let frames = stream.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    h.wait_state(ALICE, &thread, "done").await;

    // the export: the thread row (with its ledger) and every event
    let export = h
        .get(&format!("/api/threads/{thread}/export"), Some(ALICE))
        .await;
    assert_eq!(export.status, 200);
    let export = String::from_utf8_lossy(&export.body).to_string();
    assert!(!export.contains(ENVELOPE), "the envelope is in the export");
    assert!(
        !export.contains("promptSignature") && !export.contains("prompt_signature"),
        "the export names the envelope"
    );
    // the frames of the run
    let frames: Vec<Value> = frames.into_iter().map(|frame| frame.event).collect();
    let frames = serde_json::to_string(&frames).unwrap();
    assert!(!frames.contains(ENVELOPE), "the envelope is in a frame");
    // and every line the process wrote
    let log = capture.text();
    assert!(
        !log.is_empty(),
        "nothing was captured: the test proves nothing"
    );
    assert!(!log.contains(ENVELOPE), "the envelope is in a log line");
}

/// A body of the transcript `messages` plus `resume` entries, as a consumer sends them.
fn with_resume(thread: &str, run: &str, messages: &[(&str, &str)], resume: Value) -> Value {
    input_with(thread, run, messages, json!({"resume": resume}))
}

/// A thread the agent has asked a question on (`blocked`, interrupt `int-3`), started with a
/// **signed** first message: `Ok(thread)` as the signing client, `Err(problem)` when the run was
/// refused.
async fn blocked(h: &Harness, thread: &str) -> Stream {
    let stream = h
        .run_as_client(
            "plain",
            ALICE,
            "desktop",
            &signed(thread, "run-1", "m-1", "ask about branches", ENVELOPE),
        )
        .await;
    stream.all().await;
    assert_eq!(h.state(ALICE, thread).await, "blocked");
    // an attach of that run, which is not a prompt: no messages at all
    h.run_as_client(
        "plain",
        ALICE,
        "desktop",
        &json!({
            "threadId": thread, "runId": "run-1", "messages": []
        }),
    )
    .await
}

#[tokio::test]
async fn a_signing_clients_attach_and_cancel_are_never_refused() {
    // B1 (the security review of 2026-11-28): a run that writes nothing the person said is not a
    // prompt, so it is never refused for a missing signature.
    let h = Harness::start_signing(&["desktop"]).await;
    let thread = new_thread_id();
    // the attach is inside `blocked`: its stream ends at the run's terminal event
    let mut attach = blocked(&h, &thread).await;
    let frames = attach
        .until(|f| matches!(f.kind(), "RUN_FINISHED" | "RUN_ERROR"))
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");

    // a cancel (a `resume` that is `cancelled`) writes no user message either
    let cancel = h
        .post_as_client(
            "plain",
            ALICE,
            "desktop",
            &with_resume(
                &thread,
                "run-2",
                &[("m-1", "ask about branches")],
                json!([{"interruptId": "int-3", "status": "cancelled"}]),
            ),
        )
        .await;
    assert_eq!(
        cancel.status().as_u16(),
        200,
        "a signing client's cancel is served"
    );
    let frames = Stream::new(cancel)
        .until(|f| matches!(f.kind(), "RUN_FINISHED" | "RUN_ERROR"))
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
}

#[tokio::test]
async fn a_signing_clients_resolved_resume_is_checked_at_its_entry() {
    // ADR 0087, decision 3: the envelope of an answer rides the `resume` entry, not the transcript.
    let h = Harness::start_signing(&["desktop"]).await;

    // with it: served, and the agent is sent the answer
    let thread = new_thread_id();
    let mut attach = blocked(&h, &thread).await;
    let _ = attach
        .until(|f| matches!(f.kind(), "RUN_FINISHED" | "RUN_ERROR"))
        .await;
    let answer = h
        .post_as_client(
            "plain",
            ALICE,
            "desktop",
            &with_resume(
                &thread,
                "run-2",
                &[("m-1", "ask about branches")],
                json!([{
                    "interruptId": "int-3",
                    "status": "resolved",
                    "payload": {"text": "main"},
                    "metadata": {"vymalo.promptSignature": ENVELOPE},
                }]),
            ),
        )
        .await;
    assert_eq!(
        answer.status().as_u16(),
        200,
        "an answer that carries its entry's envelope is served"
    );
    let frames = Stream::new(answer)
        .until(|f| matches!(f.kind(), "RUN_FINISHED" | "RUN_ERROR"))
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    // the answer reached the agent with the envelope of its entry, byte for byte
    let sent: Vec<Option<String>> = h
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            orch_ports::memory::Call::Send(call) => Some(call.prompt_signature.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent.last(), Some(&Some(ENVELOPE.to_owned())));

    // without it: refused before anything is written, and the thread stays blocked
    let other = new_thread_id();
    let mut attach = blocked(&h, &other).await;
    let _ = attach
        .until(|f| matches!(f.kind(), "RUN_FINISHED" | "RUN_ERROR"))
        .await;
    let refused = h
        .refused_as_client(
            "plain",
            ALICE,
            "desktop",
            &with_resume(
                &other,
                "run-2",
                &[("m-1", "ask about branches")],
                json!([{"interruptId": "int-3", "status": "resolved", "payload": {"text": "main"}}]),
            ),
        )
        .await;
    let p = refused.problem(403);
    assert_eq!(p["code"], "prompt_signature_invalid", "{p}");
    assert_eq!(p["reason"], "invalid", "{p}");
    assert_eq!(
        h.state(ALICE, &other).await,
        "blocked",
        "nothing was written"
    );
}

#[tokio::test]
async fn a_signing_clients_surface_action_is_never_refused() {
    // An answer to a surface is open question 3 of ADR 0087: until the owner rules, it is accepted
    // unsigned, and it writes no message of the person's.
    let h = Harness::start_signing(&["desktop"]).await;
    let thread = new_thread_id();
    let frames = h
        .run_as_client(
            "plain",
            ALICE,
            "desktop",
            &signed(&thread, "run-1", "m-1", "ui pick one", ENVELOPE),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().event["outcome"]["type"], "interrupt");

    let action = h
        .post_as_client(
            "plain",
            ALICE,
            "desktop",
            &input_with(
                &thread,
                "run-2",
                &[],
                json!({"forwardedProps": {"a2uiAction": {"userAction": {
                    "name": "go", "surfaceId": "s1", "sourceComponentId": "go",
                    "context": {"choice": "a"}, "timestamp": "2026-09-29T10:00:00Z"
                }}}}),
            ),
        )
        .await;
    assert_eq!(action.status().as_u16(), 200, "a surface action is served");
    let frames = Stream::new(action)
        .until(|f| matches!(f.kind(), "RUN_FINISHED" | "RUN_ERROR"))
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
}

#[tokio::test]
async fn a_token_that_names_no_client_is_a_signing_clients() {
    // ADR 0087, decision 1 fails closed: a token of the issuer that names neither `azp` nor
    // `client_id` (the `Some("")` sentinel) is treated as a signing client's.
    let h = Harness::start_signing(&["desktop"]).await;
    let thread = new_thread_id();
    let r = h
        .refused_as_client(
            "plain",
            ALICE,
            NO_CLIENT,
            &input(&thread, "r", &[("m", "echo hi")]),
        )
        .await;
    let p = r.problem(403);
    assert_eq!(p["code"], "prompt_signature_invalid", "{p}");
    no_thread(&h, &thread).await;
    assert!(h.agent.sends().is_empty());
}

#[tokio::test]
async fn a_credential_that_names_no_client_at_all_is_never_checked() {
    // The proxy header names no OAuth client (`Principal::client_id` is `None`): a signing deployment
    // asks it for nothing.
    let h = Harness::start_signing(&["desktop"]).await;
    let thread = new_thread_id();
    let stream = h
        .run("plain", ALICE, &input(&thread, "r", &[("m", "echo hi")]))
        .await;
    let frames = stream.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    assert_eq!(h.state(ALICE, &thread).await, "done");
}
