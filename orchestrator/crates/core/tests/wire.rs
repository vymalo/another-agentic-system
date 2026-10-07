//! Wire-format tests: JSON must match the contract schemas exactly.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;

use orch_core::*;
use serde_json::json;
use uuid::Uuid;

fn tid() -> ThreadId {
    ThreadId(Uuid::parse_str("0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000").unwrap())
}

fn event(body: EventBody, actor: Actor) -> Event {
    Event {
        seq: 3,
        thread_id: tid(),
        at: "2026-09-29T10:00:00.123456Z".parse().unwrap(),
        actor,
        body,
    }
}

#[test]
fn event_json_is_exactly_the_contract_shape() {
    let e = event(
        EventBody::AgentStatus(AgentStatusData {
            status: AgentStatus::InputRequired,
            detail: None,
        }),
        Actor::agent(&AgentId::new("coder"), Some("rev-2".into())),
    );
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(
        v,
        json!({
            "seq": 3,
            "threadId": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "at": "2026-09-29T10:00:00.123456Z",
            "kind": "agent_status",
            "actor": {"type": "agent", "name": "coder", "revision": "rev-2"},
            "data": {"status": "input_required"}
        })
    );
    let back: Event = serde_json::from_value(v).unwrap();
    assert_eq!(back, e);
}

#[test]
fn every_kind_roundtrips_and_never_emits_null() {
    let bodies = vec![
        EventBody::UserMessage(UserMessageData::new("hi")),
        EventBody::AgentMessage(AgentMessageData {
            text: "t".into(),
            message_id: "m".into(),
            is_final: false,
            purpose: None,
            via: None,
        }),
        EventBody::AgentMessage(AgentMessageData {
            purpose: Some(MessagePurpose::Answer),
            via: Some(AnswerVia::TurnOutput),
            ..AgentMessageData::plain("m2", "t")
        }),
        EventBody::AgentStatus(AgentStatusData {
            status: AgentStatus::Canceled,
            detail: Some("d".into()),
        }),
        EventBody::Artifact(ArtifactData {
            name: "pr".into(),
            mime_type: None,
            uri: Some("https://x".into()),
            text: None,
            file: None,
        }),
        EventBody::ThreadState(ThreadStateData {
            state: ThreadState::Cancelled,
        }),
        EventBody::Error(ErrorData {
            message: "m".into(),
            retryable: true,
        }),
        EventBody::UiSurface(UiSurfaceData {
            operations: vec![json!({"version": "v0.9.1", "deleteSurface": {"surfaceId": "s"}})],
        }),
        EventBody::UiAction(UiActionData {
            surface_id: "s".into(),
            name: "go".into(),
            source_component_id: "b".into(),
            context: serde_json::Map::new(),
            version: UiVersion::V0_9_1,
            run_id: None,
        }),
        EventBody::JobStarted(JobStartedData { job: 2 }),
        EventBody::UiCatalog(note_catalog()),
        EventBody::AgentStep(AgentStepData {
            id: "t/acp:c2:1".into(),
            path: vec![],
            kind: StepKind::Command,
            label: "npm test".into(),
            state: StepState::Running,
            phase: StepPhase::Start,
            icon: Some("execute".into()),
            detail: Some("12 passed".into()),
            input: None,
            output: None,
            io_dropped: false,
        }),
        EventBody::ThreadTitled(ThreadTitledData {
            title: "Fix the build".into(),
            source: TitledBy::User,
        }),
        EventBody::ThreadForked(ThreadForkedData {
            from: ForkSource {
                thread_id: tid(),
                seq: 41,
            },
            kind: ForkKind::Fork,
            title: "Fix the build".into(),
            description: None,
            target: AgentTarget {
                agent_id: AgentId::new("coder"),
                release: None,
            },
        }),
        EventBody::ToolsAttached(ToolsData {
            servers: vec!["docs".into(), "websearch".into()],
        }),
        EventBody::ToolsDetached(ToolsData {
            servers: vec!["docs".into()],
        }),
    ];
    for body in bodies {
        let e = event(body, Actor::system());
        let text = serde_json::to_string(&e).unwrap();
        assert!(!text.contains("null"), "{text}");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["kind"], e.kind().as_str());
        assert_eq!(
            v["actor"],
            json!({"type": "system", "name": "orchestrator"})
        );
        let back: Event = serde_json::from_str(&text).unwrap();
        assert_eq!(back, e);
    }
}

fn note_catalog() -> UiCatalogData {
    let catalog = json!({
        "catalogId": "https://agents.vymalo.com/a2ui/catalogs/chat",
        "components": {"Note": {"type": "object"}},
    });
    UiCatalogData {
        catalog_id: "https://agents.vymalo.com/a2ui/catalogs/chat".into(),
        version: 2,
        digest: catalog_digest(&catalog).unwrap(),
        catalog,
    }
}

/// ADR 0023: the `ui_catalog` event is the person's, and its data is the object the web sends.
#[test]
fn a_ui_catalog_is_the_persons_event_and_its_data_is_what_the_web_sent() {
    let data = note_catalog();
    let e = event(
        EventBody::UiCatalog(data.clone()),
        Actor::user(&UserId::new("me@example.com")),
    );
    assert_eq!(e.kind(), EventKind::UiCatalog);
    assert_eq!(e.kind().as_str(), "ui_catalog");
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(
        v,
        json!({
            "seq": 3,
            "threadId": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "at": "2026-09-29T10:00:00.123456Z",
            "kind": "ui_catalog",
            "actor": {"type": "user", "name": "me@example.com"},
            "data": {
                "catalogId": "https://agents.vymalo.com/a2ui/catalogs/chat",
                "version": 2,
                "digest": data.digest,
                "catalog": data.catalog,
            }
        })
    );
    assert_eq!(serde_json::from_value::<Event>(v.clone()).unwrap(), e);
    // the data is exactly what the web sends, so the envelope check reads it back
    assert_eq!(UiCatalogData::from_json(&v["data"]).unwrap(), data);
    // a ui_catalog without its digest does not read
    let mut broken = v;
    broken["data"].as_object_mut().unwrap().remove("digest");
    assert!(serde_json::from_value::<Event>(broken).is_err());
}

