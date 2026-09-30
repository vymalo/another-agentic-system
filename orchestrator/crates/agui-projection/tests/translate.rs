//! The inbound table of `docs/api/agui.md`: `RunAgentInput` → core inputs.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};

use orch_agui_projection::{
    Audience, InputError, KnownThread, Projector, ThreadView, Translation, Warning,
    held_message_ids, release_selector, thread_id_of, translate, translate_with_warnings,
};
use orch_agui_proto::RunAgentInput;
use orch_core::{
    AgentId, Input, MAX_ACTION_CONTEXT_BYTES, MAX_ID_BYTES, ThreadState, UiActionData, UiVersion,
    UserId,
};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::log::{arb_actions, build, meta};

const THREAD: &str = "00000000-0000-7000-8000-000000000001";

fn alice() -> UserId {
    UserId::new("alice@example.com")
}

/// A request as a consumer would send it: `body` is merged over a minimal valid input.
fn request(body: Value) -> RunAgentInput {
    let mut base = json!({"threadId": THREAD, "runId": "run-x", "messages": []});
    for (k, v) in body.as_object().unwrap() {
        base[k] = v.clone();
    }
    RunAgentInput::from_value(base).unwrap().input
}

fn user_msg(id: &str, text: &str) -> Value {
    json!({"id": id, "role": "user", "content": text})
}

fn assistant_msg(id: &str, text: &str) -> Value {
    json!({"id": id, "role": "assistant", "content": text})
}

fn known(state: ThreadState) -> KnownThread {
    KnownThread {
        user: alice(),
        agent: AgentId::new("plain"),
        state,
        run_open: matches!(state, ThreadState::Queued | ThreadState::Working),
        open_interrupts: Vec::new(),
        message_ids: BTreeSet::new(),
        run_ids: BTreeSet::new(),
        surfaces: BTreeMap::new(),
    }
}

fn blocked() -> KnownThread {
    KnownThread {
        open_interrupts: vec!["int-3".to_owned()],
        message_ids: ["m-1".to_owned(), "evt-2".to_owned(), "evt-3".to_owned()].into(),
        run_ids: ["r-1".to_owned()].into(),
        ..known(ThreadState::Blocked)
    }
}

fn idle_known() -> KnownThread {
    // Nothing running, not blocked: a thread whose run closed in error and stays open.
    KnownThread {
        state: ThreadState::Blocked,
        run_open: false,
        open_interrupts: Vec::new(),
        ..blocked()
    }
}

fn um(text: &str, message_id: Option<&str>, run_id: &str) -> Input {
    Input::UserMessage {
        user: alice(),
        text: text.to_owned(),
        message_id: message_id.map(str::to_owned),
        run_id: Some(run_id.to_owned()),
        origin: orch_core::Origin::Agui,
    }
}

fn ok(input: &RunAgentInput, view: KnownThread) -> Vec<Input> {
    translate(input, &ThreadView::Known(view)).unwrap()
}

fn err(input: &RunAgentInput, view: KnownThread) -> InputError {
    translate(input, &ThreadView::Known(view)).unwrap_err()
}

// ---- a new thread ------------------------------------------------------------------------

#[test]
fn an_unknown_thread_with_one_user_message_is_created_by_it() {
    let input = request(json!({"messages": [user_msg("client-1", "hello")]}));
    let got = translate(&input, &ThreadView::new_thread(alice())).unwrap();
    assert_eq!(got, [um("hello", Some("client-1"), "run-x")]);
}

#[test]
fn a_new_thread_needs_exactly_one_user_message() {
    let view = ThreadView::new_thread(alice());
    let none = request(json!({}));
    assert!(matches!(
        translate(&none, &view).unwrap_err(),
        InputError::NothingToRun { .. }
    ));
    let two = request(json!({"messages": [user_msg("a", "1"), user_msg("b", "2")]}));
    assert_eq!(
        translate(&two, &view).unwrap_err(),
        InputError::TooManyNewMessages { count: 2 }
    );
    // A transcript the orchestrator never saw is not something to adopt.
    let history = request(json!({"messages": [assistant_msg("x", "hi"), user_msg("a", "1")]}));
    assert!(matches!(
        translate(&history, &view).unwrap_err(),
        InputError::NewNonUserMessage {
            role: "assistant",
            ..
        }
    ));
}

