# Concepts

The words this documentation uses: one line each, and the rest under **In depth** (click to open). The pages after this
one in Architecture are the long form: [Architecture](architecture.md) for the components and the lifecycles,
[Orchestrator](orchestrator.md) for the ports, the event model and the data.

## Thread

One conversation, with an owner, a state and an event log.

::: in-depth More on the thread
Its state is one of `queued`, `working`, `verifying`, `blocked`, `done`, `failed` or `cancelled`. Only the pure core moves
it, by `transition(&state, &event)`. [Thread state](orchestrator.md#thread-state-and-transitions)
:::

## Job

The work a message starts on a thread.

::: in-depth More on the job
A job is started by a message on a thread that has no open job. It is kept on the thread as its job ledger (the attempt,
the pushed commit, the check results). [Job lifecycle](architecture.md#job-lifecycle)
:::

## Event log

The append-only record of everything that happened on a thread. It is the chat: the screen is a projection of it.

::: in-depth Where it lives, and what else persists
It lives in Postgres (SQLite when the orchestrator is embedded,
[ADR 0076](decisions/0076-the-orchestrator-as-a-library-in-an-application.md)). What else persists is listed under
invariant 3 of the agent guide, each kept beside the log, not in it.
[ADR 0001](decisions/0001-rust-state-machine-on-postgres.md)
:::

## Agent

Something the orchestrator hands work to, reached by an A2A agent-card URL.

::: in-depth Hosts, and the in-process option
Remote agents are reached by an A2A agent-card URL; behind the off-by-default feature `agent-local` a worker may host
adam agents in-process ([ADR 0015](decisions/0015-control-plane-and-workers-on-adam-rs.md)). The orchestrator has no code
path of its own for any one agent. [Agent hosts](architecture.md#agent-hosts)
:::

## Dispatcher

The worker loop that carries a command to an agent and brings back what the agent reports.

::: in-depth What it does, step by step
It claims a row of the outbox, talks to the agent over A2A and turns what the agent reports into events applied to the
thread. Any replica may run it. [Event flow](orchestrator.md#event-flow)

::: in-depth Why a dying row is never lost
This is the outbox's side of the same loop. A row that dies re-enters the core as a delivery failure, so the thread goes
`blocked` (retryable) or `failed`, never on as if it was sent. See **Outbox** below.
:::
:::

## Outbox

The table of commands the core decided on, written in the same transaction as the state change.

::: in-depth The commands, and a row that dies
The commands are delegate, cancel, verify, ask and others. A row that dies re-enters the core as a delivery failure, so
the thread goes `blocked` (retryable) or `failed`, never on as if it was sent.
[Command lifecycle](orchestrator.md#command-outbox-lifecycle)
:::

## Extension

An optional, capability-detected addition to a standard protocol.

::: in-depth How an extension behaves
An example is an A2A extension an agent lists on its card. It is read live, fails closed and can be removed without
breaking plain A2A. [A2A extensions](api/extensions.md),
[ADR 0008](decisions/0008-platform-integration-via-a2a-extension.md)
:::
