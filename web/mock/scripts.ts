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
      /** Played at once after the step before it, not `stepMs` later (a burst of steps). */
      quick?: boolean;
    }
  | { pause: "cancel" }
  | {
      /**
       * A piece of the reply the agent is still writing (live text, ADR 0027): relayed to the
       * viewers, never in the log. `offset` counts UTF-16 code units; the log's message for the
       * same `messageId` says the whole reply when the agent is done.
       */
      live: {
        messageId: string;
        offset: number;
        text: string;
        end?: "open" | "last" | "abandoned";
      };
    };

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

/** The id of the web's own UI catalog (`src/features/chat/lib/a2ui/catalog/catalog.json`). */
const OWN_CATALOG_ID = "https://agents.vymalo.com/a2ui/catalogs/chat";

/** A surface under the web's own catalog: a title, and a component that this build does not have. */
const gizmoSurface = (): Step => ({
  kind: "ui_surface",
  data: {
    operations: [
      { version: "v0.9.1", createSurface: { surfaceId: UI_SURFACE_ID, catalogId: OWN_CATALOG_ID } },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: UI_SURFACE_ID,
          components: [
            { id: "root", component: "Column", children: ["title", "gizmo"] },
            { id: "title", component: "Text", text: "Before the gizmo" },
            { id: "gizmo", component: "Gizmo", spin: 3 },
          ],
        },
      },
    ],
  },
});

/**
 * The surface of the `choices` script: one Choices of three questions under the web's own catalog
 * (the fake agent of the orchestrator plays the same words): a database with an "Other", a login,
 * and where it runs (several, optional, with an "Other").
 */
const choicesSurface = (): Step => ({
  kind: "ui_surface",
  data: {
    operations: [
      { version: "v0.9.1", createSurface: { surfaceId: UI_SURFACE_ID, catalogId: OWN_CATALOG_ID } },
      {
        version: "v0.9.1",
        updateComponents: {
          surfaceId: UI_SURFACE_ID,
          components: [
            { id: "root", component: "Column", children: ["intro", "pick"] },
            { id: "intro", component: "Text", text: "A few quick choices", variant: "h3" },
            {
              id: "pick",
              component: "Choices",
              questions: [
                {
                  id: "db",
                  question: "Which database?",
                  allowOther: true,
                  options: [
                    { value: "pg", label: "Postgres", description: "Relational, the default" },
                    { value: "sqlite", label: "SQLite" },
                  ],
                },
                {
                  id: "auth",
                  question: "Which login?",
                  options: [
                    { value: "keycloak", label: "Keycloak" },
                    { value: "none", label: "No login" },
                  ],
                },
                {
                  id: "deploy",
                  question: "Where does it run?",
                  multiple: true,
                  required: false,
                  allowOther: true,
                  options: [
                    { value: "k8s", label: "Kubernetes" },
                    { value: "compose", label: "Docker Compose" },
                  ],
                },
              ],
            },
          ],
        },
      },
    ],
  },
});

/** One surface of the web's own catalog, as the fake agent sends it: `createSurface`, then the components. */
const ownSurface = (components: Record<string, unknown>[]): Step => ({
  kind: "ui_surface",
  data: {
    operations: [
      { version: "v0.9.1", createSurface: { surfaceId: UI_SURFACE_ID, catalogId: OWN_CATALOG_ID } },
      { version: "v0.9.1", updateComponents: { surfaceId: UI_SURFACE_ID, components } },
    ],
  },
});

/** A graph of the `cards-mermaid` answer: how a request meets a session. */
export const SESSION_GRAPH = [
  "flowchart TD",
  "  A[Request arrives] --> B{Session cookie?}",
  "  B -- yes --> C[Look up the session]",
  "  B -- no --> D[Create a session]",
  "  C --> E[Handle the request]",
  "  D --> E",
].join("\n");

/** The cards of the `cards-mermaid` answer: three ways to keep a login session. */
export const SESSION_CARDS = [
  {
    title: "Server-side sessions in Postgres",
    subtitle: "Durable, one more query per request",
    body: "The session lives in a table. Logging out or revoking a device is one delete.",
    url: "https://www.postgresql.org/docs/current/",
    tags: ["durable", "simple"],
  },
  {
    title: "Signed cookies",
    subtitle: "Nothing to look up",
    body: "The session is in the cookie, signed. Revoking one before it expires needs a deny list.",
    url: "https://owasp.org/www-community/controls/",
    tags: ["stateless"],
  },
  {
    title: "A cache in front of the table",
    subtitle: "Fast, and one more thing to run",
    tags: ["fast", "extra service"],
  },
];