#[test]
fn a_new_thread_ignores_resume_with_a_warning() {
    let input = request(json!({
        "messages": [user_msg("a", "hi")],
        "resume": [{"interruptId": "int-1", "status": "resolved", "payload": {"text": "x"}}]
    }));
    let t = translate_with_warnings(&input, &ThreadView::new_thread(alice())).unwrap();
    assert_eq!(t.inputs, [um("hi", Some("a"), "run-x")]);
    assert!(matches!(t.warnings[0], Warning::ResumeIgnored { .. }));
}

#[test]
fn a_thread_id_must_be_a_uuid() {
    let bad = request(json!({"threadId": "not-a-uuid"}));
    let e = thread_id_of(&bad).unwrap_err();
    assert!(matches!(e, InputError::ThreadIdNotUuid { .. }));
    assert_eq!(e.http_status(), 400);
    assert_eq!(
        thread_id_of(&request(json!({}))).unwrap().to_string(),
        THREAD
    );
}

#[test]
fn the_release_comes_from_forwarded_props_under_the_extension_uri() {
    let with = request(json!({"forwardedProps": {
        "https://agents.vymalo.com/a2a/extensions/release-channels/v1": {"release": "staging"}
    }}));
    assert_eq!(release_selector(&with), Some("staging"));
    assert_eq!(release_selector(&request(json!({}))), None);
    assert_eq!(
        release_selector(&request(json!({"forwardedProps": {"release": "staging"}}))),
        None,
        "only the extension's own key selects a release"
    );
}

// ---- a known thread ----------------------------------------------------------------------

#[test]
fn one_new_user_message_is_the_input() {
    let input = request(json!({"messages": [user_msg("client-2", "and now?")]}));
    assert_eq!(
        ok(&input, idle_known()),
        [um("and now?", Some("client-2"), "run-x")]
    );
}

#[test]
fn several_new_messages_or_a_new_non_user_message_are_refused() {
    let two = request(json!({"messages": [user_msg("a", "1"), user_msg("b", "2")]}));
    assert_eq!(
        err(&two, idle_known()),
        InputError::TooManyNewMessages { count: 2 }
    );
    let assistant = request(json!({"messages": [assistant_msg("new", "I said")]}));
    let e = err(&assistant, idle_known());
    assert!(matches!(
        e,
        InputError::NewNonUserMessage {
            role: "assistant",
            ..
        }
    ));
    assert_eq!(e.http_status(), 422);
    let system =
        request(json!({"messages": [{"id": "s", "role": "system", "content": "be nice"}]}));
    assert!(matches!(
        err(&system, idle_known()),
        InputError::NewNonUserMessage { role: "system", .. }
    ));
}

#[test]
fn messages_the_thread_already_holds_are_ignored_whatever_their_role() {
    // A client that re-sends the whole transcript, with one new message at the end.
    let input = request(json!({"messages": [
        user_msg("m-1", "first"),
        assistant_msg("evt-2", "status line"),
        assistant_msg("evt-3", "another"),
        user_msg("client-9", "the new one"),
    ]}));
    assert_eq!(
        ok(&input, idle_known()),
        [um("the new one", Some("client-9"), "run-x")]
    );
}

