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
use std::sync::{Arc, Mutex, OnceLock};

use orch_ports::{AgentError, ThreadStore};
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

/// The envelope a send carried.
fn envelope_of(call: &orch_ports::memory::SendCall) -> Option<String> {
    call.prompt_signature
        .as_ref()
        .map(|signed| signed.envelope.as_str().to_owned())
}

/// The whole signed prompt of every send, oldest first.
fn signed_sends(h: &Harness) -> Vec<Option<orch_core::SignedPrompt>> {
    h.agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            orch_ports::memory::Call::Send(call) => Some(call.prompt_signature.clone()),
            _ => None,
        })
        .collect()
}

/// The shared test vectors of the signature contract, whose hashes an implementation of its own
/// made.
fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../prompt-statement/tests/vectors/prompt-signatures.json"
    ))
    .unwrap()
}

/// The claims a run's message should be sent with.
fn claims(
    thread: &str,
    agent: &str,
    msg: &str,
    content: &str,
    props: &str,
) -> orch_core::PromptClaims {
    orch_core::PromptClaims {
        aud: PUBLIC_ORIGIN.to_owned(),
        thread: thread.to_owned(),
        agent: agent.to_owned(),
        msg: msg.to_owned(),
        content_sha256: content.to_owned(),
        props_sha256: props.to_owned(),
    }
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

/// The lines of this process: one subscriber for the whole test binary (a second global one
/// cannot be set), shared by the tests that read it. It sees the other tests' lines too, which an
/// assertion that something is **absent** can only gain from.
fn captured() -> Capture {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    CAPTURE
        .get_or_init(|| {
            let capture = Capture::default();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(capture.clone())
                .with_max_level(tracing::Level::TRACE)
                .with_ansi(false)
                .finish();
            tracing::subscriber::set_global_default(subscriber)
                .expect("no other global subscriber");
            capture
        })
        .clone()
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
            orch_ports::memory::Call::Send(call) => Some(envelope_of(&call)),
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
            orch_ports::memory::Call::Send(call) => Some(envelope_of(&call)),
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
    let capture = captured();

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
            orch_ports::memory::Call::Send(call) => Some(envelope_of(&call)),
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

// --- Stop & send (ANR-1376): the held message keeps its envelope, and no export has it ------

/// A run of one new message `m2` that stops the job of a thread whose first message was `m1`.
fn stopping_run(thread: &str, run: &str, new: &[(&str, &str, &str)], send: &str) -> Value {
    let mut messages = vec![json!({"id": "m1", "role": "user", "content": "gate wait"})];
    messages.extend(new.iter().map(|(id, text, envelope)| {
        json!({"id": id, "role": "user", "content": text,
               "metadata": {"vymalo.promptSignature": envelope}})
    }));
    json!({"threadId": thread, "runId": run, "messages": messages,
           "forwardedProps": {"vymalo.send": send}})
}

async fn user_message_count(h: &Harness, thread: &str) -> usize {
    h.events(ALICE, thread)
        .await
        .iter()
        .filter(|e| e["kind"] == "user_message")
        .count()
}

#[tokio::test]
async fn a_stop_and_sends_envelope_waits_in_the_ledger_and_no_export_has_it() {
    /// This test's own envelopes, so that no other test's bytes can pass for them.
    const FIRST: &str = "eyJhbGciOiJFZERTQSJ9..Zmlyc3Q";
    const HELD: &str = "eyJhbGciOiJFZERTQSJ9..aGVsZC1ieS1hLXN0b3A";
    let capture = captured();
    let h = Harness::start_signing(&["desktop"]).await;
    let thread = new_thread_id();
    let mut open = h
        .run_as_client(
            "plain",
            ALICE,
            "desktop",
            &signed(&thread, "r1", "m1", "gate wait", FIRST),
        )
        .await;
    assert_eq!(open.next(T).await.unwrap().kind(), "RUN_STARTED");
    h.wait_state(ALICE, &thread, "working").await;
    // The agent cannot be asked to stop for now, so the stop stays pending until the gate is
    // released and the task ends.
    h.agent
        .fail_next_cancels(100, || AgentError::unreachable("the agent is busy"));
    let second = h
        .run_as_client(
            "plain",
            ALICE,
            "desktop",
            &stopping_run(
                &thread,
                "r2",
                &[("m2", "echo do X instead", HELD)],
                "interrupt",
            ),
        )
        .await;

    // **During** the stop: afterwards the ledger's member is empty, and an export would be clean
    // without any redaction.
    let id = orch_core::ThreadId(thread.parse().unwrap());
    let stored = h.store.get_thread(None, id).await.unwrap().unwrap();
    assert_eq!(
        stored
            .job
            .after_stop_prompt_signature
            .as_ref()
            .map(|signed| signed.envelope.as_str()),
        Some(HELD),
        "the ledger holds it"
    );
    let export = h
        .get(&format!("/api/threads/{thread}/export"), Some(ALICE))
        .await;
    assert_eq!(export.status, 200);
    let document = export.json();
    assert_eq!(
        document["job"]["afterStop"], "echo do X instead",
        "the export is of a thread that is stopping: {document}"
    );
    assert!(
        document["job"].get("afterStopPromptSignature").is_none(),
        "the export has the member"
    );
    let export = String::from_utf8_lossy(&export.body).to_string();
    assert!(!export.contains(HELD), "the envelope is in the export");
    // whatever the member were called: `afterStopPromptSignature` has a capital P
    assert!(
        !export.to_lowercase().contains("promptsignature") && !export.contains("prompt_signature"),
        "the export names the envelope"
    );
    let thread_resource = h.thread(ALICE, &thread).await.to_string();
    assert!(!thread_resource.contains(HELD), "{thread_resource}");

    // A second message while one is held: 409, signed like the first, as a Send or a Stop & send,
    // and nothing is written.
    let written = user_message_count(&h, &thread).await;
    for send in ["interrupt", "steer"] {
        let p = h
            .refused_as_client(
                "plain",
                ALICE,
                "desktop",
                &stopping_run(
                    &thread,
                    "r3",
                    &[("m3", "echo and Y", "eyJhbGciOiJFZERTQSJ9..dGhpcmQ")],
                    send,
                ),
            )
            .await
            .problem(409);
        assert_eq!(p["code"], "stop_pending", "{send}: {p}");
        assert!(!p.to_string().contains(HELD), "{p}");
    }
    assert_eq!(user_message_count(&h, &thread).await, written);
    let unchanged = h.store.get_thread(None, id).await.unwrap().unwrap();
    assert_eq!(unchanged.job.after_stop, stored.job.after_stop);
    assert_eq!(
        unchanged.job.after_stop_prompt_signature,
        stored.job.after_stop_prompt_signature
    );

    // The task ends: the held message starts the next job, with its own bytes.
    h.agent.release_gate();
    let mut frames = second.all().await;
    frames.extend(open.all().await);
    h.wait_state(ALICE, &thread, "done").await;
    let sent: Vec<(String, Option<String>)> = h
        .agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            orch_ports::memory::Call::Send(call) => Some((call.text.clone(), envelope_of(&call))),
            _ => None,
        })
        .collect();
    assert_eq!(
        sent,
        [
            ("gate wait".to_owned(), Some(FIRST.to_owned())),
            ("echo do X instead".to_owned(), Some(HELD.to_owned())),
        ]
    );
    let frames: Vec<Value> = frames.into_iter().map(|frame| frame.event).collect();
    let frames = serde_json::to_string(&frames).unwrap();
    assert!(frames.contains("RUN_FINISHED"), "no frames: {frames}");
    assert!(!frames.contains(HELD), "the envelope is in a frame");
    let events = serde_json::to_string(&h.events(ALICE, &thread).await).unwrap();
    assert!(events.contains("echo do X instead"), "no message: {events}");
    assert!(!events.contains(HELD), "the envelope is in an event");
    let log = capture.text();
    assert!(
        log.contains(&thread),
        "this thread's lines were not captured: the test proves nothing"
    );
    assert!(!log.contains(HELD), "the envelope is in a log line");
}

