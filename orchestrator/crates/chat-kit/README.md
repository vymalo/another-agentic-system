# orch-chat-kit

The chat contract of an application client ([`docs/api/agui.md`](../../../docs/api/agui.md),
"Chat contract of an application client") as an executable conformance kit: one trait over a chat
home, four cases, and a macro that writes one test per case.

`publish = false`. The kit names no transport and no adapter: a consuming crate implements
`ChatHome` for its own home and runs `chat_conformance!`. The two reference implementations are
[`orch-embed`](../embed/README.md) (in process, SQLite) and the HTTP home over
[`orch-surface-agui`](../surface-agui/README.md) and [`orch-api`](../api/README.md), each in a
`tests/chat_kit.rs`.

## The trait

```rust
pub trait ChatHome {
    fn limit_rule(&self) -> LimitRule;
    async fn send(&self, thread: Option<&str>, message_id: &str, text: &str)
        -> Result<Sent, HomeError>;
    async fn replay(&self, thread: &str, after: i64) -> Result<Vec<Frame>, HomeError>;
    async fn user_messages(&self, thread: &str) -> Result<Vec<UserMessage>, HomeError>;
}
```

* `send`: `None` mints a fresh thread, `Some` continues it. `message_id` is the client's own id
  (the AG-UI `messageId`), the idempotency key of a retry. The returned frames are the run's
  stream read to its end, each with the sequence it can be resumed from.
* `replay`: the frames after the log sequence `after` (0 is the start).
* `user_messages`: the thread's user messages, oldest first.
* `limit_rule`: whether the home enforces an application-level limit (`Enforced`) or has none
  (`NotApplicable(reason)`).

The trait is generic and never used as `dyn`; implementations use native `async fn` in the trait.

## The four cases

| Case | What it proves |
|---|---|
| `replay_from_a_seq_says_only_what_follows` | a replay from a sequence number starts with `RUN_STARTED`, ends with `RUN_FINISHED`, and holds exactly the whole thread's resume points after the cursor (no gap, no repeat) |
| `a_client_message_id_makes_a_retry_one_message` | the same `messageId` sent twice is one message in the log |
| `a_limit_is_one_custom_event` | a 402 is one `CUSTOM` named `vymalo.limit_reached` with `{status: 402, code, message}`; a home that reports `NotApplicable(reason)` skips it |
| `an_old_or_unstated_client_is_refused` | a client below the minimum, an unreadable one or none is `UpdateRequired`; a client at the minimum sends |

Each case takes a factory `Fn(Setup) -> impl Future<Output = Option<H>>` (an `async fn` is one)
that builds a fresh, isolated home for the `Setup` the case asks for. `None` means the home is
unavailable: the case prints `skipped: …` and passes, as the port testkits do.

## The macro

```rust
use orch_chat_kit::{Setup, chat_conformance};

async fn make(setup: Setup) -> Option<MyHome> { /* ... */ }

chat_conformance!(make);
```

`chat_conformance!` writes one `#[tokio::test]` per case, named after it. The consuming crate
needs `tokio` with the features `macros` and `rt` as a dev-dependency.

## The 402 capability

The 402 is the application's rule, not the orchestrator's: a home with an application-level limit
(a BFF in front of the HTTP API, or an embed home with an `Admission`) reports
`LimitRule::Enforced` and is held to the event. The orchestrator's HTTP API has no such limit, so
its home reports `NotApplicable`: its own over-quota refusal is `403 allowance_spent`, and
`vymalo.limit_reached` is the application's, per [`docs/api/agui.md`](../../../docs/api/agui.md).
That is a capability of the home, not a gap in the kit.

## Deliberately broken homes

`double::RepeatedReplay` repeats the frame at the cursor and `double::GappedReplay` drops the first
frame after it. The kit's own tests run the replay case against both and require a failure, so the
case is proven to fail a broken home rather than pass everything.

## Consuming it

```toml
[dev-dependencies]
orch-chat-kit = { git = "https://github.com/vymalo/another-agentic-system", rev = "<sha>", package = "orch-chat-kit" }
tokio = { version = "1", features = ["macros", "rt"] }
```

Inside this workspace, `orch-chat-kit = { workspace = true }`.