#[test]
fn only_the_text_parts_of_a_message_are_used_and_the_rest_is_reported() {
    let input = request(json!({"messages": [{
        "id": "a", "role": "user", "content": [
            {"type": "text", "text": "look at this"},
            {"type": "image", "source": {"type": "url", "value": "https://example.com/a.png"}},
            {"type": "text", "text": "please"}
        ]
    }]}));
    let t = translate_with_warnings(&input, &ThreadView::Known(idle_known())).unwrap();
    assert_eq!(t.inputs, [um("look at this\nplease", Some("a"), "run-x")]);
    assert_eq!(
        t.warnings,
        [Warning::NonTextPartsSkipped {
            id: "a".into(),
            count: 1
        }]
    );
    let only_image = request(json!({"messages": [{
        "id": "b", "role": "user", "content": [
            {"type": "image", "source": {"type": "url", "value": "https://example.com/a.png"}}
        ]
    }]}));
    assert!(matches!(
        err(&only_image, idle_known()),
        InputError::EmptyMessage { .. }
    ));
}

#[test]
fn a_blank_message_is_refused() {
    let input = request(json!({"messages": [user_msg("a", "   ")]}));
    assert!(matches!(
        err(&input, idle_known()),
        InputError::EmptyMessage { .. }
    ));
}

// ---- resume ------------------------------------------------------------------------------

#[test]
fn a_resolved_resume_answers_the_interrupt_with_its_text() {
    let input = request(json!({
        "runId": "run-2",
        "resume": [{"interruptId": "int-3", "status": "resolved", "payload": {"text": "main"}}]
    }));
    let got = ok(&input, blocked());
    assert_eq!(got, [um("main", None, "run-2")]);
}

#[test]
fn a_resume_payload_may_be_a_bare_string() {
    let input = request(json!({
        "resume": [{"interruptId": "int-3", "status": "resolved", "payload": "main"}]
    }));
    assert_eq!(ok(&input, blocked()), [um("main", None, "run-x")]);
}

#[test]
fn a_resolved_resume_without_a_usable_answer_is_refused() {
    for payload in [
        json!(null),
        json!({"choice": 1}),
        json!({"text": ""}),
        json!(42),
    ] {
        let mut entry = json!({"interruptId": "int-3", "status": "resolved"});
        if !payload.is_null() {
            entry["payload"] = payload.clone();
        }
        let input = request(json!({"resume": [entry]}));
        assert!(
            matches!(
                err(&input, blocked()),
                InputError::InvalidResumePayload { .. }
            ),
            "{payload}"
        );
    }
}

#[test]
fn a_cancelled_resume_with_no_new_message_cancels() {
    let input = request(json!({
        "resume": [{"interruptId": "int-3", "status": "cancelled"}]
    }));
    assert_eq!(ok(&input, blocked()), [Input::Cancel { user: alice() }]);
}

#[test]
fn a_cancelled_resume_with_a_new_message_steers_away_with_it() {
    let input = request(json!({
        "messages": [user_msg("client-3", "do this instead")],
        "resume": [{"interruptId": "int-3", "status": "cancelled"}]
    }));
    assert_eq!(
        ok(&input, blocked()),
        [um("do this instead", Some("client-3"), "run-x")]
    );
}

#[test]
fn a_new_message_on_a_blocked_thread_without_resume_is_the_answer() {
    let input = request(json!({"messages": [user_msg("client-4", "main")]}));
    assert_eq!(
        ok(&input, blocked()),
        [um("main", Some("client-4"), "run-x")]
    );
}

#[test]
fn an_answer_by_resume_and_a_new_message_together_is_ambiguous() {
    let input = request(json!({
        "messages": [user_msg("client-5", "actually dev")],
        "resume": [{"interruptId": "int-3", "status": "resolved", "payload": {"text": "main"}}]
    }));
    let e = err(&input, blocked());
    assert_eq!(e, InputError::AmbiguousAnswer);
    assert_eq!(e.http_status(), 422);
}

