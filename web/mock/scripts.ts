import type { components } from "../src/lib/api/schema";

type ThreadState = components["schemas"]["ThreadState"];
type EventKind = components["schemas"]["EventKind"];
type EventData = components["schemas"]["EventData"];
type CheckSource = components["schemas"]["CheckSource"];

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

/** The gate of a verification scenario (ADR 0018): the sources that must pass and the attempts. */
export type Gate = { require: CheckSource[]; maxAttempts: number; verifier?: string };

const GATE_CHECKS: Gate = { require: ["agent_checks"], maxAttempts: 3 };
/** The verifier agent of the verifier scenarios (`VERIFIER` in fixtures.ts), the only source required. */
const GATE_VERIFIER: Gate = { require: ["verifier"], maxAttempts: 3, verifier: "verifier" };
const VERIFIER_FINDING = "src/login.rs: the empty password is accepted";
const REPOSITORY = "https://github.com/acme/demo.git";
const FINDING = "tests::login fails: expected 200, got 500";

/** The fake agent's commit of an attempt: the attempt as 40 hex digits. */
const commitOf = (attempt: number): string => attempt.toString(16).padStart(40, "0");

/** The agent's `branch` and `checks` artifacts (keys in the order the orchestrator's JSON has). */
function pushed(attempt: number, passes: boolean): Step[] {
  const commit = commitOf(attempt);
  const checks = passes
    ? { commit, passed: true, summary: "3 tests passed" }
    : { commit, findings: [FINDING], passed: false, summary: "1 test failed" };
  return [
    {
      kind: "artifact",
      data: {
        name: "branch",
        mimeType: "application/json",
        text: JSON.stringify({ branch: "agent/fix", commit, repository: REPOSITORY }),
      },
    },
    {
      kind: "artifact",
      data: { name: "checks", mimeType: "application/json", text: JSON.stringify(checks) },
    },
  ];
}

/** The agent finished; under the gate the thread is now verified. */
const completedAndVerifying: Step = {
  kind: "agent_status",
  data: { status: "completed" },
  setState: "verifying",
};

/** The orchestrator's answer of the agent-checks source for an attempt. */
function checked(attempt: number, passes: boolean): Step {
  return {
    kind: "check_result",
    system: true,
    data: passes
      ? {
          attempt,
          commit: commitOf(attempt),
          source: "agent_checks",
          status: "passed",
          summary: "3 tests passed",
        }
      : {
          attempt,
          commit: commitOf(attempt),
          findings: [FINDING],
          source: "agent_checks",
          status: "failed",
          summary: "1 test failed",
        },
  };
}

/** The gate failed and the agent is sent back: `attempt` is the one that starts. */
function reworked(attempt: number): Step {
  return {
    kind: "rework",
    system: true,
    setState: "queued",
    data: {
      attempt,
      findings: [{ findings: [FINDING], source: "agent_checks" }],
      maxAttempts: GATE_CHECKS.maxAttempts,
    },
  };
}

/** One attempt: the agent works, pushes and finishes; the checks answer. */
function attemptSteps(attempt: number, passes: boolean): Step[] {
  return [working, ...pushed(attempt, passes), completedAndVerifying, checked(attempt, passes)];
}

/** The verifier's `pending` card: the orchestrator asked, and the verifier's subagent starts. */
function asked(attempt: number): Step {
  return {
    kind: "check_result",
    system: true,
    data: { attempt, commit: commitOf(attempt), source: "verifier", status: "pending" },
  };
}

/** The verifier's verdict on an attempt, as the orchestrator records it. */
function verdict(attempt: number, passes: boolean): Step {
  return {
    kind: "check_result",
    system: true,
    data: passes
      ? { attempt, commit: commitOf(attempt), source: "verifier", status: "passed" }
      : {
          attempt,
          commit: commitOf(attempt),
          findings: [VERIFIER_FINDING],
          source: "verifier",
          status: "failed",
        },
  };
}

/** The gate failed on the verifier's findings and the agent is sent back: `attempt` is the one that starts. */
function reworkedByVerifier(attempt: number): Step {
  return {
    kind: "rework",
    system: true,
    setState: "queued",
    data: {
      attempt,
      findings: [{ findings: [VERIFIER_FINDING], source: "verifier" }],
      maxAttempts: GATE_VERIFIER.maxAttempts,
    },
  };
}

