import type { components } from "../src/api/schema";

type ThreadState = components["schemas"]["ThreadState"];
type EventKind = components["schemas"]["EventKind"];
type EventData = components["schemas"]["EventData"];

/** One scripted agent action, played `stepMs` apart. */
export type Step =
  | {
      kind: EventKind;
      data: EventData;
      /** Thread state after the step (visible via GET /api/threads/{id}). */
      setState?: ThreadState;
      /** Emitted by the orchestrator rather than the agent. */
      system?: boolean;
    }
  | { pause: "cancel" };

const PR_URL = "https://github.com/acme/demo/pull/1";

const working: Step = { kind: "agent_status", data: { status: "working" }, setState: "working" };

/** The agent's result, its `completed` status and the orchestrator's `done`. */
function finish(artifactText: string): Step[] {
  return [
    { kind: "artifact", data: { name: "result", text: artifactText, uri: PR_URL } },
    { kind: "agent_status", data: { status: "completed" } },
    { kind: "thread_state", data: { state: "done" }, setState: "done", system: true },
  ];
}

const nextMessageId = (() => {
  let n = 0;
  return () => `m-${++n}`;
})();

/**
 * The scripts tell the story the real orchestrator tells (`docs/api/examples/*.events.json`,
 * written by `orchestrator/crates/e2e/tests/golden.rs`): agent text arrives as one final
 * `agent_message`, an agent failure is `agent_status: failed` with its detail (no `error`
 * event), and the trigger words are those of the orchestrator's fake agent.
 *
 * Trigger words, matched against the FIRST word of the first message (like the fake agent):
 * - `ask`: asks "Which branch?" and blocks; the follow-up resumes to done.
 * - `slow`: works until cancelled.
 * - `fail`: `agent_status: failed` with detail, thread failed.
 * - `talk`: a status with text, one agent message, the result.
 * - anything else (`echo`): working, result artifact (a PR link), done.
 *
 * Mock-only, not produced by the current orchestrator:
 * - `partial`: streams a partial `agent_message` and replaces it by its final version.
 * - `unreachable`: the delivery was dead-lettered: an `error` event, thread blocked.
 */
export function scriptFor(text: string): { start: Step[]; resume?: (answer: string) => Step[] } {
  switch (text.split(/\s+/).find((w) => w !== "")) {
    case "ask":
      return {
        start: [
          working,
          {
            kind: "agent_status",
            data: { status: "input_required", detail: "Which branch?" },
          },
          { kind: "thread_state", data: { state: "blocked" }, setState: "blocked", system: true },
        ],
        resume: (answer) => [working, ...finish(`answered: ${answer}`)],
      };
    case "slow":
      return { start: [working, { pause: "cancel" }] };
    case "fail":
      return {
        start: [
          working,
          { kind: "agent_status", data: { status: "failed", detail: "scripted failure" } },
          { kind: "thread_state", data: { state: "failed" }, setState: "failed", system: true },
        ],
      };
    case "talk":
      return {
        start: [
          working,
          {
            kind: "agent_status",
            data: { status: "working", detail: "Reading the repository" },
          },
          {
            kind: "agent_message",
            data: { messageId: nextMessageId(), final: true, text: "Plan: add a test" },
          },
          ...finish(`echo: ${text}`),
        ],
      };
    case "partial": {
      const messageId = nextMessageId();
      return {
        start: [
          working,
          {
            kind: "agent_message",
            data: { messageId, final: false, text: "I'll start with the failing test" },
          },
          {
            kind: "agent_message",
            data: {
              messageId,
              final: true,
              text: "I'll start with the failing test, then make the smallest change that fixes it.\n\n- add a regression test\n- fix `parse()` and run the suite\n- open a pull request",
            },
          },
          ...finish(`echo: ${text}`),
        ],
      };
    }
    case "unreachable":
      return {
        start: [
          {
            kind: "error",
            data: {
              message: "the agent could not be reached: connection refused",
              retryable: true,
            },
            system: true,
          },
          { kind: "thread_state", data: { state: "blocked" }, setState: "blocked", system: true },
        ],
      };
    default:
      return { start: [working, ...finish(`echo: ${text}`)] };
  }
}

/** The orchestrator's fake agent answers `CancelTask` with the status text "canceled". */
export const cancelSteps: Step[] = [
  { kind: "agent_status", data: { status: "canceled", detail: "canceled" } },
  { kind: "thread_state", data: { state: "cancelled" }, setState: "cancelled", system: true },
];