#[test]
fn a_resume_for_an_interrupt_that_is_not_open_is_ignored_with_a_warning() {
    let input = request(json!({
        "messages": [user_msg("client-6", "hello")],
        "resume": [{"interruptId": "int-99", "status": "resolved", "payload": {"text": "x"}}]
    }));
    let t = translate_with_warnings(&input, &ThreadView::Known(blocked())).unwrap();
    assert_eq!(t.inputs, [um("hello", Some("client-6"), "run-x")]);
    assert_eq!(
        t.warnings,
        [Warning::ResumeIgnored {
            interrupt_id: "int-99".into(),
            why: "no such open interrupt"
        }]
    );
    // And on a thread that is not blocked at all.
    let running = request(json!({
        "runId": "r-1",
        "resume": [{"interruptId": "int-3", "status": "resolved", "payload": {"text": "x"}}]
    }));
    let idle = KnownThread {
        run_open: true,
        state: ThreadState::Working,
        ..blocked()
    };
    let t = translate_with_warnings(&running, &ThreadView::Known(idle)).unwrap();
    assert!(t.inputs.is_empty(), "attach, the entry was ignored");
    assert_eq!(
        t.warnings,
        [Warning::ResumeIgnored {
            interrupt_id: "int-3".into(),
            why: "the thread is not blocked"
        }]
    );
}

// ---- attach, and the state of the thread -------------------------------------------------

#[test]
fn nothing_new_and_a_run_that_exists_attaches() {
    let input = request(json!({"runId": "r-1", "messages": [user_msg("m-1", "first")]}));
    let t = translate_with_warnings(&input, &ThreadView::Known(blocked())).unwrap();
    assert_eq!(
        t,
        Translation {
            inputs: vec![],
            warnings: vec![]
        }
    );
}

#[test]
fn a_retried_post_attaches_even_while_its_run_is_open_or_after_it_finished() {
    for state in [
        ThreadState::Working,
        ThreadState::Done,
        ThreadState::Cancelled,
    ] {
        let view = KnownThread {
            run_open: matches!(state, ThreadState::Working),
            state,
            ..blocked()
        };
        let retry = request(json!({"runId": "r-1", "messages": [user_msg("m-1", "first")]}));
        assert!(ok(&retry, view).is_empty(), "{state:?}");
    }
}

#[test]
fn nothing_new_and_a_run_that_does_not_exist_is_refused() {
    let input = request(json!({"runId": "never-started"}));
    let e = err(&input, blocked());
    assert!(matches!(e, InputError::NothingToRun { .. }));
    assert_eq!(e.http_status(), 422);
}

#[test]
fn a_run_already_open_refuses_new_input() {
    let view = KnownThread {
        run_open: true,
        state: ThreadState::Working,
        ..blocked()
    };
    let input = request(json!({"messages": [user_msg("client-7", "hurry")]}));
    let e = err(&input, view);
    assert_eq!(e, InputError::RunInProgress);
    assert_eq!(e.http_status(), 409);
}

#[test]
fn a_run_id_is_never_reused_for_new_input() {
    // `r-1` is a run of this thread: a new message (or an answer) cannot start another run
    // under it. Sending nothing new under it is the attach, which is fine.
    let message = request(json!({"runId": "r-1", "messages": [user_msg("client-9", "again")]}));
    let e = err(&message, blocked());
    assert_eq!(
        e,
        InputError::RunIdReused {
            run_id: "r-1".into()
        }
    );
    assert_eq!(e.http_status(), 422);
    let answer = request(json!({
        "runId": "r-1",
        "resume": [{"interruptId": "int-3", "status": "resolved", "payload": {"text": "main"}}]
    }));
    assert!(matches!(
        err(&answer, blocked()),
        InputError::RunIdReused { .. }
    ));
    let attach = request(json!({"runId": "r-1"}));
    assert!(ok(&attach, blocked()).is_empty());
}

