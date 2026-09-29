//! Property tests: any event or input the crate can express survives the wire and validates.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_agui_proto::testkit::{assert_conforms, assert_input_conforms};
use orch_agui_proto::*;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

const MAX_SAFE: i64 = 9_007_199_254_740_991;

fn id() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-zA-Z0-9_-]{1,12}",
        Just("evt-1".to_owned()),
        // Ids are opaque: anything printable, non-ASCII included.
        "\\PC{1,8}",
    ]
}

fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        "\\PC{0,24}",
        "[\n\r\t\"\\\\ a-z]{0,12}"
    ]
}

/// Any JSON value without floats (their text round trip is not the property under test) and,
/// when `allow_null` is false, without a top-level `null`.
fn json_value(allow_null: bool) -> BoxedStrategy<Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        (-MAX_SAFE..=MAX_SAFE).prop_map(Value::from),
        text().prop_map(Value::from),
    ];
    let nested = leaf.prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map("[a-z.]{1,6}", inner, 0..4)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    });
    if allow_null {
        nested.boxed()
    } else {
        nested.prop_filter("no null", |v| !v.is_null()).boxed()
    }
}

fn object() -> impl Strategy<Value = Map<String, Value>> {
    prop::collection::btree_map("[a-z.]{1,6}", json_value(true), 0..4)
        .prop_map(|m| m.into_iter().collect())
}

fn opt<T: std::fmt::Debug + 'static>(
    s: impl Strategy<Value = T> + 'static,
) -> impl Strategy<Value = Option<T>> {
    prop::option::of(s)
}

fn sub() -> impl Strategy<Value = Option<SubagentRunId>> {
    opt(id().prop_map(SubagentRunId::new))
}

fn base() -> impl Strategy<Value = BaseFields> {
    (
        opt(-MAX_SAFE..=MAX_SAFE),
        opt(json_value(false)),
        opt(object()),
    )
        .prop_map(|(timestamp, raw_event, metadata)| BaseFields {
            timestamp,
            raw_event,
            metadata,
        })
}

fn pointer() -> impl Strategy<Value = JsonPointer> {
    prop_oneof![
        Just(String::new()),
        prop::collection::vec("[a-z0-9]{0,4}|~0|~1", 1..4)
            .prop_map(|t| format!("/{}", t.join("/"))),
    ]
    .prop_map(|p| JsonPointer::new(p).unwrap())
}

fn patch() -> BoxedStrategy<JsonPatch> {
    prop::collection::vec(
        prop_oneof![
            (pointer(), json_value(true))
                .prop_map(|(path, value)| JsonPatchOperation::Add { path, value }),
            pointer().prop_map(|path| JsonPatchOperation::Remove { path }),
            (pointer(), json_value(true))
                .prop_map(|(path, value)| JsonPatchOperation::Replace { path, value }),
            (pointer(), pointer()).prop_map(|(from, path)| JsonPatchOperation::Move { from, path }),
            (pointer(), pointer()).prop_map(|(from, path)| JsonPatchOperation::Copy { from, path }),
            (pointer(), json_value(true))
                .prop_map(|(path, value)| JsonPatchOperation::Test { path, value }),
        ],
        0..4,
    )
    .boxed()
}

fn source() -> BoxedStrategy<PartSource> {
    prop_oneof![
        ("[A-Za-z0-9+/]{0,8}", "[a-z]+/[a-z]+")
            .prop_map(|(value, mime_type)| PartSource::Data { value, mime_type }),
        (text(), opt("[a-z]+/[a-z]+"))
            .prop_map(|(value, mime_type)| PartSource::Url { value, mime_type }),
        (text(), opt(text()), opt("[a-z]+/[a-z]+")).prop_map(|(value, provider, mime_type)| {
            PartSource::File {
                value,
                provider,
                mime_type,
            }
        }),
    ]
    .boxed()
}