/// A Stop & send of a client that is **not listed**, with a value that is no envelope to carry
/// (`junk`: empty, or over the bound). The client is served, and the intake drops the value: the
/// job ledger never holds it, so the next message is joined to the held one and not refused.
async fn an_unlisted_clients_value_is_not_held(junk: &str) {
    let h = Harness::start_signing(&["desktop"]).await;
    let thread = new_thread_id();
    let mut open = h
        .run_as_client(
            "plain",
            ALICE,
            "web-client",
            &input(&thread, "r1", &[("m1", "gate wait")]),
        )
        .await;
    assert_eq!(open.next(T).await.unwrap().kind(), "RUN_STARTED");
    h.wait_state(ALICE, &thread, "working").await;
    // the stop stays pending: the agent cannot be asked to stop for now
    h.agent
        .fail_next_cancels(100, || AgentError::unreachable("the agent is busy"));
    let _held = h
        .run_as_client(
            "plain",
            ALICE,
            "web-client",
            &stopping_run(
                &thread,
                "r2",
                &[("m2", "echo do X instead", junk)],
                "interrupt",
            ),
        )
        .await;

    let id = orch_core::ThreadId(thread.parse().unwrap());
    let stored = h.store.get_thread(None, id).await.unwrap().unwrap();
    // the positive control: a message is held, so the empty member below says something
    assert_eq!(stored.job.after_stop.as_deref(), Some("echo do X instead"));
    // not `assert_eq!`: a failure must not print 16 KiB
    assert!(
        stored.job.after_stop_prompt_signature.is_none(),
        "the ledger holds an unlisted client's value"
    );

    // a second message, with such a value too: served (`run_as_client` wants a 200) and joined
    let _joined = h
        .run_as_client(
            "plain",
            ALICE,
            "web-client",
            &stopping_run(&thread, "r3", &[("m3", "echo and Y", junk)], "interrupt"),
        )
        .await;
    let joined = h.store.get_thread(None, id).await.unwrap().unwrap();
    assert_eq!(
        joined.job.after_stop.as_deref(),
        Some("echo do X instead\n\necho and Y")
    );
    assert!(
        joined.job.after_stop_prompt_signature.is_none(),
        "the ledger holds an unlisted client's value"
    );
}