/**
 * The surface of the `cards-mermaid` script: a sentence, three cards (one of them without a link)
 * and a graph, in one column of the web's own catalog: what the researcher answers with.
 */
const cardsMermaidSurface = (): Step =>
  ownSurface([
    { id: "root", component: "Column", children: ["intro", "options", "flow"] },
    {
      id: "intro",
      component: "Text",
      text: "Compared on how each one handles revocation and lookups.",
    },
    {
      id: "options",
      component: "Cards",
      title: "Three ways to keep a session",
      layout: "list",
      cards: SESSION_CARDS,
    },
    {
      id: "flow",
      component: "Mermaid",
      title: "How a request meets a session",
      code: SESSION_GRAPH,
      caption: "A request without a cookie gets a new session.",
    },
  ]);

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
function ciReport(
  attempt: number,
  passes: boolean,
  over: { name?: string; summary?: string } = {},
): Step {
  return {
    kind: "ci_result",
    system: true,
    data: {
      branch: "agent/fix",
      conclusion: passes ? "success" : "failure",
      name: over.name ?? CI_CHECK,
      provider: "generic",
      repository: "github.com/acme/demo",
      sha: commitOf(attempt),
      summary: over.summary ?? (passes ? "3 tests passed" : "1 test failed: tests::login"),
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

/** The agent finished, with no artifact: an answer of words and a surface only. */
const finishQuietly: Step[] = [
  { kind: "agent_status", data: { status: "completed" } },
  { kind: "thread_state", data: { state: "done" }, setState: "done", system: true },
];

/**
 * A graph written to get out of the picture: front matter and a directive that turn security and HTML
 * labels off, change the theme and add CSS, a script in a label, an event handler in a label, a click
 * handler and a link. A page that draws it as an image shows a flowchart and nothing else:
 * `window.__mermaidFlag` stays unset.
 */
export const HOSTILE_GRAPH = [
  "---",
  "config:",
  "  securityLevel: loose",
  "  htmlLabels: true",
  "  theme: forest",
  "---",
  '%%{init: {"securityLevel": "loose", "htmlLabels": true, "flowchart": {"htmlLabels": true}, "theme": "dark", "look": "handDrawn", "themeCSS": ".node rect { fill: red }"}}%%',
  "flowchart TD",
  '  A["<img src=x onerror=window.__mermaidFlag=1>"] --> B["<script>window.__mermaidFlag=2</script>"]',
  "  B --> C[Plain end]",
  "  click A call window.__mermaidFlagCallback()",
  '  click C "https://example.com/" "a link"',
].join("\n");

/** One small graph of each kind mermaid draws that an agent is likely to write: `mermaid-kinds`. */
export const GRAPH_KINDS: [string, string][] = [
  ["Flowchart", "flowchart LR\n  A[Start] --> B{Ok?}\n  B -- yes --> C[Done]\n  B -- no --> A"],
  [
    "Sequence",
    "sequenceDiagram\n  participant W as Web\n  participant O as Orchestrator\n  W->>O: run\n  O-->>W: events",
  ],
  ["Class", "classDiagram\n  class Session {\n    +id\n    +expire()\n  }\n  Session <|-- Cookie"],
  ["State", "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Busy: start\n  Busy --> [*]"],
  ["Entity relationship", "erDiagram\n  USER ||--o{ SESSION : has\n  USER { string name }"],
  [
    "Gantt",
    "gantt\n  title Plan\n  dateFormat YYYY-MM-DD\n  section A\n  Task :a1, 2026-10-01, 3d",
  ],
  ["Pie", 'pie title Share\n  "A" : 60\n  "B" : 40'],
  ["Mind map", "mindmap\n  root((Sessions))\n    Cookies\n    Tokens"],
  ["Timeline", "timeline\n  title Steps\n  2026 : Plan\n  2027 : Ship"],
  ["Git graph", "gitGraph\n  commit\n  branch dev\n  commit\n  checkout main\n  merge dev"],
];

const nextMessageId = (() => {
  let n = 0;
  return () => `m-${++n}`;
})();

/**
 * The pieces of a reply as a sender relays them while the model writes it: `parts` joined is the
 * reply. `last` ends it (the log's message closes the live one), `open` leaves it unfinished, and
 * `abandoned` gives the stream up after its words (the model failed).
 */
function livePieces(
  messageId: string,
  parts: readonly string[],
  end: "last" | "open" | "abandoned" = "last",
): Step[] {
  let offset = 0;
  const steps: Step[] = parts.map((text, i) => {
    const piece: Step = {
      live: {
        messageId,
        offset,
        text,
        end: end === "last" && i === parts.length - 1 ? "last" : "open",
      },
    };
    offset += text.length;
    return piece;
  });
  if (end === "abandoned") steps.push({ live: { messageId, offset, text: "", end: "abandoned" } });
  return steps;
}

/** A reply in Markdown, in the pieces a model writes it in (a paragraph, then a list). */
const LONG_PARTS = [
  "I'll start with the failing test, ",
  "then make the smallest change ",
  "that fixes it.\n\n",
  "- add a regression test for `parse()`\n",
  "- fix the off-by-one in the loop\n",
  "- run the suite and open a pull request\n\n",
  "I will tell you ",
  "when it is green.",
];
const LONG_REPLY = LONG_PARTS.join("");

/**
 * The coder scenarios of the screenshots (`pnpm screens`): a coding agent's run the way a chat shows
 * it, from its steps to a pull request. Mock only; the wording of the steps is the mock's, not
 * adam-coder's (unverified).
 */
const CODER_REPOSITORY = "https://github.com/acme/demo.git";
const CODER_BRANCH = "agent/fix-login-redirect";
const CODER_PR = "https://github.com/acme/demo/pull/12";

/** A `working` status with the agent's words for the step. */
const doing = (detail: string): Step => ({
  kind: "agent_status",
  data: { status: "working", detail },
});

/**
 * One report of a step of the agent's work (an `agent_step` event, ADR 0025). The ids are the
 * orchestrator's (`<task id>/<agent's id>`); the mock has no task ids, so the golden's `T` stands
 * in for it.
 */
const agentStep = (
  id: string,
  path: string[],
  kind: "subagent" | "tool" | "command" | "message",
  label: string,
  state: "running" | "waiting" | "completed" | "failed" | "canceled",
  phase: "start" | "update" | "end",
  icon?: string,
  detail?: string,
): Step => ({
  kind: "agent_step",
  data: {
    id: `T/${id}`,
    path: path.map((p) => `T/${p}`),
    kind,
    label,
    state,
    phase,
    ...(icon ? { icon } : {}),
    ...(detail ? { detail } : {}),
  },
});

/** The coder's branch, checks and pull request artifacts for a commit. */
function coderPushed(
  commit: string,
  checks: { passed: boolean; summary: string; findings?: string[] },
): Step[] {
  return [
    {
      kind: "artifact",
      data: {
        name: "branch",
        mimeType: "application/json",
        text: JSON.stringify({ branch: CODER_BRANCH, commit, repository: CODER_REPOSITORY }),
      },
    },
    {
      kind: "artifact",
      data: {
        name: "checks",
        mimeType: "application/json",
        text: JSON.stringify({ commit, ...checks }),
      },
    },
  ];
}

const coderPullRequest: Step = {
  kind: "artifact",
  data: {
    name: "pull_request",
    mimeType: "application/json",
    text: JSON.stringify({
      branch: CODER_BRANCH,
      number: 12,
      repository: "github.com/acme/demo",
      title: "Fix the redirect loop after signing in",
      url: CODER_PR,
    }),
  },
};

const CODER_SUMMARY = [
  "I fixed the redirect loop after signing in.",
  "",
  "**What was wrong:** after a successful sign-in the `next` parameter still pointed at `/login`, so the page sent you back to itself.",
  "",
  "**What I changed**",
  "- `src/auth/login.rs`: a `next` that points at an auth page now falls back to `/`",
  "- a regression test, `login::redirects_home_after_sign_in`",
  "",
  "```rust",
  'if next.starts_with("/login") {',
  '    next = "/".into();',
  "}",
  "```",
  "",
  "All 42 tests pass, and the pull request is ready for review.",
].join("\n");

/**
 * What the `sources` scenario says: links the panel's Sources tab lists (a doc, the pull request it
 * opened, a run), a link inside code that it must not, and one that is not http(s).
 */
const SOURCES_SUMMARY = [
  `I fixed the redirect loop and opened ${CODER_PR}.`,
  "",
  "- why it loops: see [the axum redirect docs](https://docs.rs/axum/latest/axum/response/struct.Redirect.html)",
  "- the run: [CI on the branch](https://ci.example.com/runs/12)",
  "- run `curl https://example.com/only-in-code` to see it in code (not a source)",
  "- [not a link we open](javascript:alert(1))",
].join("\n");

const coderWork: Step[] = [
  working,
  doing("Preparing the workspace"),
  doing("Reading src/auth/login.rs and its tests"),
  doing("$ cargo test -p auth login::"),
  doing("Fixing the redirect in src/auth/login.rs"),
  doing("$ cargo test -p auth"),
];

/** One child of OpenCode's: a read, an edit, a search or a command, reported once, as it ended. */
const openCodeChild = (
  n: number,
  label: string,
  kind: "tool" | "command",
  icon: string,
  failed?: string,
): Step => ({
  ...agentStep(
    `acp:c2:${n}`,
    ["tool:c2"],
    kind,
    label,
    failed ? "failed" : "completed",
    "end",
    icon,
    failed,
  ),
  quick: true,
});

/** What OpenCode does in the `Delegate` scenarios, in order: a failing test run among them. */
const OPENCODE_WORK: [string, "tool" | "command", string, string?][] = [
  ["read src/auth/login.rs", "tool", "read"],
  ["read src/auth/session.rs", "tool", "read"],
  ["search the repository for next=", "tool", "search"],
  ["read tests/login.rs", "tool", "read"],
  ["cargo build -p auth", "command", "execute"],
  ["cargo test -p auth login::", "command", "execute", "1 failed, 41 passed"],
  ["read the failing test's output", "tool", "read"],
  ["edit src/auth/login.rs", "tool", "edit"],
  ["edit tests/login.rs", "tool", "edit"],
  ["cargo fmt --all", "command", "execute"],
  ["cargo clippy -p auth", "command", "execute"],
  ["cargo test -p auth login::", "command", "execute"],
  ["cargo test -p auth", "command", "execute"],
  ["git diff --stat", "command", "git"],
];

/** The coder hands the work to OpenCode: `count` of its steps from the list above, then its end. */
const openCodeSteps = (count: number, finish: boolean): Step[] => [
  agentStep("tool:c2", [], "subagent", "OpenCode", "running", "start", "agent"),
  ...OPENCODE_WORK.slice(0, count).map(([label, kind, icon, failed], i) =>
    openCodeChild(i + 1, label, kind, icon, failed),
  ),
  ...(finish
    ? [agentStep("tool:c2", [], "subagent", "OpenCode", "completed", "end", "agent")]
    : []),
];

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
 * - `choices`: sends a surface of the web's own catalog with a Choices of three questions (a database
 *   and where it runs, with an "Other", a login: the last one several and optional) and asks "Three questions"; the
 *   answers (`forwardedProps.a2uiAction`, `context.answers`) resume it to done, as
 *   `ui-action answer db=pg auth=none deploy=k8s,compose` (what was chosen, in question order).
 * - `cards-mermaid`: one answer of text, a surface of the web's own catalog with three Cards (one
 *   without a link) and a Mermaid graph, then done: what the researcher answers with. No result artifact.
 * - `verify-pass`, `verify-red-once`, `verify-red`: the verification gate (ADR 0018, requires the
 *   agent's own checks, 3 attempts): the checks pass at once, fail once and then pass, or always fail.
 * - `verify-reviewed`: the gate asks a verifier agent (ADR 0018, requires the `verifier` source, 3
 *   attempts): the verifier finds something in attempt 1, the agent is sent back, and the verifier
 *   passes attempt 2. `verify-reviewed-red`: the verifier never passes it (`checks_failed`). The
 *   verifier is a subagent of its own, `sub-verify-<n>`.
 * - `verify-ci`: the gate on CI (ADR 0017, ADR 0018, `ci.required` = `ci/build`): a red `ci/build` for the
 *   first commit, the agent sent back, a green one for the second (the `ci` golden).
 * - `steps`: nested steps (ADR 0025, the `steps` golden): a sub-agent step `OpenCode`, a command `npm test`
 *   under it that fails (`1 failed`), the sub-agent's end, the agent's words and `completed`.
 * - `steps-ask`: the same sub-agent with a command that is `waiting` when the agent asks "Allow rm -rf
 *   build?" and blocks (the `steps-ask` golden); the answer ends the command and the sub-agent.
 * - `slow`: works until cancelled.
 * - `fail`: `agent_status: failed` with detail, thread failed.
 * - `talk`: a status with text, one agent message, the result.
 * - anything else (`echo`): working, result artifact (a PR link), done.
 *
 * Mock-only, not produced by the current orchestrator:
 * - `ui-bad`: an A2UI surface the renderer refuses, then the result and done.
 * - `cards-bad`: a Cards whose card has no title and a `javascript:` link: refused (rule `schema`, which
 *   comes first). `cards-bad-url`: a card whose link passes the schema (it starts with `https://`) and not
 *   the rule of ADR 0013 (user information in it): refused (rule `url`). The surface is not drawn.
 * - `mermaid-bad`: a Cards and a graph that does not parse: the cards are drawn, the graph says it
 *   could not be drawn and shows its source. `mermaid-hostile`: a graph that tries to switch its
 *   own security off (a directive, front matter, HTML in a label, a click handler): drawn as a plain image.
 * - `mermaid-kinds`: ten graphs in one surface, one of each kind an agent is likely to write (flowchart,
 *   sequence, class, state, entity relationship, Gantt, pie, mind map, timeline, git graph).
 * - `catalog-newer`: the thread was opened by a newer version of the app (its UI catalog is version
 *   99) and the agent sends a surface of that catalog with a component this build does not have:
 *   the renderer says it needs a newer version of the app. Then the result and done.
 * - `catalog-unknown`: the same surface in a thread whose catalog is this build's: the agent used a
 *   component the catalog does not have, and the surface is refused (rule `catalog`).
 * - `verify-ci-stale`: a gate on CI and the agent's checks. CI answers pending, then a stale answer of an
 *   older push, then passes (`check_result` cards replaced in place, a stale one of its own), each
 *   report with its `vymalo.ci` card.
 * - `verify-wait`: the same gate, and CI never answers: the thread stays `verifying` until cancelled.
 * - `verify-reviewed-wait`: the verifier is asked and never answers: its subagent stays open (and is
 *   told to a client that joins) until the thread is cancelled.
 * - `partial`: streams a partial `agent_message` and replaces it by its final version.
 * - `unreachable`: the delivery was dead-lettered: an `error` event, thread blocked.
 *
 * Mock only, the coder scenarios of the screenshots (plain words, so the thread titles read well):
 * - `Fix …`: the coder works through its steps, pushes, runs its checks, opens a pull request and
 *   says what it did (a markdown answer); done.
 * - `sources …`: mock only: the same steps, then what the Sources tab lists: a branch, a pull request,
 *   a CI report with a link, and a final answer with links in it (one inside code, one that is not
 *   http(s)); done.
 * - `Refactor …`: the same steps, and the coder is still running a command until cancelled.
 * - `Make …`: under the agent-checks gate: the first push fails a test, the coder is sent back,
 *   the second passes, then the pull request and the answer; done.
 * - `Upgrade …`: the coder never starts (nothing after the message) until cancelled.
 * - `Deploy …`: asks where to deploy and waits; the answer finishes it.
 * - `Also …`: a short follow-up: one step, a push and an answer.
 * - `Migrate …`: fails after two steps with the agent's reason.
 * - `Delegate …`: the coder hands the work to OpenCode (ADR 0025): a sub-agent step with fourteen steps
 *   under it (reads, a search, edits and commands), one of them a test run that fails and is run again,
 *   then the push, the checks, the pull request and the answer; done. `Investigate …`: the same, still
 *   running a command when it stops, until cancelled. `steps-many …`: a sub-agent step with 120 steps
 *   under it, played at once (a level long enough to be a scroll box), a read among them failing; done.
 * - Live text (ADR 0027): `stream …` is the golden (the words `Fib`, `onacci `, `in Rust.` as live pieces, then the
 *   log's message and done); `stream-long …` a longer Markdown reply in eight pieces, then done; `stream-hold …`
 *   (and `Write …`, for the screenshots) the same, still being written until cancelled; `stream-abandon …` a
 *   stream the model gives up halfway, then the words the agent says next under another id; done.
 */
export function scriptFor(text: string): {
  start: Step[];
  resume?: (answer: string) => Step[];
  /** A newer UI opened the thread: its catalog has this version (the mock records it as current). */
  newerCatalog?: number;
  /** The gate the thread's job runs under; absent: none, and the run ends at `completed`. */
  gate?: Gate;
} {
  const word = text.split(/\s+/).find((w) => w !== "");
  switch (word) {
    case "Fix":
      return {
        start: [
          ...coderWork,
          ...coderPushed(commitOf(12), { passed: true, summary: "42 tests passed" }),
          coderPullRequest,
          { kind: "agent_status", data: { status: "completed", detail: CODER_SUMMARY } },
          done,
        ],
      };
    case "sources":
      return {
        start: [
          ...coderWork.slice(0, 3),
          ...coderPushed(commitOf(12), { passed: true, summary: "42 tests passed" }),
          coderPullRequest,
          ciReport(12, true),
          { kind: "agent_status", data: { status: "completed", detail: SOURCES_SUMMARY } },
          done,
        ],
      };
    case "Refactor":
      return { start: [...coderWork.slice(0, 4), { pause: "cancel" }] };
    case "Make": {
      const finding = "session::expires_after_idle: expected 401, got 200";
      return {
        gate: GATE_CHECKS,
        start: [
          working,
          doing("Preparing the workspace"),
          doing("Updating the session expiry check"),
          doing("$ cargo test -p auth"),
          ...coderPushed(commitOf(1), {
            passed: false,
            summary: "1 of 42 tests failed",
            findings: [finding],
          }),
          completedAndVerifying,
          {
            kind: "check_result",
            system: true,
            data: {
              attempt: 1,
              commit: commitOf(1),
              findings: [finding],
              source: "agent_checks",
              status: "failed",
              summary: "1 of 42 tests failed",
            },
          },
          {
            kind: "rework",
            system: true,
            setState: "queued",
            data: {
              attempt: 2,
              findings: [{ findings: [finding], source: "agent_checks" }],
              maxAttempts: GATE_CHECKS.maxAttempts,
            },
          },
          working,
          doing("Fixing session::expires_after_idle"),
          doing("$ cargo test -p auth"),
          ...coderPushed(commitOf(2), { passed: true, summary: "42 tests passed" }),
          coderPullRequest,
          {
            kind: "agent_status",
            data: {
              status: "completed",
              detail:
                "Sessions now expire after 30 idle minutes. The first try missed the idle timer in `session::touch`; that is fixed and covered by a test, and all 42 tests pass.",
            },
            setState: "verifying",
          },
          {
            kind: "check_result",
            system: true,
            data: {
              attempt: 2,
              commit: commitOf(2),
              source: "agent_checks",
              status: "passed",
              summary: "42 tests passed",
            },
          },
          done,
        ],
      };
    }
    case "Delegate":
      return {
        start: [
          working,
          doing("Preparing the workspace"),
          ...openCodeSteps(OPENCODE_WORK.length, true),
          ...coderPushed(commitOf(12), { passed: true, summary: "42 tests passed" }),
          coderPullRequest,
          { kind: "agent_status", data: { status: "completed", detail: CODER_SUMMARY } },
          done,
        ],
      };
    case "Investigate":
      return {
        start: [
          working,
          doing("Preparing the workspace"),
          ...openCodeSteps(6, false),
          agentStep(
            "acp:c2:7",
            ["tool:c2"],
            "command",
            "cargo test -p auth",
            "running",
            "start",
            "execute",
          ),
          { pause: "cancel" },
        ],
      };
    case "steps-many":
      return {
        start: [
          working,
          agentStep("tool:c2", [], "subagent", "OpenCode", "running", "start", "agent"),
          ...Array.from({ length: 120 }, (_, i) =>
            openCodeChild(
              i + 1,
              i === 40 ? "read the file that is missing" : `read src/module_${i}.rs`,
              "tool",
              "read",
              i === 40 ? "no such file" : undefined,
            ),
          ),
          agentStep("tool:c2", [], "subagent", "OpenCode", "completed", "end", "agent"),
          { kind: "agent_status", data: { status: "completed", detail: "Read them all." } },
          done,
        ],
      };
    case "Upgrade":
      return { start: [{ pause: "cancel" }] };
    case "Deploy":
      return {
        start: [
          working,
          doing("Reading the deployment configuration"),
          {
            kind: "agent_status",
            data: {
              status: "input_required",
              detail: "Should I deploy to **staging** or straight to **production**?",
            },
          },
          { kind: "thread_state", data: { state: "blocked" }, setState: "blocked", system: true },
        ],
        resume: (answer) => [
          working,
          doing(`Deploying to ${answer.trim().toLowerCase() || "staging"}`),
          {
            kind: "agent_status",
            data: {
              status: "completed",
              detail: `Deployed to ${answer.trim() || "staging"}. The health checks are green.`,
            },
          },
          done,
        ],
      };
    case "Also":
      return {
        start: [
          working,
          doing("Adding an entry to CHANGELOG.md"),
          ...coderPushed(commitOf(13), { passed: true, summary: "42 tests passed" }),
          {
            kind: "agent_status",
            data: {
              status: "completed",
              detail:
                "Added a changelog entry under **Unreleased** and pushed it to the same branch, so the pull request picks it up.",
            },
          },
          done,
        ],
      };
    case "Migrate":
      return {
        start: [
          working,
          doing("Preparing the workspace"),
          doing("$ git clone https://github.com/acme/legacy.git"),
          {
            kind: "agent_status",
            data: {
              status: "failed",
              detail: "could not clone github.com/acme/legacy: permission denied (publickey)",
            },
          },
          { kind: "thread_state", data: { state: "failed" }, setState: "failed", system: true },
        ],
      };
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
          // an older push's report comes late: a card of its own, then the stale answer that decided nothing
          ciReport(0, false, { name: ci.name, summary: "the build of an older push failed" }),
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
          ciReport(1, true, { name: ci.name, summary: "build passed" }),
          {
            kind: "check_result",
            system: true,
            data: { ...ci, commit, status: "passed", summary: "build passed" },
          },
          done,
        ],
      };
    }
    // live text (ADR 0027, the `stream` golden): the words arrive piece by piece and the log says
    // them once, final, under the same id; the status that repeats them says no more
    case "stream": {
      const id = nextMessageId();
      return {
        start: [
          working,
          ...livePieces(id, ["Fib", "onacci ", "in Rust."]),
          {
            kind: "agent_message",
            data: { messageId: id, final: true, text: "Fibonacci in Rust." },
          },
          { kind: "agent_status", data: { status: "completed", detail: "Fibonacci in Rust." } },
          done,
        ],
      };
    }
    // mock only: a longer reply in Markdown, written piece by piece
    case "stream-long": {
      const id = nextMessageId();
      return {
        start: [
          working,
          ...livePieces(id, LONG_PARTS),
          { kind: "agent_message", data: { messageId: id, final: true, text: LONG_REPLY } },
          { kind: "agent_status", data: { status: "completed", detail: LONG_REPLY } },
          done,
        ],
      };
    }
    // mock only: the reply is being written and never finished (until Stop): a draft on screen
    case "stream-hold":
    case "Write": // plain words, for the screenshots: the title reads well
      return {
        start: [
          working,
          ...livePieces(nextMessageId(), LONG_PARTS.slice(0, 5), "open"),
          { pause: "cancel" },
        ],
      };
    // mock only: the model fails halfway: the stream is given up, and the log has the words the
    // agent says next, under another id
    case "stream-abandon": {
      const retry = "Sorry, let me say that again: it is forty-two.";
      const id = nextMessageId();
      const said = "The answer is forty-";
      // the model stalls for a while before it fails: the half-written reply stays on the screen a moment
      const stalled = Array.from(
        { length: 4 },
        (): Step => ({ live: { messageId: id, offset: said.length, text: "", end: "open" } }),
      );
      return {
        start: [
          working,
          ...livePieces(id, ["The answer is ", "forty-"], "open"),
          ...stalled,
          { live: { messageId: id, offset: said.length, text: "", end: "abandoned" } },
          { kind: "agent_message", data: { messageId: nextMessageId(), final: true, text: retry } },
          { kind: "agent_status", data: { status: "completed", detail: retry } },
          done,
        ],
      };
    }
    case "steps":
      return {
        start: [
          working,
          agentStep("tool:c2", [], "subagent", "OpenCode", "running", "start", "agent"),
          agentStep("acp:c2:1", ["tool:c2"], "command", "npm test", "running", "start", "execute"),
          agentStep(
            "acp:c2:1",
            ["tool:c2"],
            "command",
            "npm test",
            "failed",
            "end",
            "execute",
            "1 failed",
          ),
          agentStep("tool:c2", [], "subagent", "OpenCode", "completed", "end", "agent"),
          {
            kind: "agent_message",
            data: { messageId: nextMessageId(), final: true, text: "Done." },
          },
          { kind: "agent_status", data: { status: "completed", detail: "Done." } },
          done,
        ],
      };
    case "steps-ask":
      return {
        start: [
          working,
          agentStep("tool:c2", [], "subagent", "OpenCode", "running", "start", "agent"),
          agentStep(
            "acp:c2:1",
            ["tool:c2"],
            "command",
            "rm -rf build",
            "waiting",
            "start",
            "execute",
          ),
          {
            kind: "agent_status",
            data: { status: "input_required", detail: "Allow rm -rf build?" },
          },
          { kind: "thread_state", data: { state: "blocked" }, setState: "blocked", system: true },
        ],
        resume: () => [
          working,
          agentStep(
            "acp:c2:1",
            ["tool:c2"],
            "command",
            "rm -rf build",
            "completed",
            "end",
            "execute",
          ),
          agentStep("tool:c2", [], "subagent", "OpenCode", "completed", "end", "agent"),
          { kind: "agent_status", data: { status: "completed", detail: "Done." } },
          done,
        ],
      };
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
    case "choices":
      return {
        start: [
          working,
          choicesSurface(),
          { kind: "agent_status", data: { status: "input_required", detail: "Three questions" } },
          { kind: "thread_state", data: { state: "blocked" }, setState: "blocked", system: true },
        ],
        resume: (answer) => [working, ...finish(`answered: ${answer}`)],
      };
    case "cards-mermaid":
      return {
        start: [
          working,
          {
            kind: "agent_message",
            data: {
              messageId: nextMessageId(),
              final: true,
              text: "I compared three ways to keep a login session. The cards list them; the graph shows how a request meets one.",
            },
          },
          cardsMermaidSurface(),
          ...finishQuietly,
        ],
      };
    case "cards-bad":
    case "cards-bad-url": {
      const bad =
        word === "cards-bad"
          ? [{ subtitle: "A card needs a title", url: "javascript:alert(document.domain)" }]
          : [{ title: "Look here", url: "https://trusted.example@evil.example/login" }];
      return {
        start: [
          working,
          ownSurface([
            { id: "root", component: "Column", children: ["intro", "options"] },
            { id: "intro", component: "Text", text: "Not shown" },
            { id: "options", component: "Cards", cards: bad },
          ]),
          ...finish(`echo: ${text}`),
        ],
      };
    }
    case "mermaid-bad":
      return {
        start: [
          working,
          ownSurface([
            { id: "root", component: "Column", children: ["options", "flow"] },
            { id: "options", component: "Cards", cards: SESSION_CARDS.slice(0, 1) },
            {
              id: "flow",
              component: "Mermaid",
              title: "A graph that does not parse",
              code: "flowchart TD\n  A[Start] --> \n  B{{ not a graph",
            },
          ]),
          ...finishQuietly,
        ],
      };
    case "mermaid-kinds":
      return {
        start: [
          working,
          ownSurface([
            {
              id: "root",
              component: "Column",
              children: GRAPH_KINDS.map(([name]) => `g-${name}`),
            },
            ...GRAPH_KINDS.map(([name, code]) => ({
              id: `g-${name}`,
              component: "Mermaid",
              title: name,
              code,
            })),
          ]),
          ...finishQuietly,
        ],
      };
    case "mermaid-hostile":
      return {
        start: [
          working,
          ownSurface([
            { id: "root", component: "Column", children: ["flow"] },
            {
              id: "flow",
              component: "Mermaid",
              title: "A graph that tries things",
              code: HOSTILE_GRAPH,
            },
          ]),
          ...finishQuietly,
        ],
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
    case "catalog-newer":
      return { newerCatalog: 99, start: [working, gizmoSurface(), ...finish(`echo: ${text}`)] };
    case "catalog-unknown":
      return { start: [working, gizmoSurface(), ...finish(`echo: ${text}`)] };
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
