//! The rules of a team preset: which saves are refused and where, that a save the rules accept reads
//! back whole, and that the refusals the team ledger owns are **its** refusals.

use super::*;
// the ledger's, reused by the preset and reached here through its parent module
use super::super::{DEFAULT_MAX_HANDOFFS, MAX_HANDOFFS_CEILING, MIN_MAX_HANDOFFS};

fn member(name: &str) -> TeamMemberInput {
    TeamMemberInput {
        name: name.to_owned(),
        target: AgentTarget {
            agent_id: crate::ids::AgentId::new("coder"),
            release: None,
        },
        role: "writes it".to_owned(),
        history: MemberHistory::AgentKeeps,
    }
}

fn save() -> TeamPresetInput {
    TeamPresetInput {
        name: "ship-it".to_owned(),
        title: "Ship it".to_owned(),
        lead: "lead".to_owned(),
        members: vec![member("lead"), member("helper")],
        charter: "One change, then a review.".to_owned(),
        max_handoffs: DEFAULT_MAX_HANDOFFS,
        handoff_approval: HandoffApproval::Ask,
        checkpoints: CheckpointRules::default(),
        push_tokens: DEFAULT_PUSH_TOKENS,
        summary: false,
    }
}

fn id() -> TeamPresetId {
    TeamPresetId::from_entropy([7; 16])
}

#[test]
fn a_valid_save_reads_back_whole() {
    let input = TeamPresetInput {
        handoff_approval: HandoffApproval::After { seconds: 45 },
        checkpoints: CheckpointRules {
            after: ["lead".to_owned()].into_iter().collect(),
            before_pull_request: true,
        },
        max_handoffs: 12,
        push_tokens: 80_000,
        summary: true,
        ..save()
    };
    let preset = TeamPreset::new(id(), input.clone()).expect("valid");
    assert_eq!(preset.id(), &id());
    assert_eq!(preset.name(), "ship-it");
    assert_eq!(preset.title(), "Ship it");
    assert_eq!(preset.lead().as_str(), "lead");
    assert_eq!(preset.members().len(), 2);
    assert_eq!(
        preset.member("helper").expect("a member").role(),
        "writes it"
    );
    assert!(preset.member("nobody").is_none());
    assert_eq!(preset.charter(), "One change, then a review.");
    assert_eq!(preset.max_handoffs(), 12);
    assert_eq!(preset.cap(), 12);
    assert_eq!(preset.handoff_approval().seconds(), Some(45));
    assert!(preset.checkpoints().before_pull_request);
    assert_eq!(preset.push_tokens(), 80_000);
    assert!(preset.summary());
    assert_eq!(preset.to_input(), input);
}

/// A preset is formed as a team: the ledger takes its members and its cap without a second set of
/// rules.
#[test]
fn a_preset_forms_a_team() {
    let preset = TeamPreset::new(
        id(),
        TeamPresetInput {
            max_handoffs: MAX_HANDOFFS_CEILING,
            ..save()
        },
    )
    .expect("valid");
    let ledger = TeamLedger::formed(
        preset.lead().clone(),
        preset.ledger_members(),
        Some(preset.max_handoffs()),
    )
    .expect("the ledger forms it");
    assert_eq!(ledger.members().len(), 2);
    assert_eq!(ledger.cap(), MAX_HANDOFFS_CEILING);
    assert_eq!(ledger.lead(), preset.lead());
    assert_eq!(ledger.members()[0].agent.as_str(), "coder");
}

#[test]
fn the_defaults_are_the_adrs() {
    assert_eq!(HandoffApproval::default(), HandoffApproval::Ask);
    assert_eq!(MemberHistory::default(), MemberHistory::AgentKeeps);
    assert_eq!(CheckpointRules::default().after.len(), 0);
    assert!(!CheckpointRules::default().before_pull_request);
    assert_eq!(DEFAULT_MAX_HANDOFFS, 6);
    assert_eq!(MIN_MAX_HANDOFFS, 6);
    assert_eq!(MAX_HANDOFFS_CEILING, 20);
    assert_eq!(DEFAULT_PUSH_TOKENS, 24_000);
    assert_eq!(HandoffApproval::Auto.seconds(), None);
    assert_eq!(HandoffApproval::Ask.seconds(), None);
}

