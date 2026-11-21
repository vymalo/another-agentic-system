//! Trusted components (`trusted-components/v1`, ADR 0083), the pure half: the request an agent
//! makes is logged as one event with a closed component and bounded hints, it moves nothing, and a
//! request out of bounds never reaches the log.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;

fn user() -> UserId {
    UserId::new("me@example.com")
}

fn started() -> Snapshot {
    let (snapshot, _) = start_thread(
        GatePolicy::default(),
        &Input::UserMessage {
            user: user(),
            text: "add my search server".into(),
            message_id: Some("m-1".into()),
            run_id: None,
            origin: Origin::Agui,
            catalog: None,
            mentions: Vec::new(),
            tier: None,
            tier_cap: None,
        },
    )
    .unwrap();
    snapshot
}

fn request(component: TrustedComponent, hints: TrustedHints) -> Input {
    Input::Agent {
        agent: AgentId::new("coder"),
        revision: Some("rev-1".into()),
        update: AgentUpdate::TrustedRequest(TrustedRequestData { component, hints }),
    }
}

fn good() -> TrustedHints {
    TrustedHints {
        name: Some("My search".into()),
        scopes: vec!["search.read".into()],
        expires_in_days: Some(30),
    }
}

fn trusted(cmds: &[Command]) -> Vec<&TrustedRequestData> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(EventDraft {
                body: EventBody::TrustedRequest(data),
                ..
            }) => Some(data),
            _ => None,
        })
        .collect()
}

#[test]
fn a_request_is_one_event_with_the_component_and_the_hints() {
    let snapshot = started();
    let (next, cmds) =
        transition(&snapshot, &request(TrustedComponent::McpCredential, good())).unwrap();
    assert_eq!(next.state, snapshot.state, "a request moves nothing");
    let logged = trusted(&cmds);
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].component, TrustedComponent::McpCredential);
    assert_eq!(logged[0].hints, good());
    assert!(
        cmds.iter()
            .all(|c| !matches!(c, Command::Delegate { .. } | Command::RequestCancel { .. })),
        "no delegation and no cancel"
    );
}

#[test]
fn the_event_is_kind_trusted_request_and_reads_back() {
    let snapshot = started();
    let (_, cmds) =
        transition(&snapshot, &request(TrustedComponent::McpCredential, good())).unwrap();
    let draft = cmds
        .iter()
        .find_map(|c| match c {
            Command::Append(draft) => Some(draft),
            _ => None,
        })
        .unwrap();
    assert_eq!(draft.body.kind(), EventKind::TrustedRequest);
    assert_eq!(draft.body.kind().as_str(), "trusted_request");
    let data = draft.body.data_value();
    assert_eq!(
        data,
        serde_json::json!({
            "component": "McpCredential",
            "hints": {"name": "My search", "scopes": ["search.read"], "expiresInDays": 30}
        })
    );
    let back = EventBody::from_parts(EventKind::TrustedRequest, data).unwrap();
    assert_eq!(back, draft.body);
}

#[test]
fn an_empty_hints_object_is_written_as_one() {
    let snapshot = started();
    let (_, cmds) = transition(
        &snapshot,
        &request(TrustedComponent::McpCredential, TrustedHints::default()),
    )
    .unwrap();
    assert_eq!(trusted(&cmds)[0].hints, TrustedHints::default());
    let draft = cmds
        .iter()
        .find_map(|c| match c {
            Command::Append(draft) => Some(draft),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        draft.body.data_value(),
        serde_json::json!({"component": "McpCredential", "hints": {}})
    );
}

#[test]
fn a_request_out_of_bounds_never_reaches_the_log() {
    let snapshot = started();
    for hints in [
        TrustedHints {
            name: Some(String::new()),
            ..TrustedHints::default()
        },
        TrustedHints {
            name: Some("two\nlines".into()),
            ..TrustedHints::default()
        },
        TrustedHints {
            scopes: vec!["has space".into()],
            ..TrustedHints::default()
        },
        TrustedHints {
            expires_in_days: Some(0),
            ..TrustedHints::default()
        },
        TrustedHints {
            expires_in_days: Some(MAX_TRUSTED_EXPIRES_IN_DAYS + 1),
            ..TrustedHints::default()
        },
    ] {
        let (next, cmds) =
            transition(&snapshot, &request(TrustedComponent::McpCredential, hints)).unwrap();
        assert_eq!(next.state, snapshot.state);
        assert!(trusted(&cmds).is_empty(), "{cmds:?}");
    }
}

#[test]
fn a_request_on_a_finished_thread_is_refused() {
    for state in [
        ThreadState::Done,
        ThreadState::Failed,
        ThreadState::Cancelled,
    ] {
        let snapshot = Snapshot::new(state);
        let refused =
            transition(&snapshot, &request(TrustedComponent::McpCredential, good())).unwrap_err();
        assert!(matches!(
            refused,
            TransitionError::InvalidInState {
                input: "agent update",
                ..
            }
        ));
    }
}
