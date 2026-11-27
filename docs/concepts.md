# Concepts

The words this documentation uses, each in a line or two, with the page that says the rest. The pages after this one in
Architecture are the long form: [Architecture](architecture.md) for the components and the lifecycles,
[Orchestrator](orchestrator.md) for the ports, the event model and the data.

| Word | What it is |
|---|---|
| **Thread** | One conversation. It has an owner, a state (`queued`, `working`, `verifying`, `blocked`, `done`, `failed` or `cancelled`) and an event log. Only the pure core moves its state, by `transition(&state, &event)`. [Thread state](orchestrator.md#thread-state-and-transitions) |
| **Job** | The work a message starts on a thread that has no open job, kept on the thread as its job ledger (the attempt, the pushed commit, the check results). [Job lifecycle](architecture.md#job-lifecycle) |
| **Event log** | The append-only record of everything that happened on a thread. It is the chat: the screen is a projection of it. It lives in Postgres (SQLite when the orchestrator is embedded, [ADR 0076](decisions/0076-the-orchestrator-as-a-library-in-an-application.md)); what else persists is listed under invariant 3 of the agent guide, each kept beside the log, not in it. [ADR 0001](decisions/0001-rust-state-machine-on-postgres.md) |
| **Agent** | Remote agents are reached by an A2A agent-card URL; behind the off-by-default feature `agent-local` a worker may host adam agents in-process ([ADR 0015](decisions/0015-control-plane-and-workers-on-adam-rs.md)). The orchestrator has no code path of its own for any one agent. [Agent hosts](architecture.md#agent-hosts) |
| **Dispatcher** | The worker loop that claims a row of the outbox, talks to the agent over A2A and turns what the agent reports into events applied to the thread. Any replica may run it. [Event flow](orchestrator.md#event-flow) |
| **Outbox** | The table of commands the core decided on (delegate, cancel, verify, ask and others), written in the same transaction as the state change. A row that dies re-enters the core as a delivery failure, so the thread goes `blocked` (retryable) or `failed`, never on as if it was sent. [Command lifecycle](orchestrator.md#command-outbox-lifecycle) |
| **Extension** | An optional, capability-detected addition to a standard protocol, such as an A2A extension an agent lists on its card. It is read live, fails closed and can be removed without breaking plain A2A. [A2A extensions](api/extensions.md), [ADR 0008](decisions/0008-platform-integration-via-a2a-extension.md) |