#[test]
fn a_name_that_is_not_a_slug_is_refused() {
    for bad in ["", "Ship it", "-lead", "A", "a".repeat(33).as_str()] {
        let error = TeamPreset::new(
            id(),
            TeamPresetInput {
                name: bad.to_owned(),
                ..save()
            },
        )
        .expect_err("refused");
        assert_eq!(
            (error.code, error.at.as_str()),
            ("bad_name", "name"),
            "{bad}"
        );
        assert_eq!(error.refusal, None);
    }
    assert!(
        TeamPreset::new(
            id(),
            TeamPresetInput {
                name: "a".repeat(MAX_NAME_CHARS),
                ..save()
            }
        )
        .is_ok()
    );
}

#[test]
fn the_title_and_the_charter_are_bounded() {
    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            title: String::new(),
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!(error.code, "bad_title");
    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            charter: "x".repeat(MAX_CHARTER_BYTES + 1),
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!((error.code, error.at.as_str()), ("bad_charter", "charter"));
    assert!(
        TeamPreset::new(
            id(),
            TeamPresetInput {
                charter: "x".repeat(MAX_CHARTER_BYTES),
                ..save()
            }
        )
        .is_ok()
    );
    assert!(
        TeamPreset::new(
            id(),
            TeamPresetInput {
                title: "x".repeat(MAX_TEAM_TITLE_CHARS),
                ..save()
            }
        )
        .is_ok()
    );
}

#[test]
fn nine_members_are_refused_by_the_ledgers_own_refusal() {
    let nine = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            members: nine.iter().map(|m| member(m)).collect(),
            lead: "a".to_owned(),
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!((error.code, error.at.as_str()), ("bad_members", "members"));
    assert_eq!(error.refusal, Some(TeamRefusal::TooManyMembers));
    assert!(error.to_string().contains("8"), "{error}");
    // one member is no team either: a thread of one agent keeps no team state
    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            members: vec![member("a")],
            lead: "a".to_owned(),
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!(error.code, "bad_members");
    assert_eq!(error.refusal, Some(TeamRefusal::TooFewMembers));
    assert!(
        TeamPreset::new(
            id(),
            TeamPresetInput {
                members: ["a", "b", "c", "d", "e", "f", "g", "h"]
                    .iter()
                    .map(|m| member(m))
                    .collect(),
                lead: "a".to_owned(),
                ..save()
            }
        )
        .is_ok()
    );
}

#[test]
fn a_member_that_repeats_a_name_is_refused() {
    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            members: vec![member("lead"), member("lead")],
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!(
        (error.code, error.at.as_str()),
        ("bad_member_name", "members.1.name")
    );
    assert_eq!(error.refusal, Some(TeamRefusal::DuplicateName));
    // two members may be the same agent (two coders): that is the whole point of a member's name
    let same_agent = TeamPreset::new(
        id(),
        TeamPresetInput {
            members: vec![member("coder-a"), member("coder-b")],
            lead: "coder-a".to_owned(),
            ..save()
        },
    )
    .expect("two members of one agent");
    assert_eq!(
        same_agent.members()[0].target(),
        same_agent.members()[1].target()
    );
}