/// ADR 0025: an `agent_step` is the agent's event, its data has the camelCase shape the contract
/// says, and the optional members are left out, never `null`.
#[test]
fn an_agent_step_is_the_agents_event_and_leaves_out_what_it_does_not_say() {
    let data = AgentStepData {
        id: "task-1/acp:c2:1".into(),
        path: vec!["task-1/tool:c2".into()],
        kind: StepKind::Command,
        label: "npm test".into(),
        state: StepState::Failed,
        phase: StepPhase::End,
        icon: None,
        detail: Some("1 failed".into()),
        input: None,
        output: None,
        io_dropped: false,
    };
    let e = event(
        EventBody::AgentStep(data.clone()),
        Actor::agent(&AgentId::new("coder"), Some("rev-2".into())),
    );
    assert_eq!(e.kind(), EventKind::AgentStep);
    assert_eq!(e.kind().as_str(), "agent_step");
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(
        v,
        json!({
            "seq": 3,
            "threadId": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "at": "2026-09-29T10:00:00.123456Z",
            "kind": "agent_step",
            "actor": {"type": "agent", "name": "coder", "revision": "rev-2"},
            "data": {
                "id": "task-1/acp:c2:1",
                "path": ["task-1/tool:c2"],
                "kind": "command",
                "label": "npm test",
                "state": "failed",
                "phase": "end",
                "detail": "1 failed"
            }
        })
    );
    assert_eq!(serde_json::from_value::<Event>(v.clone()).unwrap(), e);
    // a log written without a path (or with unknown members) still reads
    let mut loose = v;
    loose["data"].as_object_mut().unwrap().remove("path");
    loose["data"]["future"] = json!(1);
    let read: Event = serde_json::from_value(loose).unwrap();
    assert!(matches!(&read.body, EventBody::AgentStep(s) if s.path.is_empty()));
    // a state or a phase this build does not know does not read
    for (key, bad) in [("state", "paused"), ("phase", "middle"), ("kind", "robot")] {
        let mut v = serde_json::to_value(&e).unwrap();
        v["data"][key] = json!(bad);
        assert!(serde_json::from_value::<Event>(v).is_err(), "{key}");
    }
}

#[test]
fn job_started_is_the_systems_word_and_carries_the_number() {
    let e = event(
        EventBody::JobStarted(JobStartedData { job: 2 }),
        Actor::system(),
    );
    assert_eq!(e.kind(), EventKind::JobStarted);
    assert_eq!(e.kind().as_str(), "job_started");
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        json!({
            "seq": 3,
            "threadId": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "at": "2026-09-29T10:00:00.123456Z",
            "kind": "job_started",
            "actor": {"type": "system", "name": "orchestrator"},
            "data": {"job": 2}
        })
    );
    // a job_started without its number does not read
    assert!(
        serde_json::from_value::<Event>(json!({
            "seq": 3,
            "threadId": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "at": "2026-09-29T10:00:00Z",
            "kind": "job_started",
            "actor": {"type": "system", "name": "orchestrator"},
            "data": {}
        }))
        .is_err()
    );
}

#[test]
fn a_ledger_without_a_number_is_job_one_and_the_ledger_writes_it() {
    let job: Job = serde_json::from_value(json!({})).unwrap();
    assert_eq!(job.number, 1);
    let old: Job = serde_json::from_value(json!({"attempt": 2, "verification": 3})).unwrap();
    assert_eq!(old.number, 1);
    // the ledger always says which job it is (the export shows it), unlike the client's view
    assert_eq!(serde_json::to_value(Job::default()).unwrap()["number"], 1);
    let second = old.next();
    let v = serde_json::to_value(&second).unwrap();
    assert_eq!(v["number"], 2);
    assert_eq!(v["attempt"], 1);
    assert_eq!(v["verification"], 3);
    assert_eq!(serde_json::from_value::<Job>(v).unwrap(), second);
}

#[test]
fn the_view_of_a_later_job_says_which_job_it_is() {
    let mut job = Job::with_gate(GatePolicy::requiring([CheckSource::AgentChecks]));
    assert_eq!(
        serde_json::to_value(job.view().unwrap()).unwrap(),
        json!({"attempt": 1, "maxAttempts": 3, "gate": ["agent_checks"]})
    );
    job = job.next();
    assert_eq!(
        serde_json::to_value(job.view().unwrap()).unwrap(),
        json!({"number": 2, "attempt": 1, "maxAttempts": 3, "gate": ["agent_checks"]})
    );
    let read: JobView =
        serde_json::from_value(json!({"attempt": 1, "maxAttempts": 3, "gate": []})).unwrap();
    assert_eq!(read.number, 1);
}

#[test]
fn agent_message_uses_final_and_message_id() {
    let e = event(
        EventBody::AgentMessage(AgentMessageData {
            text: "t".into(),
            message_id: "m1".into(),
            is_final: true,
            purpose: None,
            via: None,
        }),
        Actor::system(),
    );
    assert_eq!(
        serde_json::to_value(&e).unwrap()["data"],
        json!({"text": "t", "messageId": "m1", "final": true})
    );
}