#[test]
fn a_finished_thread_refuses_new_input() {
    for state in [
        ThreadState::Done,
        ThreadState::Failed,
        ThreadState::Cancelled,
    ] {
        let view = KnownThread {
            state,
            run_open: false,
            open_interrupts: vec![],
            ..blocked()
        };
        let input = request(json!({"messages": [user_msg("client-8", "again")]}));
        let e = err(&input, view);
        assert_eq!(e, InputError::ThreadFinished { state });
        assert_eq!(e.http_status(), 409);
    }
}

#[test]
fn the_url_must_name_the_agent_the_thread_targets() {
    let view = ThreadView::Known(blocked());
    assert!(view.ensure_agent(&AgentId::new("plain")).is_ok());
    let e = view.ensure_agent(&AgentId::new("coder")).unwrap_err();
    assert!(matches!(e, InputError::WrongAgent { .. }));
    assert_eq!(e.http_status(), 409);
    // A thread that does not exist yet targets whatever the URL names.
    assert!(
        ThreadView::new_thread(alice())
            .ensure_agent(&AgentId::new("coder"))
            .is_ok()
    );
}

// ---- version, tools, context, unknown members --------------------------------------------

#[test]
fn another_major_version_is_refused_and_a_newer_minor_is_served_with_a_warning() {
    let view = ThreadView::Known(idle_known());
    let with = |version: &str| {
        request(json!({"protocolVersion": version, "messages": [user_msg("a", "hi")]}))
    };
    for bad in ["2.0", "0.9", "1", "one.zero", "1.x", ""] {
        let e = translate(&with(bad), &view).unwrap_err();
        assert!(matches!(e, InputError::ProtocolVersion { .. }), "{bad:?}");
        assert_eq!(e.http_status(), 400);
    }
    let t = translate_with_warnings(&with("1.0"), &view).unwrap();
    assert!(t.warnings.is_empty());
    let t = translate_with_warnings(&with("1.7"), &view).unwrap();
    assert_eq!(
        t.warnings,
        [Warning::NewerMinorVersion { got: "1.7".into() }]
    );
    assert_eq!(t.inputs.len(), 1, "served");
}

#[test]
fn tools_and_context_are_ignored_with_a_warning_and_state_silently() {
    let input = request(json!({
        "messages": [user_msg("a", "hi")],
        "tools": [{"name": "t", "description": "d"}],
        "context": [{"description": "where", "value": "here"}],
        "state": {"anything": true}
    }));
    let t = translate_with_warnings(&input, &ThreadView::Known(idle_known())).unwrap();
    assert_eq!(t.inputs.len(), 1);
    assert_eq!(
        t.warnings,
        [
            Warning::ToolsIgnored { count: 1 },
            Warning::ContextIgnored { count: 1 }
        ]
    );
}

// ---- A2UI actions (ADR 0013) ---------------------------------------------------------------

/// A blocked thread that has the surface `s1` (spoken in `v0.9.1`) and, deleted or never seen,
/// no other.
fn with_surface() -> KnownThread {
    KnownThread {
        surfaces: BTreeMap::from([("s1".to_owned(), UiVersion::V0_9_1)]),
        ..blocked()
    }
}

fn action_request(user_action: Value) -> RunAgentInput {
    request(json!({
        "runId": "run-act",
        "forwardedProps": {"a2uiAction": {"userAction": user_action}}
    }))
}

fn go() -> Value {
    json!({"name": "go", "surfaceId": "s1", "sourceComponentId": "btn",
           "context": {"choice": "a"}, "timestamp": "2026-09-29T10:00:00Z",
           "userMessage": "text an agent wrote", "type": "a2ui:action"})
}

fn expected_action() -> Input {
    let mut context = serde_json::Map::new();
    context.insert("choice".into(), json!("a"));
    Input::UiAction {
        user: alice(),
        action: UiActionData {
            surface_id: "s1".into(),
            name: "go".into(),
            source_component_id: "btn".into(),
            context,
            version: UiVersion::V0_9_1,
            run_id: Some("run-act".into()),
        },
    }
}