#[tokio::test]
async fn an_unlisted_clients_empty_value_is_not_held_by_a_stop_and_send() {
    an_unlisted_clients_value_is_not_held("").await;
}

#[tokio::test]
async fn an_unlisted_clients_oversized_value_is_not_held_by_a_stop_and_send() {
    // one byte over the 16 KiB bound
    an_unlisted_clients_value_is_not_held(&"c".repeat(16 * 1024 + 1)).await;
}

// --- `startChildSession` (ANR-1383): the helper's first round is the person's words ------------

/// A thread of `client`'s on an agent that offers the helper `planner`, answered, so that it has a
/// context a session can be started in.
async fn parent_with_helpers(h: &Harness, client: &str) -> String {
    h.agent.set_child_sessions(
        "plain",
        Some(
            orch_core::ChildSessionsOffer::from_params(&json!({
                "agents": [{"name": "planner", "description": "Plans"}],
                "maxOpen": 16
            }))
            .unwrap(),
        ),
    );
    let thread = new_thread_id();
    let frames = h
        .run_as_client(
            "plain",
            ALICE,
            client,
            // an envelope of its own, so the helper's cannot be mistaken for the parent's
            &signed(&thread, "r", "m", "echo hi", "cGFyZW50..cGFyZW50"),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    h.wait_state(ALICE, &thread, "done").await;
    thread
}

const HELPER_PROMPT: &str = "echo list every banana";

fn helper_start(metadata: Option<Value>) -> Value {
    let mut body = json!({
        "id": "0190bbbb-0000-7000-8000-000000001383",
        "agent": "planner",
        "description": "Find the writers",
        "text": HELPER_PROMPT,
    });
    if let Some(metadata) = metadata {
        body["metadata"] = metadata;
    }
    body
}

/// The envelopes of the requests that carried the helper's prompt: one per start sent.
fn helper_sends(h: &Harness) -> Vec<Option<String>> {
    h.agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            orch_ports::memory::Call::Send(call) if call.text == HELPER_PROMPT => {
                Some(envelope_of(&call))
            }
            _ => None,
        })
        .collect()
}