#[test]
fn what_the_words_are_for_is_written_only_when_it_is_known() {
    let marked = |purpose, via| {
        event(
            EventBody::AgentMessage(AgentMessageData {
                purpose,
                via,
                ..AgentMessageData::plain("m1", "t")
            }),
            Actor::system(),
        )
    };
    let data = |e: &Event| serde_json::to_value(e).unwrap()["data"].clone();
    assert_eq!(
        data(&marked(Some(MessagePurpose::Working), None)),
        json!({"text": "t", "messageId": "m1", "final": true, "purpose": "working"})
    );
    assert_eq!(
        data(&marked(
            Some(MessagePurpose::Answer),
            Some(AnswerVia::TurnOutput)
        )),
        json!({"text": "t", "messageId": "m1", "final": true, "purpose": "answer", "via": "turn_output"})
    );
    // and read back whole
    for e in [
        marked(Some(MessagePurpose::Working), None),
        marked(Some(MessagePurpose::Answer), Some(AnswerVia::TurnOutput)),
    ] {
        let back: Event = serde_json::from_value(serde_json::to_value(&e).unwrap()).unwrap();
        assert_eq!(back, e);
    }
}

#[test]
fn an_agent_message_logged_before_purpose_existed_still_reads() {
    // the shape every log held until ADR 0031
    let old = json!({
        "seq": 3,
        "threadId": "00000000-0000-7000-8000-000000000001",
        "at": "2026-10-01T00:00:00Z",
        "kind": "agent_message",
        "actor": {"type": "agent", "name": "coder"},
        "data": {"text": "t", "messageId": "m1", "final": true}
    });
    let e: Event = serde_json::from_value(old).unwrap();
    assert_eq!(
        e.body,
        EventBody::AgentMessage(AgentMessageData::plain("m1", "t"))
    );
    // a value this build does not know is refused, never guessed at (closed enums, ADR 0004)
    let unknown = json!({
        "seq": 3,
        "threadId": "00000000-0000-7000-8000-000000000001",
        "at": "2026-10-01T00:00:00Z",
        "kind": "agent_message",
        "actor": {"type": "agent", "name": "coder"},
        "data": {"text": "t", "messageId": "m1", "final": true, "purpose": "pondering"}
    });
    assert!(serde_json::from_value::<Event>(unknown).is_err());
}

#[test]
fn artifact_uses_camel_case_mime_type() {
    let e = event(
        EventBody::Artifact(ArtifactData {
            name: "patch".into(),
            mime_type: Some("text/x-diff".into()),
            uri: None,
            text: Some("diff".into()),
            file: None,
        }),
        Actor::system(),
    );
    assert_eq!(
        serde_json::to_value(&e).unwrap()["data"],
        json!({"name": "patch", "mimeType": "text/x-diff", "text": "diff"})
    );
}

#[test]
fn a_file_artifact_holds_a_reference_and_old_events_still_read() {
    let sha = "0f".repeat(32);
    let e = event(
        EventBody::Artifact(ArtifactData {
            name: "chart".into(),
            mime_type: Some("image/png".into()),
            uri: None,
            text: None,
            file: Some(FileRef {
                sha256: sha.clone(),
                size: 12,
                filename: Some("chart.png".into()),
            }),
        }),
        Actor::system(),
    );
    let wire = serde_json::to_value(&e).unwrap();
    assert_eq!(
        wire["data"],
        json!({
            "name": "chart", "mimeType": "image/png",
            "file": {"sha256": sha, "size": 12, "filename": "chart.png"}
        })
    );
    assert_eq!(serde_json::from_value::<Event>(wire).unwrap(), e);
    // an artifact logged before files existed has no `file`
    let mut old = serde_json::to_value(event(
        EventBody::Artifact(ArtifactData {
            name: "pr".into(),
            mime_type: None,
            uri: Some("https://x".into()),
            text: None,
            file: None,
        }),
        Actor::system(),
    ))
    .unwrap();
    assert!(old["data"].get("file").is_none());
    old["data"] = json!({"name": "pr", "uri": "https://x"});
    let EventBody::Artifact(read) = serde_json::from_value::<Event>(old).unwrap().body else {
        panic!("not an artifact")
    };
    assert!(read.file.is_none());
}

#[test]
fn only_the_preview_types_have_a_preview_and_a_file_has_an_href() {
    for (media_type, preview) in [
        ("image/png", Some(Preview::Image)),
        ("image/jpeg", Some(Preview::Image)),
        ("image/gif", Some(Preview::Image)),
        ("image/webp", Some(Preview::Image)),
        ("image/svg+xml", Some(Preview::Image)),
        ("text/plain", Some(Preview::Text)),
        ("application/json", Some(Preview::Text)),
        ("text/html", None),
        ("image/bmp", None),
        ("application/pdf", None),
        ("application/octet-stream", None),
        ("IMAGE/PNG", None),
        ("", None),
    ] {
        assert_eq!(Preview::of(media_type), preview, "{media_type}");
    }
    assert_eq!(Preview::Image.as_str(), "image");
    assert_eq!(Preview::Text.as_str(), "text");
    let file = FileRef {
        sha256: "ab".repeat(32),
        size: 1,
        filename: None,
    };
    assert_eq!(
        file.href(tid()),
        format!("/api/threads/{}/artifacts/{}", tid(), "ab".repeat(32))
    );
}

