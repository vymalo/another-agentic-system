import type { components } from "../src/lib/api/schema";

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

/**
 * The surface of the orchestrator's `ui` scenario (`docs/api/examples/a2ui.events.json`): a
 * Column with a title and a Button whose action is an `event`, sent in two payloads the way the
 * fake agent sends them (`v0.9.1`, the version the orchestrator relays).
 */
export const UI_SURFACE_ID = "s1";
const CATALOG = "https://a2ui.org/specification/v0_9_1/catalogs/basic/catalog.json";
const uiCreate = {
  version: "v0.9.1",
  createSurface: { surfaceId: UI_SURFACE_ID, catalogId: CATALOG },
};
const uiComponents = {
  version: "v0.9.1",
  updateComponents: {
    surfaceId: UI_SURFACE_ID,
    components: [
      { id: "root", component: "Column", children: ["title", "go"] },
      { id: "title", component: "Text", text: "Pick one" },
      { id: "go_label", component: "Text", text: "Go" },
      {
        id: "go",
        component: "Button",
        child: "go_label",
        variant: "primary",
        action: { event: { name: "go", context: { choice: "a" } } },
      },
    ],
  },
};

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
 * - `ui`: sends an A2UI surface (a title and a button) with the question "Pick one" and blocks; the
 *   owner's action on the surface (`forwardedProps.a2uiAction`) resumes to done, as `ui-action <name>`.
 * - `slow`: works until cancelled.
 * - `fail`: `agent_status: failed` with detail, thread failed.
 * - `talk`: a status with text, one agent message, the result.
 * - anything else (`echo`): working, result artifact (a PR link), done.
 *
 * Mock-only, not produced by the current orchestrator:
 * - `ui-bad`: an A2UI surface the renderer refuses, then the result and done.
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
    case "ui":
      return {
        start: [
          working,
          { kind: "ui_surface", data: { operations: [uiCreate] } },
          { kind: "ui_surface", data: { operations: [uiComponents] } },
          { kind: "agent_status", data: { status: "input_required", detail: "Pick one" } },
          { kind: "thread_state", data: { state: "blocked" }, setState: "blocked", system: true },
        ],
        resume: (answer) => [working, ...finish(`answered: ${answer}`)],
      };
    case "ui-bad":
      // mock only: a surface the renderer refuses (a component outside its vocabulary, and a link
      // that is not http(s)); the thread finishes, so the refusal is what the owner sees
      return {
        start: [
          working,
          {
            kind: "ui_surface",
            data: {
              operations: [
                uiCreate,
                {
                  version: "v0.9.1",
                  updateComponents: {
                    surfaceId: UI_SURFACE_ID,
                    components: [
                      {
                        id: "root",
                        component: "Column",
                        children: ["title", "icon", "open", "open_label"],
                      },
                      { id: "title", component: "Text", text: "Not shown" },
                      { id: "icon", component: "Icon", name: "check" },
                      { id: "open_label", component: "Text", text: "Open" },
                      {
                        id: "open",
                        component: "Button",
                        child: "open_label",
                        action: {
                          functionCall: { call: "openUrl", args: { url: "javascript:alert(1)" } },
                        },
                      },
                    ],
                  },
                },
              ],
            },
          },
          ...finish(`echo: ${text}`),
        ],
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