fn part() -> BoxedStrategy<ContentPart> {
    prop_oneof![
        (opt(id()), text(), opt(json_value(false)))
            .prop_map(|(id, text, metadata)| ContentPart::Text { id, text, metadata }),
        (opt(id()), source(), opt(json_value(false))).prop_map(|(id, source, metadata)| {
            ContentPart::Image {
                id,
                source,
                metadata,
            }
        }),
        (opt(id()), source(), opt(json_value(false))).prop_map(|(id, source, metadata)| {
            ContentPart::Audio {
                id,
                source,
                metadata,
            }
        }),
        (opt(id()), source(), opt(json_value(false))).prop_map(|(id, source, metadata)| {
            ContentPart::Video {
                id,
                source,
                metadata,
            }
        }),
        (opt(id()), source(), opt(json_value(false))).prop_map(|(id, source, metadata)| {
            ContentPart::Document {
                id,
                source,
                metadata,
            }
        }),
    ]
    .boxed()
}

fn content() -> BoxedStrategy<MessageContent> {
    prop_oneof![
        text().prop_map(MessageContent::Text),
        prop::collection::vec(part(), 0..3).prop_map(MessageContent::Parts),
    ]
    .boxed()
}

fn tool_call() -> BoxedStrategy<ToolCall> {
    (id(), text(), text(), opt(text()), opt(object()))
        .prop_map(|(id, name, arguments, encrypted_value, metadata)| {
            let mut call = ToolCall::new(id, name, arguments);
            call.encrypted_value = encrypted_value;
            call.metadata = metadata;
            call
        })
        .boxed()
}

fn message() -> BoxedStrategy<Message> {
    let common = || (id(), opt(text()), opt(text()), opt(object()), sub());
    prop_oneof![
        (common(), text()).prop_map(
            |((id, name, encrypted_value, metadata, subagent_run_id), content)| {
                Message::Developer(DeveloperMessage {
                    id: id.into(),
                    content,
                    name,
                    encrypted_value,
                    metadata,
                    subagent_run_id,
                })
            }
        ),
        (common(), text()).prop_map(
            |((id, name, encrypted_value, metadata, subagent_run_id), content)| {
                Message::System(SystemMessage {
                    id: id.into(),
                    content,
                    name,
                    encrypted_value,
                    metadata,
                    subagent_run_id,
                })
            }
        ),
        (
            common(),
            opt(text()),
            opt(prop::collection::vec(tool_call(), 0..3))
        )
            .prop_map(
                |((id, name, encrypted_value, metadata, subagent_run_id), content, tool_calls)| {
                    Message::Assistant(AssistantMessage {
                        id: id.into(),
                        content,
                        tool_calls,
                        name,
                        encrypted_value,
                        metadata,
                        subagent_run_id,
                    })
                }
            ),
        (common(), content()).prop_map(
            |((id, name, encrypted_value, metadata, subagent_run_id), content)| {
                Message::User(UserMessage {
                    id: id.into(),
                    content,
                    name,
                    encrypted_value,
                    metadata,
                    subagent_run_id,
                })
            }
        ),
        (
            id(),
            content(),
            id(),
            opt(text()),
            opt(text()),
            opt(object()),
            sub()
        )
            .prop_map(
                |(id, content, tool_call_id, error, encrypted_value, metadata, subagent_run_id)| {
                    Message::Tool(ToolMessage {
                        id: id.into(),
                        content,
                        tool_call_id: tool_call_id.into(),
                        error,
                        encrypted_value,
                        metadata,
                        subagent_run_id,
                    })
                }
            ),
        (id(), text(), object(), opt(object()), sub()).prop_map(
            |(id, activity_type, content, metadata, subagent_run_id)| {
                Message::Activity(ActivityMessage {
                    id: id.into(),
                    activity_type,
                    content,
                    metadata,
                    subagent_run_id,
                })
            }
        ),
        (id(), text(), opt(text()), opt(object()), sub()).prop_map(
            |(id, content, encrypted_value, metadata, subagent_run_id)| {
                Message::Reasoning(ReasoningMessage {
                    id: id.into(),
                    content,
                    encrypted_value,
                    metadata,
                    subagent_run_id,
                })
            }
        ),
    ]
    .boxed()
}