/**
 * One attempt under the verifier: the agent works, says what it did, pushes and finishes (it runs
 * no checks of its own), and the verifier answers. `answers: false` leaves the verifier asked and
 * silent.
 */
function verifiedSteps(attempt: number, passes: boolean, answers = true): Step[] {
  const [branch] = pushed(attempt, passes);
  return [
    working,
    {
      kind: "agent_message",
      data: {
        messageId: nextMessageId(),
        final: true,
        text: "I pushed the fix: the empty password is rejected now.",
      },
    },
    branch as Step,
    completedAndVerifying,
    asked(attempt),
    ...(answers ? [verdict(attempt, passes)] : []),
  ];
}

const GATE_CI: Gate = { require: ["ci"], maxAttempts: 3 };
const CI_CHECK = "ci/build";

/** The `ci` golden's report of an attempt: the check, the commit of that attempt, the run it links to. */
function ciReport(attempt: number, passes: boolean): Step {
  return {
    kind: "ci_result",
    system: true,
    data: {
      branch: "agent/fix",
      conclusion: passes ? "success" : "failure",
      name: CI_CHECK,
      provider: "generic",
      repository: "github.com/acme/demo",
      sha: commitOf(attempt),
      summary: passes ? "3 tests passed" : "1 test failed: tests::login",
      url: `https://ci.example.com/runs/${attempt}`,
    },
  };
}

/** One attempt under the CI gate: the agent pushes (no checks of its own) and finishes; CI answers. */
function ciAttemptSteps(attempt: number, passes: boolean): Step[] {
  const commit = commitOf(attempt);
  return [
    working,
    {
      kind: "artifact",
      data: {
        name: "branch",
        mimeType: "application/json",
        text: JSON.stringify({ branch: "agent/fix", commit, repository: REPOSITORY }),
      },
    },
    completedAndVerifying,
    {
      kind: "check_result",
      system: true,
      data: { attempt, commit, source: "ci", status: "pending" },
    },
    ciReport(attempt, passes),
    {
      kind: "check_result",
      system: true,
      data: passes
        ? { attempt, commit, source: "ci", status: "passed", summary: "3 tests passed" }
        : {
            attempt,
            commit,
            findings: [
              `${CI_CHECK}: failure - 1 test failed: tests::login (https://ci.example.com/runs/${attempt})`,
            ],
            source: "ci",
            status: "failed",
            summary: "1 test failed: tests::login",
          },
    },
  ];
}

/** The red report sends the agent back: `attempt` is the one that starts. */
function ciReworked(attempt: number): Step {
  return {
    kind: "rework",
    system: true,
    setState: "queued",
    data: {
      attempt,
      findings: [
        {
          findings: [
            `${CI_CHECK}: failure - 1 test failed: tests::login (https://ci.example.com/runs/${attempt - 1})`,
          ],
          source: "ci",
        },
      ],
      maxAttempts: GATE_CI.maxAttempts,
    },
  };
}