#[test]
fn an_action_on_a_surface_of_a_blocked_thread_is_one_ui_action() {
    let got = ok(&action_request(go()), with_surface());
    assert_eq!(
        got,
        [expected_action()],
        "timestamp, userMessage and type are dropped; the surface's version and the run id are set"
    );
}

#[test]
fn an_action_speaks_the_version_of_its_surface() {
    let view = KnownThread {
        surfaces: BTreeMap::from([("s1".to_owned(), UiVersion::V1_0)]),
        ..blocked()
    };
    let got = ok(&action_request(go()), view);
    let [Input::UiAction { action, .. }] = &got[..] else {
        panic!("{got:?}");
    };
    assert_eq!(action.version, UiVersion::V1_0);
}

#[test]
fn an_action_needs_no_context() {
    let mut a = go();
    a.as_object_mut().unwrap().remove("context");
    let got = ok(&action_request(a), with_surface());
    let [Input::UiAction { action, .. }] = &got[..] else {
        panic!("{got:?}");
    };
    assert!(action.context.is_empty());
}

#[test]
fn an_action_for_a_surface_the_thread_does_not_have_is_refused() {
    let mut a = go();
    a["surfaceId"] = json!("elsewhere");
    let e = err(&action_request(a), with_surface());
    assert_eq!(
        e,
        InputError::UnknownSurface {
            surface_id: "elsewhere".into()
        }
    );
    assert_eq!(e.http_status(), 422);
    // A thread with no surface at all, and a thread that does not exist yet.
    assert_eq!(err(&action_request(go()), blocked()).http_status(), 422);
    let new = translate(&action_request(go()), &ThreadView::new_thread(alice()));
    assert!(matches!(new, Err(InputError::UnknownSurface { .. })));
}

#[test]
fn a_surface_id_in_an_error_is_cut_short() {
    let mut a = go();
    a["surfaceId"] = json!("s".repeat(MAX_ID_BYTES));
    let InputError::UnknownSurface { surface_id } = err(&action_request(a), with_surface()) else {
        panic!("not an unknown surface");
    };
    assert_eq!(surface_id.len(), 64);
}

#[test]
fn a_malformed_action_is_refused_before_anything_is_looked_up() {
    let cases = [
        json!("go"),
        json!({}),
        json!({"surfaceId": "s1", "sourceComponentId": "btn"}),
        json!({"name": "go", "sourceComponentId": "btn"}),
        json!({"name": "go", "surfaceId": "s1"}),
        json!({"name": 3, "surfaceId": "s1", "sourceComponentId": "btn"}),
        json!({"name": "", "surfaceId": "s1", "sourceComponentId": "btn"}),
        json!({"name": "go", "surfaceId": "s1", "sourceComponentId": "btn", "context": [1]}),
        json!({"name": "go", "surfaceId": "s1", "sourceComponentId": "btn", "context": "x"}),
    ];
    for user_action in cases {
        let e = err(&action_request(user_action.clone()), with_surface());
        assert!(
            matches!(e, InputError::InvalidAction { .. }),
            "{user_action}: {e:?}"
        );
        assert_eq!(e.http_status(), 422);
    }
    // No `userAction` at all.
    let no_user_action = request(json!({"forwardedProps": {"a2uiAction": {"name": "go"}}}));
    let e = err(&no_user_action, with_surface());
    assert!(matches!(e, InputError::InvalidAction { .. }), "{e:?}");
}

#[test]
fn an_oversized_action_is_refused_with_413() {
    let mut long_name = go();
    long_name["name"] = json!("n".repeat(MAX_ID_BYTES + 1));
    let mut big_context = go();
    big_context["context"] = json!({"k": "v".repeat(MAX_ACTION_CONTEXT_BYTES)});
    for a in [long_name, big_context] {
        let e = err(&action_request(a), with_surface());
        assert!(matches!(e, InputError::ActionTooLarge { .. }), "{e:?}");
        assert_eq!(e.http_status(), 413);
    }
}