fn tool() -> BoxedStrategy<Tool> {
    (text(), text(), opt(json_value(false)), opt(object()))
        .prop_map(|(name, description, parameters, metadata)| Tool {
            name,
            description,
            parameters,
            metadata,
        })
        .boxed()
}

fn resume_entry() -> BoxedStrategy<ResumeEntry> {
    (
        id(),
        prop_oneof![Just(ResumeStatus::Resolved), Just(ResumeStatus::Cancelled)],
        opt(json_value(false)),
        opt(object()),
    )
        .prop_map(|(interrupt_id, status, payload, metadata)| ResumeEntry {
            interrupt_id: interrupt_id.into(),
            status,
            payload,
            metadata,
        })
        .boxed()
}

fn input() -> BoxedStrategy<RunAgentInput> {
    (
        (id(), id(), opt("[0-9]\\.[0-9]"), opt(id())),
        opt(json_value(false)),
        prop::collection::vec(message(), 0..4),
        opt(prop::collection::vec(tool(), 0..3)),
        opt(prop::collection::vec(
            (text(), text()).prop_map(|(description, value)| Context { description, value }),
            0..3,
        )),
        opt(json_value(false)),
        opt(prop::collection::vec(resume_entry(), 0..3)),
    )
        .prop_map(
            |(
                (thread_id, run_id, protocol_version, parent_run_id),
                state,
                messages,
                tools,
                context,
                forwarded_props,
                resume,
            )| RunAgentInput {
                thread_id: thread_id.into(),
                run_id: run_id.into(),
                protocol_version,
                parent_run_id: parent_run_id.map(RunId::new),
                state,
                messages,
                tools,
                context,
                forwarded_props,
                resume,
            },
        )
        .boxed()
}

fn usage() -> BoxedStrategy<TokenUsage> {
    let n = || opt(0..=MAX_SAFE as u64);
    ((opt(text()), opt(text())), (n(), n(), n()), (n(), n(), n()))
        .prop_map(
            |(
                (provider, model),
                (input_tokens, output_tokens, total_tokens),
                (reasoning_tokens, cached_input_tokens, cache_write_input_tokens),
            )| TokenUsage {
                provider,
                model,
                input_tokens,
                output_tokens,
                total_tokens,
                reasoning_tokens,
                cached_input_tokens,
                cache_write_input_tokens,
            },
        )
        .boxed()
}

fn interrupt() -> BoxedStrategy<Interrupt> {
    (
        id(),
        text(),
        opt(text()),
        opt(id()),
        opt(object()),
        opt(text()),
        opt(object()),
        sub(),
    )
        .prop_map(
            |(
                id,
                reason,
                message,
                tool_call_id,
                response_schema,
                expires_at,
                metadata,
                subagent_run_id,
            )| Interrupt {
                id: id.into(),
                reason,
                message,
                tool_call_id: tool_call_id.map(ToolCallId::new),
                response_schema,
                expires_at,
                metadata,
                subagent_run_id,
            },
        )
        .boxed()
}

fn outcome() -> BoxedStrategy<RunFinishedOutcome> {
    prop_oneof![
        opt(prop::collection::vec(id().prop_map(ToolCallId::new), 0..3)).prop_map(
            |pending_tool_call_ids| RunFinishedOutcome::Success {
                pending_tool_call_ids
            }
        ),
        prop::collection::vec(interrupt(), 1..3)
            .prop_map(|interrupts| RunFinishedOutcome::Interrupt { interrupts }),
        Just(RunFinishedOutcome::Cancelled),
    ]
    .boxed()
}

fn sub_outcome() -> impl Strategy<Value = SubagentFinishedOutcome> {
    prop_oneof![
        Just(SubagentFinishedOutcome::Success),
        opt(prop::collection::vec(id().prop_map(InterruptId::new), 0..3))
            .prop_map(|interrupt_ids| SubagentFinishedOutcome::Suspended { interrupt_ids }),
    ]
}