const done: Step = {
  kind: "thread_state",
  data: { state: "done" },
  setState: "done",
  system: true,
};

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
 * - `verify-pass`, `verify-red-once`, `verify-red`: the verification gate (ADR 0018, requires the
 *   agent's own checks, 3 attempts): the checks pass at once, fail once and then pass, or always fail.
 * - `verify-reviewed`: the gate asks a verifier agent (ADR 0018, requires the `verifier` source, 3
 *   attempts): the verifier finds something in attempt 1, the agent is sent back, and the verifier
 *   passes attempt 2. `verify-reviewed-red`: the verifier never passes it (`checks_failed`). The
 *   verifier is a subagent of its own, `sub-verify-<n>`.
 * - `verify-ci`: the gate on CI (ADR 0017, ADR 0018, `ci.required` = `ci/build`): a red `ci/build` for the
 *   first commit, the agent sent back, a green one for the second (the `ci` golden).
 * - `slow`: works until cancelled.
 * - `fail`: `agent_status: failed` with detail, thread failed.
 * - `talk`: a status with text, one agent message, the result.
 * - anything else (`echo`): working, result artifact (a PR link), done.
 *
 * Mock-only, not produced by the current orchestrator:
 * - `ui-bad`: an A2UI surface the renderer refuses, then the result and done.
 * - `verify-ci-stale`: a gate on CI and the agent's checks. CI answers pending, then a stale answer of an
 *   older push, then passes (`check_result` cards replaced in place, a stale one of its own).
 * - `verify-wait`: the same gate, and CI never answers: the thread stays `verifying` until cancelled.
 * - `verify-reviewed-wait`: the verifier is asked and never answers: its subagent stays open (and is
 *   told to a client that joins) until the thread is cancelled.
 * - `partial`: streams a partial `agent_message` and replaces it by its final version.
 * - `unreachable`: the delivery was dead-lettered: an `error` event, thread blocked.
 */
export function scriptFor(text: string): {
  start: Step[];
  resume?: (answer: string) => Step[];
  /** The gate the thread's job runs under; absent: none, and the run ends at `completed`. */
  gate?: Gate;
} {
  const word = text.split(/\s+/).find((w) => w !== "");
  switch (word) {
    case "verify-pass":
      return { gate: GATE_CHECKS, start: [...attemptSteps(1, true), done] };
    case "verify-red-once":
      return {
        gate: GATE_CHECKS,
        start: [...attemptSteps(1, false), reworked(2), ...attemptSteps(2, true), done],
      };
    case "verify-red":
      return {
        gate: GATE_CHECKS,
        start: [
          ...attemptSteps(1, false),
          reworked(2),
          ...attemptSteps(2, false),
          reworked(3),
          ...attemptSteps(3, false),
          {
            kind: "error",
            system: true,
            data: {
              message: `the work did not pass verification after ${GATE_CHECKS.maxAttempts} attempts; the agent's own checks: ${FINDING}`,
              retryable: false,
            },
          },
          { kind: "thread_state", data: { state: "failed" }, setState: "failed", system: true },
        ],
      };
    case "verify-reviewed":
      return {
        gate: GATE_VERIFIER,
        start: [...verifiedSteps(1, false), reworkedByVerifier(2), ...verifiedSteps(2, true), done],
      };
    case "verify-reviewed-red":
      return {
        gate: GATE_VERIFIER,
        start: [
          ...verifiedSteps(1, false),
          reworkedByVerifier(2),
          ...verifiedSteps(2, false),
          reworkedByVerifier(3),
          ...verifiedSteps(3, false),
          {
            kind: "error",
            system: true,
            data: {
              message: `the work did not pass verification after ${GATE_VERIFIER.maxAttempts} attempts; the verifier: ${VERIFIER_FINDING}`,
              retryable: false,
            },
          },
          { kind: "thread_state", data: { state: "failed" }, setState: "failed", system: true },
        ],
      };
    case "verify-reviewed-wait":
      // mock only: the verifier is asked and never answers
      return {
        gate: GATE_VERIFIER,
        start: [...verifiedSteps(1, true, false), { pause: "cancel" }],
      };
    case "verify-ci":
      // the `ci` golden (docs/api/examples/ci.events.json): the agent has no checks of its own
      return {
        gate: GATE_CI,
        start: [...ciAttemptSteps(1, false), ciReworked(2), ...ciAttemptSteps(2, true), done],
      };
    case "verify-ci-stale":
    case "verify-wait": {
      // mock only: a stale answer of an older push is not produced by the current orchestrator
      const commit = commitOf(1);
      const ci = { attempt: 1, name: "build", source: "ci" as const };
      const checkedOnCi: Step[] = [
        {
          kind: "check_result",
          system: true,
          data: {
            attempt: 1,
            commit,
            source: "agent_checks",
            status: "passed",
            summary: "3 tests passed",
          },
        },
        { kind: "check_result", system: true, data: { ...ci, commit, status: "pending" } },
      ];
      const gate: Gate = { require: ["ci", "agent_checks"], maxAttempts: 3 };
      const head = [working, ...pushed(1, true), completedAndVerifying, ...checkedOnCi];
      if (word === "verify-wait") return { gate, start: [...head, { pause: "cancel" }] };
      return {
        gate,
        start: [
          ...head,
          {
            kind: "check_result",
            system: true,
            data: {
              ...ci,
              commit: commitOf(0),
              findings: ["the build of an older push failed"],
              stale: true,
              status: "failed",
              summary: "answered for a commit that is no longer the current one",
            },
          },
          {
            kind: "check_result",
            system: true,
            data: { ...ci, commit, status: "passed", summary: "build passed" },
          },
          done,
        ],
      };
    }
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
