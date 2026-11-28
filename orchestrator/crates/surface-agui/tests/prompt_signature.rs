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