fn role() -> impl Strategy<Value = TextMessageRole> {
    prop_oneof![
        Just(TextMessageRole::Developer),
        Just(TextMessageRole::System),
        Just(TextMessageRole::Assistant),
        Just(TextMessageRole::User)
    ]
}

/// Events of the first half of the types.
fn event_a() -> BoxedStrategy<Event> {
    let mid = || id().prop_map(MessageId::new);
    let tid = || id().prop_map(ToolCallId::new);
    prop_oneof![
        (base(), mid(), opt(role()), opt(text()), sub()).prop_map(
            |(base, message_id, role, name, subagent_run_id)| TextMessageStartEvent {
                base,
                message_id,
                role,
                name,
                subagent_run_id
            }
            .into()
        ),
        (base(), mid(), text(), sub()).prop_map(|(base, message_id, delta, subagent_run_id)| {
            TextMessageContentEvent {
                base,
                message_id,
                delta,
                subagent_run_id,
            }
            .into()
        }),
        (base(), mid(), sub()).prop_map(|(base, message_id, subagent_run_id)| {
            TextMessageEndEvent {
                base,
                message_id,
                subagent_run_id,
            }
            .into()
        }),
        (
            base(),
            opt(mid()),
            opt(role()),
            opt(text()),
            opt(text()),
            sub()
        )
            .prop_map(|(base, message_id, role, delta, name, subagent_run_id)| {
                TextMessageChunkEvent {
                    base,
                    message_id,
                    role,
                    delta,
                    name,
                    subagent_run_id,
                }
                .into()
            }),
        (base(), tid(), text(), opt(mid()), sub()).prop_map(
            |(base, tool_call_id, tool_call_name, parent_message_id, subagent_run_id)| {
                ToolCallStartEvent {
                    base,
                    tool_call_id,
                    tool_call_name,
                    parent_message_id,
                    subagent_run_id,
                }
                .into()
            }
        ),
        (base(), tid(), text(), sub()).prop_map(|(base, tool_call_id, delta, subagent_run_id)| {
            ToolCallArgsEvent {
                base,
                tool_call_id,
                delta,
                subagent_run_id,
            }
            .into()
        }),
        (base(), tid(), sub()).prop_map(|(base, tool_call_id, subagent_run_id)| ToolCallEndEvent {
            base,
            tool_call_id,
            subagent_run_id
        }
        .into()),
        (
            base(),
            opt(tid()),
            opt(text()),
            opt(mid()),
            opt(text()),
            sub()
        )
            .prop_map(
                |(
                    base,
                    tool_call_id,
                    tool_call_name,
                    parent_message_id,
                    delta,
                    subagent_run_id,
                )| ToolCallChunkEvent {
                    base,
                    tool_call_id,
                    tool_call_name,
                    parent_message_id,
                    delta,
                    subagent_run_id
                }
                .into()
            ),
        (
            base(),
            mid(),
            tid(),
            content(),
            opt(Just(ToolRole::Tool)),
            sub()
        )
            .prop_map(
                |(base, message_id, tool_call_id, content, role, subagent_run_id)| {
                    ToolCallResultEvent {
                        base,
                        message_id,
                        tool_call_id,
                        content,
                        role,
                        subagent_run_id,
                    }
                    .into()
                }
            ),
        (base(), json_value(true), sub()).prop_map(|(base, snapshot, subagent_run_id)| {
            StateSnapshotEvent {
                base,
                snapshot,
                subagent_run_id,
            }
            .into()
        }),
        (base(), patch(), sub()).prop_map(|(base, delta, subagent_run_id)| StateDeltaEvent {
            base,
            delta,
            subagent_run_id
        }
        .into()),
        (base(), prop::collection::vec(message(), 0..4))
            .prop_map(|(base, messages)| MessagesSnapshotEvent { base, messages }.into()),
        (base(), mid(), text(), object(), opt(any::<bool>()), sub()).prop_map(
            |(base, message_id, activity_type, content, replace, subagent_run_id)| {
                ActivitySnapshotEvent {
                    base,
                    message_id,
                    activity_type,
                    content,
                    replace,
                    subagent_run_id,
                }
                .into()
            }
        ),
        (base(), mid(), text(), patch(), sub()).prop_map(
            |(base, message_id, activity_type, patch, subagent_run_id)| ActivityDeltaEvent {
                base,
                message_id,
                activity_type,
                patch,
                subagent_run_id
            }
            .into()
        ),
        (base(), json_value(false), opt(text()), sub()).prop_map(
            |(base, event, source, subagent_run_id)| RawEvent {
                base,
                event,
                source,
                subagent_run_id
            }
            .into()
        ),
    ]
    .boxed()
}