/// The `child_session_start_ended` of `parent`, once there is one.
async fn start_ended(h: &Harness, parent: &str) -> Value {
    let deadline = tokio::time::Instant::now() + T;
    loop {
        let events = h.events(ALICE, parent).await;
        if let Some(e) = events
            .into_iter()
            .find(|e| e["kind"] == "child_session_start_ended")
        {
            return e;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the start did not end"
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_signing_clients_unsigned_helper_start_is_refused_and_nothing_is_written() {
    let h = Harness::start_signing(&["desktop"]).await;
    let parent = parent_with_helpers(&h, "desktop").await;
    let events = h.events(ALICE, &parent).await;
    let sends = h.agent.sends().len();
    for (why, metadata) in [
        ("no metadata", None),
        ("an empty metadata", Some(json!({}))),
        ("two parts", Some(json!({"vymalo.promptSignature": "a.b"}))),
        ("not text", Some(json!({"vymalo.promptSignature": 5}))),
    ] {
        let p = h
            .api_post_as_client(
                &format!("/api/threads/{parent}/sessions"),
                ALICE,
                "desktop",
                &helper_start(metadata),
            )
            .await
            .problem(403);
        assert_eq!(p["code"], "prompt_signature_invalid", "{why}: {p}");
        assert_eq!(p["reason"], "invalid", "{why}: {p}");
    }
    // and a token that names no client at all is a signing client's (fail closed)
    let p = h
        .api_post_as_client(
            &format!("/api/threads/{parent}/sessions"),
            ALICE,
            NO_CLIENT,
            &helper_start(None),
        )
        .await
        .problem(403);
    assert_eq!(p["code"], "prompt_signature_invalid", "{p}");
    // no `child_session_start_asked`, so no row to send and no helper
    assert_eq!(h.events(ALICE, &parent).await, events);
    assert_eq!(h.agent.sends().len(), sends);
}

#[tokio::test]
async fn an_unsigned_helper_start_is_refused_before_the_thread_or_the_card_is_looked_at() {
    // Each of these has an answer of its own for a signed request (404, 422 `unknown_helper`).
    // The 403 instead says the check runs before the application looks either of them up.
    let h = Harness::start_signing(&["desktop"]).await;
    let parent = parent_with_helpers(&h, "desktop").await;
    let events = h.events(ALICE, &parent).await;
    let sends = h.agent.sends().len();
    let mut unknown_helper = helper_start(None);
    unknown_helper["agent"] = json!("nobody");
    for (why, thread, body) in [
        (
            "a thread that does not exist",
            new_thread_id(),
            helper_start(None),
        ),
        (
            "a helper the card does not list",
            parent.clone(),
            unknown_helper,
        ),
    ] {
        let p = h
            .api_post_as_client(
                &format!("/api/threads/{thread}/sessions"),
                ALICE,
                "desktop",
                &body,
            )
            .await
            .problem(403);
        assert_eq!(p["code"], "prompt_signature_invalid", "{why}: {p}");
        assert_eq!(p["reason"], "invalid", "{why}: {p}");
    }
    assert_eq!(h.events(ALICE, &parent).await, events);
    assert_eq!(h.agent.sends().len(), sends);
}

#[tokio::test]
async fn a_signed_helper_start_opens_the_session_with_the_same_bytes_and_logs_none_of_them() {
    let h = Harness::start_signing(&["desktop"]).await;
    let parent = parent_with_helpers(&h, "desktop").await;
    let r = h
        .api_post_as_client(
            &format!("/api/threads/{parent}/sessions"),
            ALICE,
            "desktop",
            &helper_start(Some(json!({"vymalo.promptSignature": ENVELOPE}))),
        )
        .await;
    assert_eq!(r.status, 202, "{}", String::from_utf8_lossy(&r.body));
    let ended = start_ended(&h, &parent).await;
    assert_eq!(ended["data"]["outcome"], "opened", "{ended}");
    assert_eq!(helper_sends(&h), [Some(ENVELOPE.to_owned())]);
    // The envelope and nothing else (ADR 0087): the contract does not say what a helper start
    // signs, so no claim is made up, although this deployment has a public origin. The parent's
    // own run, sent before it, has its claims.
    let sends = signed_sends(&h);
    let (helper, runs): (Vec<_>, Vec<_>) = sends
        .into_iter()
        .flatten()
        .partition(|signed| signed.envelope.as_str() == ENVELOPE);
    assert_eq!(
        helper,
        [orch_core::SignedPrompt::envelope_only(ENVELOPE.to_owned())]
    );
    assert!(
        runs.iter().all(|signed| signed.claims.is_some()),
        "{runs:?}"
    );
    assert!(!runs.is_empty());

    // neither the parent's export nor the helper's, whose log holds the person's message
    let child = ended["data"]["thread"].as_str().unwrap().to_owned();
    h.wait_state(ALICE, &child, "done").await;
    for thread in [&parent, &child] {
        let export = h
            .get(&format!("/api/threads/{thread}/export"), Some(ALICE))
            .await;
        assert_eq!(export.status, 200);
        let export = String::from_utf8_lossy(&export.body).to_string();
        assert!(!export.contains(ENVELOPE), "the envelope is in an export");
        assert!(
            !export.contains("promptSignature") && !export.contains("prompt_signature"),
            "an export names the envelope"
        );
        let events = h.events(ALICE, thread).await;
        // the positive control: the absences below are not of empty answers. The prompt is the
        // helper's log alone.
        if thread == &child {
            assert!(export.contains(HELPER_PROMPT), "the export has no prompt");
            assert!(
                events
                    .iter()
                    .any(|e| e["kind"] == "user_message" && e["data"]["text"] == HELPER_PROMPT),
                "the events have no prompt: {events:?}"
            );
        }
        let events = serde_json::to_string(&events).unwrap();
        assert!(!events.contains(ENVELOPE), "the envelope is in an event");
        // the frames every viewer of the thread is sent, folded from the whole log
        let history = h
            .get(&format!("/agui/threads/{thread}/history"), Some(ALICE))
            .await;
        assert_eq!(history.status, 200);
        let history = String::from_utf8_lossy(&history.body).to_string();
        if thread == &child {
            assert!(history.contains(HELPER_PROMPT), "the history has no prompt");
        }
        assert!(!history.contains(ENVELOPE), "the envelope is in a frame");
    }
}

#[tokio::test]
async fn a_client_that_is_not_listed_starts_a_helper_unsigned() {
    let h = Harness::start_signing(&["desktop"]).await;
    let parent = parent_with_helpers(&h, "web-client").await;
    let r = h
        .api_post_as_client(
            &format!("/api/threads/{parent}/sessions"),
            ALICE,
            "web-client",
            &helper_start(None),
        )
        .await;
    assert_eq!(r.status, 202, "{}", String::from_utf8_lossy(&r.body));
    let ended = start_ended(&h, &parent).await;
    assert_eq!(ended["data"]["outcome"], "opened", "{ended}");
    assert_eq!(helper_sends(&h), [None]);
}

// --- the claims (ANR-1422): what only the request could say, computed at intake ----------------

#[tokio::test]
async fn every_claim_of_a_run_reaches_the_agents_request_as_the_request_held_it() {
    let v = vectors();
    // (the vector, its forwarded props, the hash they must give: `{}`'s when there are none)
    for (name, props, props_sha256) in [
        ("v3a", Value::Null, &v["v3a"]["props_sha256"]),
        (
            "v3c",
            v["v3c"]["forwardedProps"].clone(),
            &v["v3c"]["props_sha256"],
        ),
    ] {
        let h = Harness::start_signing(&["desktop"]).await;
        // the thread as the request writes it, which is not how the id is written back
        let thread = new_thread_id().to_uppercase();
        let mut body = json!({"threadId": thread, "runId": "r", "messages": [{
            "id": "msg-test-0001", "role": "user", "content": v[name]["content"],
            "metadata": {"vymalo.promptSignature": ENVELOPE}
        }]});
        if !props.is_null() {
            body["forwardedProps"] = props;
        }
        // an alias of `plain` (ADR 0049): the person signed the segment they sent
        h.run_as_client("old-plain", ALICE, "desktop", &body)
            .await
            .all()
            .await;
        let expected = orch_core::SignedPrompt {
            envelope: ENVELOPE.to_owned().into(),
            claims: Some(claims(
                &thread,
                "old-plain",
                "msg-test-0001",
                v[name]["content_sha256"].as_str().unwrap(),
                props_sha256.as_str().unwrap(),
            )),
        };
        assert_eq!(signed_sends(&h), [Some(expected)], "{name}");
    }
}

#[tokio::test]
async fn the_content_is_hashed_as_it_arrived_with_the_members_the_typed_view_drops() {
    /// `[{"type":"text","text":"echo hi","annotations":{"seen":1}}]`, RFC 8785 and SHA-256, computed
    /// outside this code.
    const AS_SENT: &str = "00yCoXv62iPoblQpTXh2MbCH3E8X6r7aYevkkXKEq-k";
    /// The same without `annotations`: what the typed view would give.
    const AS_PARSED: &str = "3k8Bgd297ueNhZ7sPIO8HZ4WYIfPoEcir8LK0JV_pqA";
    let h = Harness::start_signing(&["desktop"]).await;
    let thread = new_thread_id();
    let body = json!({"threadId": thread, "runId": "r", "messages": [{
        "id": "m", "role": "user",
        "content": [{"type": "text", "text": "echo hi", "annotations": {"seen": 1}}],
        "metadata": {"vymalo.promptSignature": ENVELOPE}
    }]});
    h.run_as_client("plain", ALICE, "desktop", &body)
        .await
        .all()
        .await;
    let sends = signed_sends(&h);
    let hash = &sends[0]
        .as_ref()
        .unwrap()
        .claims
        .as_ref()
        .unwrap()
        .content_sha256;
    assert_eq!(hash, AS_SENT);
    assert_ne!(hash, AS_PARSED);
}

#[tokio::test]
async fn a_resumed_answers_claims_name_the_interrupt_and_hash_its_whole_payload() {
    // The shared vectors have no resume. The hash is of `{"draft":{"n":2},"text":"main"}`, RFC
    // 8785 and SHA-256, computed outside this code.
    const PAYLOAD: &str = "gmzhEDTM0PmEU8jQD0P0WoPmXe6vXaj3XpNjLsMCqLE";
    let h = Harness::start_signing(&["desktop"]).await;
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
                    "payload": {"text": "main", "draft": {"n": 2}},
                    "metadata": {"vymalo.promptSignature": ENVELOPE},
                }]),
            ),
        )
        .await;
    assert_eq!(answer.status().as_u16(), 200);
    Stream::new(answer)
        .until(|f| matches!(f.kind(), "RUN_FINISHED" | "RUN_ERROR"))
        .await;
    let v = vectors();
    let expected = claims(
        &thread,
        "plain",
        "int-3",
        PAYLOAD,
        v["v3a"]["props_sha256"].as_str().unwrap(),
    );
    let last = signed_sends(&h).pop().unwrap().unwrap();
    assert_eq!(last.claims, Some(expected));
}
