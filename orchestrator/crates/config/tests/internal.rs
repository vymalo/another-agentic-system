//! `server.internal`: the run routes' own listener and its tokens (ADR 0084 decision 5).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_config::{Role, SecretRef};
use support::{Fake, LONG, lines, load};

/// A file with only what is required, and the run routes' listener on.
fn text(internal: &str) -> String {
    format!(
        "\
version: 1
server:
  internal:
{internal}
database:
  url: {{ env: DATABASE_URL }}
agents:
  file: agents.yaml
"
    )
}

fn env() -> Fake {
    Fake::default()
        .env("DATABASE_URL", "postgres://u:pw@db/orch")
        .env("ORCH_RUN_TOKEN", LONG)
        .env(
            "AGENT_RUN_TOKEN",
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        )
}

/// The section's keys, indented as `server.internal` holds them. Built with `concat!` and not
/// with a `"\`-continued literal: a backslash before a newline strips the next line's leading
/// white space, and the first line would then sit at column 1, under `internal:`.
const SECTION: &str = concat!(
    "    token: { env: ORCH_RUN_TOKEN }\n",
    "    agents:\n",
    "      model-a: { env: AGENT_RUN_TOKEN }\n",
);

#[test]
fn the_listener_is_its_own_address_and_its_tokens_are_resolved() {
    let valid = load(&text(SECTION), &env()).unwrap();
    let internal = valid.config.server.internal.as_ref().unwrap();
    assert_eq!(internal.listen, "0.0.0.0:8081", "its own port, not 8080");
    assert_eq!(internal.token, SecretRef::Env("ORCH_RUN_TOKEN".to_owned()));
    assert_eq!(
        internal
            .agents
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["model-a"],
        "one token per agent, by agent id"
    );
    assert_eq!(
        valid.secrets.internal_token.as_ref().unwrap().expose(),
        LONG
    );
    assert_eq!(
        valid
            .secrets
            .internal_agent_tokens
            .get("model-a")
            .unwrap()
            .expose(),
        "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
    );
    // The configuration never prints a token.
    let shown = format!("{valid:?}");
    assert!(shown.contains("ORCH_RUN_TOKEN"), "{shown}");
    assert!(!shown.contains(LONG), "{shown}");
}

#[test]
fn without_the_section_there_is_no_second_listener() {
    let valid = load(
        "version: 1\ndatabase: { url: { env: DATABASE_URL } }\nagents: { file: agents.yaml }\n",
        &env(),
    )
    .unwrap();
    assert!(valid.config.server.internal.is_none());
    assert!(valid.secrets.internal_token.is_none());
    assert!(valid.secrets.internal_agent_tokens.is_empty());
}

#[test]
fn the_section_is_closed() {
    for line in [
        "    surprise: true\n",
        "    token: { env: ORCH_RUN_TOKEN }\n    agents: { model-a: { env: AGENT_RUN_TOKEN } }\n    port: 8081\n",
    ] {
        let errors = lines(load(&text(line), &env()));
        assert!(
            errors.iter().any(|e| e.contains("server.internal")),
            "{line}: {errors:?}"
        );
    }
    // `token` is required: the listener would otherwise answer nobody.
    let errors = lines(load(&text("    listen: 0.0.0.0:8081\n"), &env()));
    assert!(
        errors.iter().any(|e| e.contains("server.internal.token")),
        "{errors:?}"
    );
}

#[test]
fn the_address_is_a_socket_address_of_its_own() {
    for (internal, says) in [
        (
            "    listen: nowhere\n    token: { env: ORCH_RUN_TOKEN }\n",
            "not a socket address",
        ),
        (
            "    listen: 0.0.0.0:8080\n    token: { env: ORCH_RUN_TOKEN }\n",
            "is server.listen",
        ),
    ] {
        let errors = lines(load(&text(internal), &env()));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("server.internal.listen") && e.contains(says)),
            "{internal}: {errors:?}"
        );
    }
    // A different address on another interface is fine.
    let valid = load(
        &text("    listen: 127.0.0.1:8081\n    token: { env: ORCH_RUN_TOKEN }\n"),
        &env(),
    )
    .unwrap();
    assert_eq!(
        valid.config.server.internal.unwrap().listen,
        "127.0.0.1:8081"
    );
}

#[test]
fn a_short_token_is_refused_and_never_echoed() {
    let short = "too-short";
    let fake = env().env("SHORT_RUN_TOKEN", short);
    let errors = lines(load(&text("    token: { env: SHORT_RUN_TOKEN }\n"), &fake));
    assert!(
        errors
            .iter()
            .any(|e| e.contains("server.internal.token") && e.contains("shorter than")),
        "{errors:?}"
    );
    assert!(
        errors.iter().all(|e| !e.contains(short)),
        "a refused token is not printed: {errors:?}"
    );
    // The same bound for an agent's.
    let fake = env().env("AGENT_SHORT", short);
    let errors = lines(load(
        &text(
            "    token: { env: ORCH_RUN_TOKEN }\n    agents:\n      model-a: { env: AGENT_SHORT }\n",
        ),
        &fake,
    ));
    assert!(
        errors
            .iter()
            .any(|e| e.contains("server.internal.agents.model-a") && e.contains("shorter than")),
        "{errors:?}"
    );
}

#[test]
fn two_callers_may_not_share_a_token() {
    // The agent's token is the orchestrator's: the door checks the orchestrator's first, so the
    // agent would bind and unbind instead of leasing.
    let errors = lines(load(
        &text(
            "    token: { env: ORCH_RUN_TOKEN }\n    agents:\n      model-a: { env: ORCH_RUN_TOKEN }\n",
        ),
        &env(),
    ));
    assert!(
        errors
            .iter()
            .any(|e| e.contains("server.internal.agents.model-a") && e.contains("same token")),
        "{errors:?}"
    );
    // Two agents with one token: the second is refused, whichever it is.
    let errors = lines(load(
        &text(
            "    token: { env: ORCH_RUN_TOKEN }\n    agents:\n      model-a: { env: AGENT_RUN_TOKEN }\n      model-b: { env: AGENT_RUN_TOKEN }\n",
        ),
        &env(),
    ));
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("server.internal.agents.model-b"),
        "{errors:?}"
    );
}

#[test]
fn a_token_is_keyed_by_an_agent_id() {
    for bad in ["Model A", "model.a", "-model", ""] {
        let internal = format!(
            "    token: {{ env: ORCH_RUN_TOKEN }}\n    agents:\n      \"{bad}\": {{ env: AGENT_RUN_TOKEN }}\n"
        );
        let errors = lines(load(&text(&internal), &env()));
        assert!(
            errors
                .iter()
                .any(|e| e.contains("server.internal.agents") && e.contains("not an agent id")),
            "{bad:?}: {errors:?}"
        );
    }
}

#[test]
fn a_worker_reads_and_checks_the_section_it_never_opens() {
    let fake = env();
    let file = format!(
        "version: 1\nserver:\n  role: worker\n  internal:\n{SECTION}database: {{ url: {{ env: DATABASE_URL }} }}\nagents: {{ file: agents.yaml }}\n"
    );
    let valid = load(&file, &fake).unwrap();
    assert_eq!(valid.config.server.role, Role::Worker);
    assert!(valid.config.server.internal.is_some());
    // A wrong section stops a worker too: the file is shared, and a typo is heard at once.
    let file = file.replace(
        "    token: { env: ORCH_RUN_TOKEN }",
        "    token: { env: MISSING }",
    );
    let errors = lines(load(&file, &fake));
    assert!(errors.iter().any(|e| e.contains("MISSING")), "{errors:?}");
}