/// Events of the second half of the types.
fn event_b() -> BoxedStrategy<Event> {
    let mid = || id().prop_map(MessageId::new);
    let usage_list = || opt(prop::collection::vec(usage(), 0..3));
    prop_oneof![
        (base(), text(), json_value(false), sub()).prop_map(
            |(base, name, value, subagent_run_id)| CustomEvent {
                base,
                name,
                value,
                subagent_run_id
            }
            .into()
        ),
        (
            base(),
            id(),
            id(),
            opt("[0-9]\\.[0-9]"),
            opt(id()),
            opt(input())
        )
            .prop_map(
                |(base, thread_id, run_id, protocol_version, parent_run_id, input)| {
                    RunStartedEvent {
                        base,
                        thread_id: thread_id.into(),
                        run_id: run_id.into(),
                        protocol_version,
                        parent_run_id: parent_run_id.map(RunId::new),
                        input: input.map(Box::new),
                    }
                    .into()
                }
            ),
        (
            base(),
            id(),
            id(),
            opt(json_value(false)),
            opt(outcome()),
            usage_list()
        )
            .prop_map(|(base, thread_id, run_id, result, outcome, usage)| {
                RunFinishedEvent {
                    base,
                    thread_id: thread_id.into(),
                    run_id: run_id.into(),
                    result,
                    outcome,
                    usage,
                }
                .into()
            }),
        (base(), text(), opt(text()), usage_list()).prop_map(|(base, message, code, usage)| {
            RunErrorEvent {
                base,
                message,
                code,
                usage,
            }
            .into()
        }),
        (base(), text(), sub()).prop_map(|(base, step_name, subagent_run_id)| StepStartedEvent {
            base,
            step_name,
            subagent_run_id
        }
        .into()),
        (base(), text(), sub()).prop_map(|(base, step_name, subagent_run_id)| StepFinishedEvent {
            base,
            step_name,
            subagent_run_id
        }
        .into()),
        (base(), mid(), sub()).prop_map(|(base, message_id, subagent_run_id)| {
            ReasoningStartEvent {
                base,
                message_id,
                subagent_run_id,
            }
            .into()
        }),
        (base(), mid(), sub()).prop_map(|(base, message_id, subagent_run_id)| {
            ReasoningMessageStartEvent {
                base,
                message_id,
                role: ReasoningRole::Reasoning,
                subagent_run_id,
            }
            .into()
        }),
        (base(), mid(), text(), sub()).prop_map(|(base, message_id, delta, subagent_run_id)| {
            ReasoningMessageContentEvent {
                base,
                message_id,
                delta,
                subagent_run_id,
            }
            .into()
        }),
        (base(), mid(), sub()).prop_map(|(base, message_id, subagent_run_id)| {
            ReasoningMessageEndEvent {
                base,
                message_id,
                subagent_run_id,
            }
            .into()
        }),
        (base(), opt(mid()), opt(text()), sub()).prop_map(
            |(base, message_id, delta, subagent_run_id)| ReasoningMessageChunkEvent {
                base,
                message_id,
                delta,
                subagent_run_id
            }
            .into()
        ),
        (base(), mid(), sub()).prop_map(|(base, message_id, subagent_run_id)| ReasoningEndEvent {
            base,
            message_id,
            subagent_run_id
        }
        .into()),
        (
            base(),
            prop_oneof![
                Just(ReasoningEncryptedValueSubtype::ToolCall),
                Just(ReasoningEncryptedValueSubtype::Message)
            ],
            text(),
            text(),
            sub()
        )
            .prop_map(
                |(base, subtype, entity_id, encrypted_value, subagent_run_id)| {
                    ReasoningEncryptedValueEvent {
                        base,
                        subtype,
                        entity_id,
                        encrypted_value,
                        subagent_run_id,
                    }
                    .into()
                }
            ),
        (
            base(),
            id(),
            text(),
            opt(text()),
            opt(id()),
            opt(id()),
            opt(mid())
        )
            .prop_map(
                |(
                    base,
                    subagent_run_id,
                    name,
                    description,
                    parent,
                    parent_tool_call_id,
                    parent_message_id,
                )| SubagentStartedEvent {
                    base,
                    subagent_run_id: subagent_run_id.into(),
                    name,
                    description,
                    parent_subagent_run_id: parent.map(SubagentRunId::new),
                    parent_tool_call_id: parent_tool_call_id.map(ToolCallId::new),
                    parent_message_id
                }
                .into()
            ),
        (base(), id(), opt(json_value(false)), opt(sub_outcome())).prop_map(
            |(base, subagent_run_id, result, outcome)| SubagentFinishedEvent {
                base,
                subagent_run_id: subagent_run_id.into(),
                result,
                outcome
            }
            .into()
        ),
        (base(), id(), text(), opt(text())).prop_map(|(base, subagent_run_id, message, code)| {
            SubagentErrorEvent {
                base,
                subagent_run_id: subagent_run_id.into(),
                message,
                code,
            }
            .into()
        }),
    ]
    .boxed()
}