#[test]
fn thread_wire_has_the_owner_and_hides_the_version() {
    let t = ThreadRecord {
        id: tid(),
        owner: UserId::new("a@b.c"),
        title: "T".into(),
        description: None,
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        state: ThreadState::Working,
        job: Job::default(),
        version: 7,
        forked_from: None,
        share: None,
        pinned_at: None,
        archived_at: None,
        rail_parent: None,
        rail_rank: "i".to_owned(),
        last_seq: 2,
        created_at: "2026-09-29T10:00:00Z".parse().unwrap(),
        updated_at: "2026-09-29T10:00:01Z".parse().unwrap(),
    };
    assert_eq!(
        serde_json::to_value(&t).unwrap(),
        json!({
            "id": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "owner": "a@b.c",
            "title": "T",
            "target": {"agentId": "coder"},
            "state": "working",
            "lastSeq": 2,
            "createdAt": "2026-09-29T10:00:00Z",
            "updatedAt": "2026-09-29T10:00:01Z"
        })
    );
}

/// ADR 0029: a fork says where it came from, and the thread of a deleted parent still says how it
/// was made; a thread that was not forked says nothing.
#[test]
fn a_forked_thread_says_where_it_came_from() {
    let t = ThreadRecord {
        id: tid(),
        owner: UserId::new("a@b.c"),
        title: "T".into(),
        description: None,
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        state: ThreadState::Done,
        job: Job::default(),
        version: 1,
        forked_from: Some(ForkedFrom {
            thread_id: Some(tid()),
            seq: 41,
            kind: ForkKind::Edit,
        }),
        share: None,
        pinned_at: None,
        archived_at: None,
        rail_parent: None,
        rail_rank: "i".to_owned(),
        last_seq: 42,
        created_at: "2026-09-29T10:00:00Z".parse().unwrap(),
        updated_at: "2026-09-29T10:00:01Z".parse().unwrap(),
    };
    assert_eq!(
        serde_json::to_value(&t).unwrap()["forkedFrom"],
        json!({"threadId": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000", "seq": 41, "kind": "edit"})
    );
    let orphan = ThreadRecord {
        forked_from: Some(ForkedFrom {
            thread_id: None,
            seq: 0,
            kind: ForkKind::Fork,
        }),
        ..t
    };
    assert_eq!(
        serde_json::to_value(&orphan).unwrap()["forkedFrom"],
        json!({"seq": 0, "kind": "fork"})
    );
}

#[test]
fn a_thread_under_a_gate_carries_its_job_and_one_without_carries_none() {
    let mut job = Job::with_gate(GatePolicy::requiring([CheckSource::AgentChecks]));
    job.attempt = 2;
    job.pushed = Some(PushedRef {
        repository: "github.com/acme/demo".to_owned(),
        branch: "agent/fix".to_owned(),
        commit: "a".repeat(40),
    });
    let t = ThreadRecord {
        id: tid(),
        owner: UserId::new("a@b.c"),
        title: "T".into(),
        description: None,
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        state: ThreadState::Verifying,
        job,
        version: 7,
        forked_from: None,
        share: None,
        pinned_at: None,
        archived_at: None,
        rail_parent: None,
        rail_rank: "i".to_owned(),
        last_seq: 2,
        created_at: "2026-09-29T10:00:00Z".parse().unwrap(),
        updated_at: "2026-09-29T10:00:01Z".parse().unwrap(),
    };
    let v = serde_json::to_value(&t).unwrap();
    assert_eq!(v["state"], "verifying");
    assert_eq!(
        v["job"],
        json!({"attempt": 2, "maxAttempts": 3, "gate": ["agent_checks"], "sha": "a".repeat(40)})
    );
    // Nothing else of the ledger leaks: the task, the results and the policy stay internal.
    let ungated = ThreadRecord {
        job: Job::default(),
        ..t
    };
    assert!(serde_json::to_value(&ungated).unwrap().get("job").is_none());
}

#[test]
fn the_gate_events_have_the_wire_shape_the_contract_describes() {
    let check = event(
        EventBody::CheckResult(CheckResult {
            source: CheckSource::AgentChecks,
            name: None,
            attempt: 1,
            commit: Some("b".repeat(40)),
            status: CheckStatus::Failed,
            summary: Some("1 test failed".to_owned()),
            stale: false,
            findings: vec!["login fails".to_owned()],
        }),
        Actor::system(),
    );
    let v = serde_json::to_value(&check).unwrap();
    assert_eq!(v["kind"], "check_result");
    assert_eq!(
        v["data"],
        json!({"source": "agent_checks", "attempt": 1, "commit": "b".repeat(40),
               "status": "failed", "summary": "1 test failed", "findings": ["login fails"]})
    );
    let rework = event(
        EventBody::Rework(ReworkData {
            attempt: 2,
            max_attempts: 3,
            findings: vec![SourceFindings {
                source: CheckSource::AgentChecks,
                findings: vec!["login fails".to_owned()],
            }],
        }),
        Actor::system(),
    );
    let v = serde_json::to_value(&rework).unwrap();
    assert_eq!(v["kind"], "rework");
    assert_eq!(
        v["data"],
        json!({"attempt": 2, "maxAttempts": 3,
               "findings": [{"source": "agent_checks", "findings": ["login fails"]}]})
    );
}

#[test]
fn releases_and_target_wire() {
    let r = Releases {
        default_channel: "stable".into(),
        channels: BTreeMap::from([("stable".to_owned(), "rev-1".to_owned())]),
        revisions: None,
    };
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({"defaultChannel": "stable", "channels": {"stable": "rev-1"}})
    );
    assert!(r.accepts("stable"));
    assert!(!r.accepts("rev-1"));
    assert_eq!(r.resolve("stable").as_deref(), Some("rev-1"));
    let t: AgentTarget = serde_json::from_value(json!({"agentId": "a", "release": "x"})).unwrap();
    assert_eq!(t.release.as_deref(), Some("x"));
}

#[test]
fn agent_info_card_url_is_optional_and_absent_when_none() {
    let mut info = AgentInfo {
        id: AgentId::new("coder"),
        name: "Coder".into(),
        aliases: Vec::new(),
        description: None,
        card_url: Some("https://coder.example.com/.well-known/agent-card.json".into()),
        releases: None,
        source: AgentSource::Static,
        tags: Vec::new(),
    };
    assert_eq!(
        serde_json::to_value(&info).unwrap(),
        json!({
            "id": "coder",
            "name": "Coder",
            "cardUrl": "https://coder.example.com/.well-known/agent-card.json",
            "source": "static"
        })
    );
    info.card_url = None;
    let wire = serde_json::to_value(&info).unwrap();
    assert_eq!(
        wire,
        json!({"id": "coder", "name": "Coder", "source": "static"})
    );
    // And a client that never learned the key reads it back.
    let back: AgentInfo = serde_json::from_value(wire).unwrap();
    assert_eq!(back, info);
}

#[test]
fn agent_info_says_where_it_is_listed_from_and_carries_the_registry_tags() {
    let info = AgentInfo {
        id: AgentId::new("platform-coder"),
        name: "Coder".into(),
        aliases: Vec::new(),
        description: None,
        card_url: None,
        releases: None,
        source: AgentSource::Registry,
        tags: vec!["coding".into(), "git".into()],
    };
    let wire = serde_json::to_value(&info).unwrap();
    assert_eq!(
        wire,
        json!({"id": "platform-coder", "name": "Coder", "source": "registry", "tags": ["coding", "git"]})
    );
    let back: AgentInfo = serde_json::from_value(wire).unwrap();
    assert_eq!(back, info);
    // A document from before the field reads as a static agent with no tags.
    let old: AgentInfo = serde_json::from_value(json!({"id": "coder", "name": "Coder"})).unwrap();
    assert_eq!(old.source, AgentSource::Static);
    assert!(old.tags.is_empty());
}

#[test]
fn thread_state_spelling() {
    for (s, w) in [
        (ThreadState::Queued, "queued"),
        (ThreadState::Working, "working"),
        (ThreadState::Blocked, "blocked"),
        (ThreadState::Done, "done"),
        (ThreadState::Failed, "failed"),
        (ThreadState::Cancelled, "cancelled"),
    ] {
        assert_eq!(serde_json::to_value(s).unwrap(), json!(w));
        assert_eq!(s.as_str(), w);
    }
}

#[test]
fn user_id_is_normalised() {
    assert_eq!(UserId::new("  A@B.Com ").as_str(), "a@b.com");
}

#[test]
fn user_message_ids_are_optional_camel_case_and_absent_when_none() {
    let plain = event(
        EventBody::UserMessage(UserMessageData::new("hi")),
        Actor::system(),
    );
    assert_eq!(
        serde_json::to_value(&plain).unwrap()["data"],
        json!({"text": "hi"}),
        "no null, no empty member"
    );

    let named = event(
        EventBody::UserMessage(UserMessageData {
            text: "hi".into(),
            message_id: Some("msg-1".into()),
            run_id: Some("run-1".into()),
            origin: orch_core::Origin::Agui,
            delivery: None,
            mentions: Vec::new(),
        }),
        Actor::system(),
    );
    let v = serde_json::to_value(&named).unwrap();
    assert_eq!(
        v["data"],
        json!({"text": "hi", "messageId": "msg-1", "runId": "run-1"})
    );
    assert_eq!(serde_json::from_value::<Event>(v).unwrap(), named);

    // Only one of the two, and a log written before these fields existed, both read back.
    let only_run = json!({"text": "hi", "runId": "run-1"});
    let d: UserMessageData = serde_json::from_value(only_run.clone()).unwrap();
    assert_eq!(
        (d.message_id.as_deref(), d.run_id.as_deref()),
        (None, Some("run-1"))
    );
    assert_eq!(serde_json::to_value(&d).unwrap(), only_run);
    let old: UserMessageData = serde_json::from_value(json!({"text": "hi"})).unwrap();
    assert_eq!(old, UserMessageData::new("hi"));
}

/// ADR 0019: `origin` is `agui` (left out of the log) or `mcp`. A log written before the field
/// existed has no `origin`, and reads as the chat.
#[test]
fn a_message_from_a_tool_says_so_and_an_old_one_reads_as_the_chat() {
    let mcp = UserMessageData {
        origin: Origin::Mcp,
        ..UserMessageData::new("fix it")
    };
    let v = serde_json::to_value(&mcp).unwrap();
    assert_eq!(v, json!({"text": "fix it", "origin": "mcp"}));
    assert_eq!(serde_json::from_value::<UserMessageData>(v).unwrap(), mcp);

    let old: UserMessageData = serde_json::from_value(json!({"text": "hi"})).unwrap();
    assert_eq!(old.origin, Origin::Agui);
    let spelled: UserMessageData =
        serde_json::from_value(json!({"text": "hi", "origin": "agui"})).unwrap();
    assert_eq!(spelled, old, "an explicit agui is the same message");
    assert_eq!(
        serde_json::to_value(&old).unwrap(),
        json!({"text": "hi"}),
        "the default is not spelled"
    );
    assert!(
        serde_json::from_value::<UserMessageData>(json!({"text": "x", "origin": "chat_api"}))
            .is_err()
    );
    assert_eq!(
        (Origin::Agui.as_str(), Origin::Mcp.as_str()),
        ("agui", "mcp")
    );
}

/// ADR 0036: `delivery` is `steer` or `interrupt`, written by the core only for a message sent
/// while a job ran. A log written before the field existed reads with none, and writes none back.
#[test]
fn a_message_sent_while_a_job_runs_says_how_it_was_delivered_and_an_old_one_says_nothing() {
    for (delivery, spelled) in [
        (Delivery::Steer, "steer"),
        (Delivery::Interrupt, "interrupt"),
    ] {
        assert_eq!(delivery.as_str(), spelled);
        let message = UserMessageData {
            delivery: Some(delivery),
            ..UserMessageData::new("you were wrong since line 1")
        };
        let v = serde_json::to_value(&message).unwrap();
        assert_eq!(
            v,
            json!({"text": "you were wrong since line 1", "delivery": spelled})
        );
        assert_eq!(
            serde_json::from_value::<UserMessageData>(v).unwrap(),
            message
        );
    }
    let old: UserMessageData = serde_json::from_value(json!({"text": "hi", "runId": "r"})).unwrap();
    assert_eq!(old.delivery, None);
    assert_eq!(
        serde_json::to_value(&old).unwrap(),
        json!({"text": "hi", "runId": "r"}),
        "an absent delivery is not spelled, not even as null"
    );
    // closed: a word this build does not know is refused, not read as something else
    assert!(
        serde_json::from_value::<UserMessageData>(json!({"text": "x", "delivery": "queue"}))
            .is_err()
    );
}

/// ADR 0036: `afterStop` is the job ledger's, absent unless a stop is on its way, and a ledger
/// stored before the field existed reads as one that is not stopping.
#[test]
fn a_job_ledger_without_after_stop_is_not_stopping_and_does_not_write_one() {
    let old: Job = serde_json::from_value(json!({"number": 2, "attempt": 1})).unwrap();
    assert_eq!(old.after_stop, None);
    assert_eq!(old.number, 2);
    assert!(
        serde_json::to_value(&old)
            .unwrap()
            .get("afterStop")
            .is_none()
    );

    let stopping = Job {
        after_stop: Some("do X instead".into()),
        ..Job::default()
    };
    let v = serde_json::to_value(&stopping).unwrap();
    assert_eq!(v["afterStop"], "do X instead");
    assert_eq!(serde_json::from_value::<Job>(v).unwrap(), stopping);
    // the next job is what the text was held for
    assert_eq!(stopping.next().after_stop, None);
}

/// ADR 0026: `mentions` is the references as the person sent them, camelCase, absent when there
/// are none. A log written before the field existed reads as a message that mentions nobody and
/// writes the same bytes back; a job ledger stored before `mentioned` existed has none either.
#[test]
fn a_message_that_mentions_agents_stores_them_as_sent_and_an_old_one_mentions_nobody() {
    let message = UserMessageData {
        mentions: vec![
            Mention {
                agent_id: AgentId::new("mock-researcher"),
                label: "@researcher".into(),
                start: 3,
                end: 14,
                card_url: Some("http://mock-researcher:8080/.well-known/agent-card.json".into()),
            },
            Mention {
                agent_id: AgentId::new("mock-coder"),
                label: "@coder".into(),
                start: 20,
                end: 26,
                card_url: None,
            },
        ],
        ..UserMessageData::new("so @researcher check it, then @coder plot it")
    };
    let v = serde_json::to_value(&message).unwrap();
    assert_eq!(
        v,
        json!({
            "text": "so @researcher check it, then @coder plot it",
            "mentions": [
                {"agentId": "mock-researcher", "label": "@researcher", "start": 3, "end": 14,
                 "cardUrl": "http://mock-researcher:8080/.well-known/agent-card.json"},
                {"agentId": "mock-coder", "label": "@coder", "start": 20, "end": 26}
            ]
        })
    );
    assert_eq!(
        serde_json::from_value::<UserMessageData>(v).unwrap(),
        message
    );

    // the whole event, as the log stores it
    let logged = event(
        EventBody::UserMessage(message.clone()),
        Actor::user(&UserId::new("a@b.c")),
    );
    let back: Event = serde_json::from_value(serde_json::to_value(&logged).unwrap()).unwrap();
    assert_eq!(back, logged);

    // a log written before the field: no member, none read, none written
    let old: UserMessageData =
        serde_json::from_value(json!({"text": "hi", "messageId": "m", "delivery": "steer"}))
            .unwrap();
    assert!(old.mentions.is_empty());
    assert_eq!(
        serde_json::to_value(&old).unwrap(),
        json!({"text": "hi", "messageId": "m", "delivery": "steer"}),
        "an empty list is not spelled, not even as []"
    );
    assert!(UserMessageData::new("hi").mentions.is_empty());
    let old_event: Event = serde_json::from_value(json!({
        "threadId": "00000000-0000-0000-0000-000000000001", "seq": 1,
        "at": "2026-01-01T00:00:00Z", "actor": {"type": "user", "name": "a@b.c"},
        "kind": "user_message", "data": {"text": "hi"}
    }))
    .unwrap();
    let EventBody::UserMessage(old_message) = old_event.body else {
        panic!("a user message");
    };
    assert!(old_message.mentions.is_empty());

    // a reference is a closed shape: a member this build does not know is not dropped silently
    // by the stored type (it is checked at the door, `orch_app::mentions`), but a reference
    // without its members is refused here
    assert!(
        serde_json::from_value::<UserMessageData>(
            json!({"text": "x", "mentions": [{"agentId": "a", "label": "@a", "start": 0}]})
        )
        .is_err()
    );

    // the job ledger
    let ledger: Job = serde_json::from_value(json!({"number": 2, "attempt": 1})).unwrap();
    assert!(ledger.mentioned.is_empty() && ledger.after_stop_mentions.is_empty());
    let v = serde_json::to_value(&ledger).unwrap();
    assert!(v.get("mentioned").is_none() && v.get("afterStopMentions").is_none());
    let with = Job {
        mentioned: [AgentId::new("mock-researcher"), AgentId::new("mock-coder")].into(),
        after_stop_mentions: vec![Mention {
            agent_id: AgentId::new("mock-coder"),
            label: "@coder".into(),
            start: 0,
            end: 6,
            card_url: None,
        }],
        ..Job::default()
    };
    let v = serde_json::to_value(&with).unwrap();
    assert_eq!(
        v["mentioned"],
        json!(["mock-coder", "mock-researcher"]),
        "sorted"
    );
    assert_eq!(serde_json::from_value::<Job>(v).unwrap(), with);
    // the next job starts with the mentions of its own message only
    let next = with.next();
    assert!(next.mentioned.is_empty() && next.after_stop_mentions.is_empty());
}

#[test]
fn auth_required_is_its_own_status_spelling() {
    let e = event(
        EventBody::AgentStatus(AgentStatusData {
            status: AgentStatus::AuthRequired,
            detail: Some("github".into()),
        }),
        Actor::agent(&AgentId::new("coder"), None),
    );
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(
        v["data"],
        json!({"status": "auth_required", "detail": "github"})
    );
    assert_eq!(serde_json::from_value::<Event>(v).unwrap(), e);
}

#[test]
fn ui_events_use_the_contract_spelling() {
    let surface = event(
        EventBody::UiSurface(UiSurfaceData {
            operations: vec![json!({"version": "v0.9.1", "createSurface": {"surfaceId": "s1"}})],
        }),
        Actor::agent(&AgentId::new("coder"), None),
    );
    let v = serde_json::to_value(&surface).unwrap();
    assert_eq!(v["kind"], "ui_surface");
    assert_eq!(
        v["data"],
        json!({"operations": [{"version": "v0.9.1", "createSurface": {"surfaceId": "s1"}}]})
    );
    assert_eq!(serde_json::from_value::<Event>(v).unwrap(), surface);

    let mut context = serde_json::Map::new();
    context.insert("email".into(), json!("a@b.c"));
    let action = event(
        EventBody::UiAction(UiActionData {
            surface_id: "s1".into(),
            name: "submit".into(),
            source_component_id: "btn".into(),
            context,
            version: UiVersion::V0_9_1,
            run_id: Some("run-2".into()),
        }),
        Actor::user(&UserId::new("a@b.c")),
    );
    let v = serde_json::to_value(&action).unwrap();
    assert_eq!(v["kind"], "ui_action");
    assert_eq!(
        v["data"],
        json!({"surfaceId": "s1", "name": "submit", "sourceComponentId": "btn",
               "context": {"email": "a@b.c"}, "version": "v0.9.1", "runId": "run-2"})
    );
    assert_eq!(serde_json::from_value::<Event>(v).unwrap(), action);
}

#[test]
fn an_event_of_an_unknown_kind_or_a_malformed_ui_body_does_not_read() {
    let base = |kind: &str, data: serde_json::Value| {
        json!({"seq": 1, "threadId": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
               "at": "2026-09-29T10:00:00Z", "kind": kind,
               "actor": {"type": "system", "name": "orchestrator"}, "data": data})
    };
    assert!(serde_json::from_value::<Event>(base("ui_other", json!({}))).is_err());
    assert!(serde_json::from_value::<Event>(base("ui_surface", json!({"operations": 1}))).is_err());
    assert!(serde_json::from_value::<Event>(base("ui_action", json!({"name": "x"}))).is_err());
}

/// A rename is the person's event: its data is the new title and who wrote it, and a source this
/// build does not know does not read (the first message's words are a thread's start, never an
/// event).
#[test]
fn a_thread_titled_is_the_persons_event_with_the_title_and_its_writer() {
    let e = event(
        EventBody::ThreadTitled(ThreadTitledData {
            title: "Fix the build".into(),
            source: TitledBy::User,
        }),
        Actor::user(&UserId::new("me@example.com")),
    );
    assert_eq!(e.kind(), EventKind::ThreadTitled);
    assert_eq!(e.kind().as_str(), "thread_titled");
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(
        v,
        json!({
            "seq": 3,
            "threadId": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "at": "2026-09-29T10:00:00.123456Z",
            "kind": "thread_titled",
            "actor": {"type": "user", "name": "me@example.com"},
            "data": {"title": "Fix the build", "source": "user"}
        })
    );
    assert_eq!(serde_json::from_value::<Event>(v.clone()).unwrap(), e);
    for bad in ["first_message", "robot"] {
        let mut v = v.clone();
        v["data"]["source"] = json!(bad);
        assert!(serde_json::from_value::<Event>(v).is_err(), "{bad}");
    }
}

/// ADR 0029: the `thread_forked` event is the person's; it names the thread and the last event
/// copied, how the fork was made, the title it keeps and the agent it talks to. A fork onto the
/// parent's agent and a release names both, and nothing else is spelled that is not set.
#[test]
fn a_thread_forked_is_the_persons_event_with_where_it_came_from() {
    let e = event(
        EventBody::ThreadForked(ThreadForkedData {
            from: ForkSource {
                thread_id: tid(),
                seq: 41,
            },
            kind: ForkKind::Edit,
            title: "Fix the redirect loop".into(),
            description: Some("Fixing a redirect loop on the login page.".into()),
            target: AgentTarget {
                agent_id: AgentId::new("coder"),
                release: Some("stable".into()),
            },
        }),
        Actor::user(&UserId::new("me@example.com")),
    );
    assert_eq!(e.kind(), EventKind::ThreadForked);
    assert_eq!(e.kind().as_str(), "thread_forked");
    let v = serde_json::to_value(&e).unwrap();
    assert_eq!(
        v,
        json!({
            "seq": 3,
            "threadId": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000",
            "at": "2026-09-29T10:00:00.123456Z",
            "kind": "thread_forked",
            "actor": {"type": "user", "name": "me@example.com"},
            "data": {
                "from": {"threadId": "0199aaaa-bbbb-7ccc-8ddd-eeeeffff0000", "seq": 41},
                "kind": "edit",
                "title": "Fix the redirect loop",
                "description": "Fixing a redirect loop on the login page.",
                "target": {"agentId": "coder", "release": "stable"}
            }
        })
    );
    assert_eq!(serde_json::from_value::<Event>(v.clone()).unwrap(), e);
    for (member, bad) in [("kind", json!("branch")), ("from", json!({"seq": 1}))] {
        let mut v = v.clone();
        v["data"][member] = bad;
        assert!(serde_json::from_value::<Event>(v).is_err(), "{member}");
    }
}

/// ADR 0035: a description is the model's or the person's event; its data is the description and who
/// wrote it, an empty one is the person clearing it, and a source this build does not know does not
/// read.
#[test]
fn a_thread_described_is_an_event_with_the_description_and_its_writer() {
    for (description, source, actor) in [
        (
            "Moving the build to Rust; the tests pass.",
            "model",
            Actor::system(),
        ),
        ("", "user", Actor::user(&UserId::new("me@example.com"))),
    ] {
        let by = if source == "model" {
            DescribedBy::Model
        } else {
            DescribedBy::User
        };
        let e = event(
            EventBody::ThreadDescribed(ThreadDescribedData {
                description: description.into(),
                source: by,
            }),
            actor.clone(),
        );
        assert_eq!(e.kind(), EventKind::ThreadDescribed);
        assert_eq!(e.kind().as_str(), "thread_described");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], "thread_described");
        assert_eq!(
            v["data"],
            json!({"description": description, "source": source})
        );
        assert_eq!(serde_json::from_value::<Event>(v.clone()).unwrap(), e);
        let mut bad = v.clone();
        bad["data"]["source"] = json!("robot");
        assert!(serde_json::from_value::<Event>(bad).is_err());
    }
}

