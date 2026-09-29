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

const PR_URL = "https://github.com/vymalo/example/pull/1";

const working: Step = { kind: "agent_status", data: { status: "working" }, setState: "working" };

const finish: Step[] = [
  {
    kind: "artifact",
    data: { name: "pull-request", mimeType: "text/uri-list", uri: PR_URL },
  },
  { kind: "agent_status", data: { status: "completed" } },
  { kind: "thread_state", data: { state: "done" }, setState: "done", system: true },
];

const nextMessageId = (() => {
  let n = 0;
  return () => `m-${++n}`;
})();

function happy(): Step[] {
  const messageId = nextMessageId();
  return [
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
    ...finish,
  ];
}

/**
 * Keyword-driven scripts, chosen from the first message:
 * - "question": asks for input and blocks; a follow-up resumes to done.
 * - "slow": works until cancelled.
 * - "fail": errors out and fails.
 * - anything else: the happy path.
 */
export function scriptFor(text: string): { start: Step[]; resume?: Step[] } {
  const t = text.toLowerCase();
  if (t.includes("question")) {
    return {
      start: [
        working,
        {
          kind: "agent_status",
          data: { status: "input_required", detail: "Which branch should I use?" },
        },
        { kind: "thread_state", data: { state: "blocked" }, setState: "blocked", system: true },
      ],
      resume: [
        working,
        {
          kind: "agent_message",
          data: {
            messageId: nextMessageId(),
            final: true,
            text: "Thanks, continuing on that branch.",
          },
        },
        ...finish,
      ],
    };
  }
  if (t.includes("slow")) {
    return { start: [working, { pause: "cancel" }] };
  }
  if (t.includes("fail")) {
    return {
      start: [
        working,
        { kind: "error", data: { message: "Agent crashed", retryable: false } },
        { kind: "agent_status", data: { status: "failed", detail: "Agent crashed" } },
        { kind: "thread_state", data: { state: "failed" }, setState: "failed", system: true },
      ],
    };
  }
  return { start: happy() };
}

export const cancelSteps: Step[] = [
  { kind: "agent_status", data: { status: "canceled" } },
  { kind: "thread_state", data: { state: "cancelled" }, setState: "cancelled", system: true },
];