#[test]
fn an_action_beside_a_message_an_answer_or_a_cancel_is_ambiguous() {
    let with = |extra: Value| {
        request(json!({
            "runId": "run-act",
            "forwardedProps": {"a2uiAction": {"userAction": go()}},
            "messages": extra.get("messages").cloned().unwrap_or(json!([])),
            "resume": extra.get("resume").cloned().unwrap_or(json!([])),
        }))
    };
    for extra in [
        json!({"messages": [user_msg("new", "hi")]}),
        json!({"resume": [{"interruptId": "int-3", "status": "resolved", "payload": {"text": "x"}}]}),
        json!({"resume": [{"interruptId": "int-3", "status": "cancelled"}]}),
    ] {
        let e = err(&with(extra.clone()), with_surface());
        assert_eq!(e, InputError::AmbiguousAction, "{extra}");
        assert_eq!(e.http_status(), 422);
    }
}

#[test]
fn an_action_follows_the_rules_of_any_input() {
    // A run is open, or the thread is finished: 409, as for a message.
    let open = KnownThread {
        run_open: true,
        state: ThreadState::Working,
        ..with_surface()
    };
    assert_eq!(err(&action_request(go()), open), InputError::RunInProgress);
    for state in [
        ThreadState::Done,
        ThreadState::Failed,
        ThreadState::Cancelled,
    ] {
        let done = KnownThread {
            state,
            ..with_surface()
        };
        assert_eq!(
            err(&action_request(go()), done),
            InputError::ThreadFinished { state }
        );
    }
    // A run id is never reused, and an action starts a run.
    let mut reused = with_surface();
    reused.run_ids.insert("run-act".to_owned());
    assert_eq!(
        err(&action_request(go()), reused),
        InputError::RunIdReused {
            run_id: "run-act".into()
        }
    );
}

#[test]
fn unknown_members_do_not_get_in_the_way() {
    let body = json!({
        "threadId": THREAD, "runId": "run-x", "surprise": 1,
        "messages": [{"id": "a", "role": "user", "content": "hi", "extra": {"x": 1}}]
    });
    let parsed = RunAgentInput::from_value(body).unwrap();
    let mut dropped = parsed.dropped.clone();
    dropped.sort();
    assert_eq!(dropped, ["messages[0].extra", "surprise"]);
    assert_eq!(
        ok(&parsed.input, idle_known()),
        [um("hi", Some("a"), "run-x")]
    );
}

#[test]
fn the_held_message_ids_are_the_ids_of_the_requests_messages() {
    let input = request(json!({"messages": [user_msg("a", "1"), assistant_msg("b", "2")]}));
    assert_eq!(
        held_message_ids(&input),
        ["a".to_owned(), "b".to_owned()].into()
    );
}

#[test]
fn every_refusal_says_which_status_it_is() {
    let table: [(InputError, u16); 12] = [
        (InputError::ThreadIdNotUuid { got: "x".into() }, 400),
        (InputError::ProtocolVersion { got: "2.0".into() }, 400),
        (InputError::NothingToRun { run_id: "r".into() }, 422),
        (InputError::TooManyNewMessages { count: 2 }, 422),
        (
            InputError::NewNonUserMessage {
                id: "m".into(),
                role: "system",
            },
            422,
        ),
        (InputError::EmptyMessage { id: "m".into() }, 422),
        (
            InputError::InvalidResumePayload {
                interrupt_id: "i".into(),
            },
            422,
        ),
        (InputError::AmbiguousAnswer, 422),
        (InputError::RunIdReused { run_id: "r".into() }, 422),
        (InputError::RunInProgress, 409),
        (
            InputError::ThreadFinished {
                state: ThreadState::Done,
            },
            409,
        ),
        (
            InputError::WrongAgent {
                thread: "a".into(),
                requested: "b".into(),
            },
            409,
        ),
    ];
    for (e, status) in table {
        assert_eq!(e.http_status(), status, "{e}");
        assert!(!e.to_string().is_empty());
    }
}