fn event() -> impl Strategy<Value = Event> {
    prop_oneof![event_a(), event_b()]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn events_round_trip_through_text(event in event()) {
        let text = serde_json::to_string(&event).unwrap();
        let back: Event = serde_json::from_str(&text).unwrap();
        prop_assert_eq!(&back, &event);
        // And through a Value, the path the projector's goldens take.
        let value = serde_json::to_value(&event).unwrap();
        prop_assert_eq!(serde_json::from_value::<Event>(value).unwrap(), event);
    }

    #[test]
    fn events_conform_to_the_schema(event in event()) {
        assert_conforms(&event);
    }

    #[test]
    fn events_carry_their_type_tag(event in event()) {
        let value = serde_json::to_value(&event).unwrap();
        prop_assert_eq!(value["type"].as_str().unwrap(), event.event_type().as_str());
    }

    #[test]
    fn inputs_round_trip_and_conform(input in input()) {
        assert_input_conforms(&input);
        let text = serde_json::to_vec(&input).unwrap();
        let parsed = RunAgentInput::parse(&text).unwrap();
        prop_assert!(parsed.dropped.is_empty(), "{:?}", parsed.dropped);
        prop_assert_eq!(parsed.input, input);
    }

    #[test]
    fn unknown_members_never_change_the_typed_view(input in input(), junk in "[a-z]{3,8}") {
        let clean = serde_json::to_value(&input).unwrap();
        let mut noisy = clean.clone();
        noisy.as_object_mut().unwrap().insert(format!("x-{junk}"), json!({"k": [1, null]}));
        if let Some(first) = noisy["messages"].get_mut(0).and_then(Value::as_object_mut) {
            first.insert(format!("x-{junk}"), json!(1));
        }
        let parsed = RunAgentInput::from_value(noisy).unwrap();
        prop_assert_eq!(&parsed.input, &input);
        prop_assert!(!parsed.dropped.is_empty());
        prop_assert_eq!(serde_json::to_value(&parsed.input).unwrap(), clean);
    }
}