#[test]
fn a_member_name_is_the_ledgers() {
    // the ledger's rule, not a second one: `@` is optional, `_` and capitals are names
    let preset = TeamPreset::new(
        id(),
        TeamPresetInput {
            members: vec![member("@Lead"), member("Coder_B")],
            lead: "Lead".to_owned(),
            ..save()
        },
    )
    .expect("the ledger's names");
    assert_eq!(preset.members()[0].name().as_str(), "Lead");
    assert_eq!(preset.members()[1].name().as_str(), "Coder_B");

    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            members: vec![member("lead"), member("with space")],
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!(
        (error.code, error.at.as_str()),
        ("bad_member_name", "members.1.name")
    );
    assert_eq!(error.refusal, Some(TeamRefusal::NameNotPlain));

    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            members: vec![member("lead"), member("")],
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!(error.refusal, Some(TeamRefusal::EmptyName));

    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            members: vec![member("lead"), member(&"x".repeat(MAX_NAME_CHARS + 1))],
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!(error.refusal, Some(TeamRefusal::NameTooLong));
}

#[test]
fn a_member_with_a_bad_agent_a_role_too_long_or_a_release_that_is_not_one() {
    let mut broken = save();
    broken.members[1].target.agent_id = crate::ids::AgentId::new("Coder!");
    let error = TeamPreset::new(id(), broken).expect_err("refused");
    assert_eq!(
        (error.code, error.at.as_str()),
        ("bad_member_agent", "members.1.agentId")
    );
    assert_eq!(error.refusal, None);

    let mut broken = save();
    broken.members[1].role = "x".repeat(MAX_ROLE_CHARS + 1);
    let error = TeamPreset::new(id(), broken).expect_err("refused");
    assert_eq!(
        (error.code, error.at.as_str()),
        ("bad_member_role", "members.1.role")
    );
    assert_eq!(error.refusal, Some(TeamRefusal::RoleTooLong));
    assert!(
        TeamPreset::new(
            id(),
            TeamPresetInput {
                members: vec![
                    member("lead"),
                    TeamMemberInput {
                        role: "x".repeat(MAX_ROLE_CHARS),
                        ..member("helper")
                    },
                ],
                ..save()
            }
        )
        .is_ok()
    );

    let mut broken = save();
    broken.members[0].target.release = Some(String::new());
    let error = TeamPreset::new(id(), broken).expect_err("refused");
    assert_eq!(
        (error.code, error.at.as_str()),
        ("bad_member_release", "members.0.release")
    );
    let released = TeamPreset::new(
        id(),
        TeamPresetInput {
            members: vec![
                TeamMemberInput {
                    target: AgentTarget {
                        agent_id: crate::ids::AgentId::new("coder"),
                        release: Some("stable".to_owned()),
                    },
                    ..member("lead")
                },
                member("helper"),
            ],
            ..save()
        },
    )
    .expect("a release");
    assert_eq!(
        released.members()[0].target().release.as_deref(),
        Some("stable")
    );
}

#[test]
fn the_lead_is_a_members_name() {
    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            lead: "nobody".to_owned(),
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!((error.code, error.at.as_str()), ("bad_lead", "lead"));
    assert_eq!(error.refusal, Some(TeamRefusal::LeadNotMember));
    assert!(error.to_string().contains("not a member"), "{error}");

    // a lead that is no name at all is the ledger's own name refusal
    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            lead: "with space".to_owned(),
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!(error.refusal, Some(TeamRefusal::NameNotPlain));
}

#[test]
fn the_cap_of_handoffs_is_the_ledgers() {
    for bad in [0, 1, 5, 21, 100] {
        let error = TeamPreset::new(
            id(),
            TeamPresetInput {
                max_handoffs: bad,
                ..save()
            },
        )
        .expect_err("refused");
        assert_eq!(
            (error.code, error.at.as_str()),
            ("bad_max_handoffs", "maxHandoffs"),
            "{bad}"
        );
        assert_eq!(error.refusal, Some(TeamRefusal::HandoffCapOutOfRange(bad)));
        assert!(error.to_string().contains("6 to 20"), "{error}");
    }
    for good in [MIN_MAX_HANDOFFS, 13, MAX_HANDOFFS_CEILING] {
        assert!(
            TeamPreset::new(
                id(),
                TeamPresetInput {
                    max_handoffs: good,
                    ..save()
                }
            )
            .is_ok(),
            "{good}"
        );
    }
}