/// The thread resource says its description when it has one, and says nothing when it has none.
#[test]
fn a_thread_says_its_description_only_when_it_has_one() {
    let mut t = ThreadRecord {
        id: tid(),
        owner: UserId::new("a@b.c"),
        title: "T".into(),
        description: Some("A sentence.".into()),
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        state: ThreadState::Done,
        job: Job::default(),
        version: 3,
        forked_from: None,
        share: None,
        pinned_at: None,
        archived_at: None,
        rail_parent: None,
        rail_rank: "i".to_owned(),
        last_seq: 2,
        created_at: Timestamp::from_second(1_790_000_000).unwrap(),
        updated_at: Timestamp::from_second(1_790_000_001).unwrap(),
    };
    assert_eq!(
        serde_json::to_value(&t).unwrap()["description"],
        "A sentence."
    );
    t.description = None;
    assert!(
        serde_json::to_value(&t)
            .unwrap()
            .get("description")
            .is_none()
    );
}

/// The owner's organisation of their list is on the wire only when it is set (ADR 0042): a thread
/// that is neither pinned, archived nor nested says nothing of them, and the rank is never said.
#[test]
fn a_thread_says_what_the_owner_did_to_their_list_only_when_they_did_it() {
    let at = Timestamp::from_second(1_790_000_000).unwrap();
    let mut t = ThreadRecord {
        id: ThreadId(uuid::Uuid::from_u128(1)),
        owner: UserId::new("alice@example.com"),
        title: "t".into(),
        description: None,
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        state: ThreadState::Done,
        job: Job::default(),
        version: 1,
        forked_from: None,
        share: None,
        pinned_at: None,
        archived_at: None,
        rail_parent: None,
        rail_rank: "i".to_owned(),
        last_seq: 0,
        created_at: at,
        updated_at: at,
    };
    let plain = serde_json::to_value(&t).unwrap();
    for member in [
        "pinned",
        "archived",
        "nestedUnder",
        "railRank",
        "railParent",
        "rail_rank",
    ] {
        assert!(plain.get(member).is_none(), "{member} in {plain}");
    }
    t.pinned_at = Some(at);
    t.archived_at = Some(at);
    t.rail_parent = Some(ThreadId(uuid::Uuid::from_u128(7)));
    t.rail_rank = "0000zz".to_owned();
    let set = serde_json::to_value(&t).unwrap();
    assert_eq!(set["pinned"], serde_json::json!(true));
    assert_eq!(set["archived"], serde_json::json!(true));
    assert_eq!(
        set["nestedUnder"],
        serde_json::json!(t.rail_parent.unwrap().to_string())
    );
    assert!(
        !set.to_string().contains("0000zz"),
        "the rank is never said: {set}"
    );
    for member in ["pinnedAt", "archivedAt", "railParent", "railRank"] {
        assert!(set.get(member).is_none(), "{member} in {set}");
    }
}