// ---- with a real log ---------------------------------------------------------------------

#[test]
fn an_answer_is_idempotent_across_the_log_the_projection_and_the_view() {
    use orch_core::{Actor, AgentStatus, AgentStatusData, Event, EventBody, Timestamp};
    use orch_core::{ThreadStateData, UserMessageData};

    let ev = |seq: i64, actor: Actor, body: EventBody| Event {
        seq,
        thread_id: support::log::thread_id(),
        at: Timestamp::from_second(1_800_000_000 + seq).unwrap(),
        actor,
        body,
    };
    let agent = Actor::agent(&AgentId::new("plain"), None);
    let mut events = vec![
        ev(
            1,
            Actor::user(&alice()),
            EventBody::UserMessage(UserMessageData {
                text: "go".into(),
                message_id: Some("client-1".into()),
                run_id: Some("run-a".into()),
                origin: orch_core::Origin::Agui,
            }),
        ),
        ev(
            2,
            agent,
            EventBody::AgentStatus(AgentStatusData {
                status: AgentStatus::InputRequired,
                detail: Some("Which branch?".into()),
            }),
        ),
        ev(
            3,
            Actor::system(),
            EventBody::ThreadState(ThreadStateData {
                state: ThreadState::Blocked,
            }),
        ),
    ];
    let view = |events: &[Event]| {
        let mut p = Projector::new(meta());
        for e in events {
            p.apply(e, Audience::Viewer);
        }
        p.view(&alice())
    };

    // The consumer answers through the interrupt the projection announced.
    let answer = request(json!({
        "runId": "run-b",
        "messages": [user_msg("client-1", "go")],
        "resume": [{"interruptId": "int-2", "status": "resolved", "payload": {"text": "main"}}]
    }));
    let inputs = translate(&answer, &view(&events)).unwrap();
    let Input::UserMessage {
        text,
        message_id,
        run_id,
        ..
    } = &inputs[0]
    else {
        panic!("not a user message: {inputs:?}");
    };
    assert_eq!(
        (text.as_str(), message_id.as_deref(), run_id.as_deref()),
        ("main", None, Some("run-b"))
    );

    // The service records it (the core's transition puts the ids in the log) ...
    events.push(ev(
        4,
        Actor::user(&alice()),
        EventBody::UserMessage(UserMessageData {
            text: text.clone(),
            message_id: message_id.clone(),
            run_id: run_id.clone(),
            origin: orch_core::Origin::Agui,
        }),
    ));
    // ... and a retry of the same POST attaches instead of answering twice.
    let after = view(&events);
    assert!(translate(&answer, &after).unwrap().is_empty());
    // While that run is open, a *new* message is refused.
    let another = request(json!({"runId": "run-c", "messages": [user_msg("client-2", "hurry")]}));
    assert_eq!(
        translate(&another, &after).unwrap_err(),
        InputError::RunInProgress
    );
}

proptest! {
    /// Re-sending everything the log holds, with a run it has, never applies anything twice:
    /// reconciliation by id turns it into an attach.
    #[test]
    fn re_sending_the_transcript_never_duplicates_input(actions in arb_actions()) {
        let events = build(&actions);
        let mut projector = Projector::new(meta());
        for e in &events {
            projector.apply(e, Audience::Viewer);
        }
        let ThreadView::Known(view) = projector.view(&alice()) else { unreachable!() };
        let Some(run) = view.run_ids.iter().next().cloned() else { return Ok(()) };
        let messages: Vec<Value> = view
            .message_ids
            .iter()
            .map(|id| user_msg(id, "x"))
            .collect();
        let input = request(json!({"runId": run, "messages": messages}));
        let t = translate(&input, &ThreadView::Known(view)).unwrap();
        prop_assert!(t.is_empty());
    }
}