#[test]
fn the_wait_of_an_approval_is_five_to_six_hundred_seconds() {
    for bad in [0, 4, 601, 65_535] {
        let error = TeamPreset::new(
            id(),
            TeamPresetInput {
                handoff_approval: HandoffApproval::After { seconds: bad },
                ..save()
            },
        )
        .expect_err("refused");
        assert_eq!(
            (error.code, error.at.as_str()),
            ("bad_handoff_approval", "handoffApproval"),
            "{bad}"
        );
        assert_eq!(error.refusal, None, "the ledger does not own the wait");
        assert!(error.to_string().contains("5 to 600"), "{error}");
    }
    for good in [MIN_APPROVAL_SECONDS, 300, MAX_APPROVAL_SECONDS] {
        assert!(
            TeamPreset::new(
                id(),
                TeamPresetInput {
                    handoff_approval: HandoffApproval::After { seconds: good },
                    ..save()
                }
            )
            .is_ok(),
            "{good}"
        );
    }
}

#[test]
fn a_checkpoint_rule_must_name_a_member() {
    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            checkpoints: CheckpointRules {
                after: ["ghost".to_owned()].into_iter().collect(),
                before_pull_request: false,
            },
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!(
        (error.code, error.at.as_str()),
        ("bad_checkpoint", "checkpoints.after.ghost")
    );
}

#[test]
fn the_budget_of_a_push_is_four_thousand_to_a_hundred_thousand() {
    for bad in [0, 1, 3_999, 100_001] {
        let error = TeamPreset::new(
            id(),
            TeamPresetInput {
                push_tokens: bad,
                ..save()
            },
        )
        .expect_err("refused");
        assert_eq!(
            (error.code, error.at.as_str()),
            ("bad_push_tokens", "pushTokens"),
            "{bad}"
        );
        assert!(error.to_string().contains("4000 to 100000"), "{error}");
    }
    for good in [MIN_PUSH_TOKENS, 60_000, MAX_PUSH_TOKENS] {
        assert!(
            TeamPreset::new(
                id(),
                TeamPresetInput {
                    push_tokens: good,
                    ..save()
                }
            )
            .is_ok(),
            "{good}"
        );
    }
}

#[test]
fn an_id_is_t_and_twelve_base36_characters() {
    let id = TeamPresetId::from_entropy([0; 16]);
    assert_eq!(id.as_str(), "t-000000000000");
    let id = TeamPresetId::from_entropy([0xff; 16]);
    assert_eq!(id.as_str().len(), 14);
    assert!(TeamPresetId::parse(id.as_str()).is_ok());
    for bad in [
        "t-",
        "t-01234567890",
        "t-0123456789012",
        "p-012345678901",
        "t-01234567890A",
    ] {
        assert!(TeamPresetId::parse(bad).is_err(), "{bad}");
    }
    assert!(TeamPresetId::parse("t-012345678901").is_ok());
    assert_eq!(
        TeamPresetId::from_entropy([9; 16]),
        TeamPresetId::from_entropy([9; 16])
    );
}

#[test]
fn a_member_name_is_displayed_as_the_ledger_displays_it() {
    let name = MemberName::new("coder-a").expect("a name");
    assert_eq!(name.as_str(), "coder-a");
    assert_eq!(name.to_string(), "coder-a");
    assert_eq!(MemberName::new("@coder-a").expect("with its @"), name);
    assert!(MemberName::new("coder a").is_err());
    assert!(MemberName::new("").is_err());
}

#[test]
fn a_refused_save_is_invalid_and_holds_no_value_of_the_preset() {
    use crate::error::Classify;
    let error = TeamPreset::new(
        id(),
        TeamPresetInput {
            name: "SHIP".to_owned(),
            ..save()
        },
    )
    .expect_err("refused");
    assert_eq!(error.class(), crate::ErrorClass::Invalid);
    assert!(!error.to_string().contains("SHIP"));
    assert!(!format!("{error:?}").contains("SHIP"));
}
