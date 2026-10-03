import type { components } from "../src/lib/api/schema";
import { CHART, EXPORT, HOSTILE_SVG, type MockFile, NOTES, RESULTS } from "./files";

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
  | {
      /**
       * The run waits here: `cancel` until the person stops it, `release` until a test says go on
       * (`POST /__mock/release?thread=<id>`, mock/server.ts), so that a state a test needs to look at
       * stays as long as it takes, whatever the speed of the machine.
       */
      pause: "cancel" | "release";
    }
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

/** An artifact that is a file the artifact store keeps (ADR 0032): the reference, never the bytes. */
const keptFile = (file: MockFile, name: string): Step => ({
  kind: "artifact",
  data: {
    name,
    mimeType: file.mimeType,
    file: { sha256: file.sha256, size: file.bytes.length, filename: file.filename },
  },
});

/** What the model says a described thread is about (the `description` golden's words). */
export const DESCRIPTION = "The person wants a plan for a test.";

/** A description of three sentences: more than a line wide, to be expanded. */
export const LONG_DESCRIPTION =
  "The person wants a plan for adding a test to the session expiry check, so that a session idle for thirty minutes is refused with a 401. The agent read the repository and proposed one test beside the existing ones. Nothing was changed yet.";

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

export const nextMessageId = (() => {
  let n = 0;
  return () => `m-${++n}`;
})();

/**
 * The pieces of a reply as a sender relays them while the model writes it: `parts` joined is the
 * reply. `last` ends it (the log's message closes the live one), `open` leaves it unfinished, and
 * `abandoned` gives the stream up after its words (the model failed). `from` is how much of the reply
 * was said before `parts` (the pieces that go on after a hold).
 */
function livePieces(
  messageId: string,
  parts: readonly string[],
  end: "last" | "open" | "abandoned" = "last",
  from = 0,
): Step[] {
  let offset = from;
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
  io?: StepIo,
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
    ...(io?.input ? { input: io.input } : {}),
    ...(io?.output ? { output: io.output } : {}),
    ...(io?.ioDropped ? { ioDropped: true } : {}),
  },
});

/**
 * What a step carries besides its words (ADR 0030): the input (an object) on its start or, for a
 * step reported once as it ended, with that one report; the output on its end. The mock says them
 * the way the orchestrator logs them: redacted (`"[redacted]"`) and cut (`truncated`, `bytes`).
 */
type StepIo = {
  input?: Record<string, unknown>;
  output?: { text: string; truncated?: true; bytes?: number; error?: true };
  /** The job's budget had no room for the step's input and output. */
  ioDropped?: true;
};

/**
 * What the agent says while it works, as the log has it since ADR 0031: a final message the
 * adapter marked `working` (said on a `working` status, before a tool call). With no purpose it is
 * the same words with no mark, as a plain A2A agent or a log from before the mark says them.
 */
const said = (
  text: string,
  purpose: "working" | "answer" | undefined,
  messageId = nextMessageId(),
): Step => ({
  kind: "agent_message",
  data: { messageId, final: true, text, ...(purpose ? { purpose } : {}) },
});

/** One finished tool step, reported once as it ended, with what it was called with and returned. */
const tool = (n: number, label: string, icon: string, io?: StepIo, failed?: string): Step => ({
  ...agentStep(
    `tool:n${n}`,
    [],
    "tool",
    label,
    failed ? "failed" : "completed",
    "end",
    icon,
    failed,
    io,
  ),
  quick: true,
});

/** The surface the coder draws mid-turn (`show`): what the drawing holds, as two cards. */
const drawingSurface = (): Step =>
  ownSurface([
    { id: "root", component: "Column", children: ["intro", "shapes"] },
    { id: "intro", component: "Text", text: "The export, as the shapes it holds" },
    {
      id: "shapes",
      component: "Cards",
      title: "What the drawing holds",
      layout: "list",
      cards: [
        { title: "Background", subtitle: "A rectangle, 800 by 450", tags: ["fill #0b1020"] },
        {
          title: "Sun",
          subtitle: "A circle of radius 70",
          body: "Centred at 640, 120.",
          tags: ["#f5b942"],
        },
      ],
    },
  ]);

/**
 * The owner's coder chat of 2026-10-02, in its shape and in other words: the coder says a sentence
 * before each tool call, a test run fails once, a surface is drawn mid-turn and the turn ends with
 * one answer. In the log since ADR 0031 each sentence is marked `working` and the last words
 * `answer`; with `marked` false none is (an older log), and the screen reads the turn by its rule.
 */
const coderNotes = (marked: boolean): Step[] => {
  const w = marked ? "working" : undefined;
  const answer = [
    "The drawing is exported, and its shape is in the cards above.",
    "",
    "**What I built:** a scratch project `canvas-draw`, a tiny Node.js drawing layer with no dependencies.",
    "",
    "- `src/draw.js` builds the picture from rectangles and circles",
    "- `src/export.js` writes it as an SVG",
    "- seven tests cover both, and they all pass",
    "",
    "Say the word and I will turn it into a repository.",
  ].join("\n");
  return [
    working,
    said("I'll build something small in a scratch project first, then show you the export.", w),
    tool(1, "start_scratch", "tool", { input: { name: "canvas-draw" }, output: { text: "ready" } }),
    said(
      "Your screen draws text, cards and diagrams, not images. So Node draws it and I show its shapes.",
      w,
    ),
    tool(2, "write_file", "edit", {
      input: { path: "src/draw.js" },
      output: { text: "wrote 41 lines" },
    }),
    tool(3, "write_file", "edit", {
      input: { path: "src/export.js" },
      output: { text: "wrote 23 lines" },
    }),
    said("Now the tests for both.", w),
    tool(4, "write_file", "edit", {
      input: { path: "test/draw.test.js" },
      output: { text: "wrote 58 lines" },
    }),
    tool(
      5,
      "run_checks",
      "test",
      { input: { check: "npm test" }, output: { text: "2 of 7 failed", error: true } },
      "checks failed: npm test",
    ),
    said(
      "Node 24 wants an explicit glob for the test directory. I'll fix the script, not the tests.",
      w,
    ),
    tool(6, "apply_patch", "edit", {
      input: { path: "package.json" },
      output: { text: "patched" },
    }),
    tool(7, "run_checks", "test", { input: { check: "npm test" }, output: { text: "7 passed" } }),
    said("All 7 tests pass. Now I'll export the drawing.", w),
    tool(8, "run_command", "execute", {
      input: { command: "npm run render" },
      output: { text: "wrote out/drawing.svg" },
    }),
    said("Here is the drawing. Your screen has no images, so these are its shapes.", w),
    agentStep("tool:n9", [], "tool", "show", "running", "start", "tool"),
    drawingSurface(),
    agentStep("tool:n9", [], "tool", "show", "completed", "end", "tool"),
    tool(10, "read_file", "read", {
      input: { path: "out/drawing.svg" },
      output: { text: "<svg …>" },
    }),
    said(answer, marked ? "answer" : undefined),
    { kind: "agent_status", data: { status: "completed", detail: answer } },
    done,
  ];
};

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
  io?: StepIo,
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
    io,
  ),
  quick: true,
});

/** What OpenCode does in the `Delegate` scenarios, in order: a failing test run among them. */
const OPENCODE_WORK: [string, "tool" | "command", string, string?, StepIo?][] = [
  [
    "read src/auth/login.rs",
    "tool",
    "read",
    undefined,
    {
      input: { path: "src/auth/login.rs" },
      output: {
        text: 'pub fn login(req: &Request) -> Redirect {\n    let next = req.query("next");\n    Redirect::to(next.unwrap_or("/"))\n}\n',
      },
    },
  ],
  ["read src/auth/session.rs", "tool", "read"],
  [
    "search the repository for next=",
    "tool",
    "search",
    undefined,
    {
      input: { pattern: "next=", glob: "**/*.rs" },
      output: {
        text: 'src/auth/login.rs:2: let next = req.query("next");\ntests/login.rs:9: get("/login?next=/billing")',
      },
    },
  ],
  ["read tests/login.rs", "tool", "read"],
  ["cargo build -p auth", "command", "execute"],
  [
    "cargo test -p auth login::",
    "command",
    "execute",
    "1 failed, 41 passed",
    {
      input: { command: "cargo test -p auth login::", cwd: "/work/demo" },
      output: {
        text: "running 42 tests\n...\nfailures:\n    login::keeps_the_next_url\n\ntest result: FAILED. 41 passed; 1 failed",
        error: true,
      },
    },
  ],
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
  ...OPENCODE_WORK.slice(0, count).map(([label, kind, icon, failed, io], i) =>
    openCodeChild(i + 1, label, kind, icon, failed, io),
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
 * - `steps-io`: mock only: what a tool step can carry (ADR 0030), one leaf step of each shape: a search with its
 *   input (start) and output (end), a cut output (`truncated`, `bytes`), an input too big to keep
 *   (`{_cut, bytes}`), a failed command whose output is its error, a step the job's budget had no room for
 *   (`ioDropped`), and a step with none. The `steps` golden's `npm test` carries input and output too.
 * - `relay`: mock only: calls of attached MCP servers as the orchestrator reports them (thread-tools-v1, "The
 *   step of a call", `icon: "mcp-server:<id>"`): a web search of a server with an icon, one of a server the
 *   list has no icon for (the generic icon), one that failed, one of a server the deployment no longer lists;
 *   the answer and done. The orchestrator does not relay yet (slice 8's second half): the mock plays the story.
 * - `steps-ask`: the same sub-agent with a command that is `waiting` when the agent asks "Allow rm -rf
 *   build?" and blocks (the `steps-ask` golden); the answer ends the command and the sub-agent.
 * - `slow`: works until cancelled. `gate`: works until a test releases the run (`POST /__mock/release`), then the result and done.
 * - `fail`: `agent_status: failed` with detail, thread failed.
 * - `talk`: a status with text, one agent message, the result.
 * - `describe`: the same as `talk`, then, after the thread is done, the description the orchestrator's model
 *   writes for it (`thread_described`, `source: model`, ADR 0035; the `description` golden). `describe-long`: the
 *   same with a description of three sentences, longer than a line (`Plan …` is that one in plain words, for the
 *   screenshots and the e2e of descriptions). A description a person wrote is never
 *   replaced by one of these.
 * - `file`: one artifact that is a file the artifact store keeps (ADR 0032: a PNG, `chart.png`, the reference in
 *   the event's `file`), then done (the `file` golden). The mock serves it as `getArtifact` does.
 * - anything else (`echo`): working, result artifact (a PR link), done.
 *
 * Mock only, files (ADR 0032, plan 10 S12): `file-image` is the `file` scenario's chart, then an `Image` of the
 * catalog (v4) that places it by its hash in an answer; `file-image-foreign` names a hash the thread does not hold
 * (the surface is refused); `file-svg` keeps an SVG written to run a script and load a stylesheet and an image (the
 * browser's e2e draws it as an `<img>`); `files` keeps three files, an image, a text file and an archive; `file-lost` is an
 * artifact the store did not keep (no `file`) and the error that says why (the file is too large to keep).
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
 *   (and `Write …`, for the screenshots) the same, still being written until cancelled; `stream-gate …` the same
 *   five pieces, then nothing until the test releases it (`POST /__mock/release?thread=<id>`), then the other three,
 *   the log's message and done; `stream-abandon …` a stream the model gives up halfway, after the test releases it,
 *   then the words the agent says next under another id; done.
 * - Working text and the answer (ADR 0031, plan 10 S6): `stream-words …` is the `working` golden (words before a tool call
 *   as live pieces, marked `working`, then the reply marked `answer`). `coder-notes …` (and `Draw …`, for the screenshots) is
 *   the owner's coder chat of 2026-10-02 in its shape: six sentences said before tool calls (marked `working`), ten tool steps
 *   with a failed test run among them, a surface drawn mid-turn (`show`) and one answer (marked `answer`), done.
 *   `coder-notes-hold …` (and `Sketch …`, for the screenshots) is its first five sentences and the steps between, still
 *   working until cancelled. `coder-notes-legacy …` is the same turn with no mark on any word (an older log, a plain A2A agent): the screen reads it by
 *   its rule. `coder-notes-running …` says one unmarked sentence and waits for the test to release it (a draft of the answer
 *   while the turn runs), then a step, then the words that end the turn; done.
 */
export function scriptFor(text: string): {
  start: Step[];
  resume?: (answer: string) => Step[];
  /** A newer UI opened the thread: its catalog has this version (the mock records it as current). */
  newerCatalog?: number;
  /** The gate the thread's job runs under; absent: none, and the run ends at `completed`. */
  gate?: Gate;
  /** The agent lists `steer/v1`: a message sent while it works is read by the running task (ADR 0036). */
  steerable?: true;
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
    // working text and the answer (ADR 0031, the `working` golden): the words before a tool call
    // are stated on a `working` status and the reply on `completed`; the log marks them `working`
    // and `answer`, and the live message of the words ends as working text
    case "stream-words": {
      const words = nextMessageId();
      const reply = nextMessageId();
      return {
        start: [
          working,
          ...livePieces(words, ["Let me run ", "the tests first."]),
          {
            kind: "agent_message",
            data: {
              messageId: words,
              final: true,
              purpose: "working",
              text: "Let me run the tests first.",
            },
          },
          agentStep("tool:c1", [], "command", "npm test", "completed", "end", "execute"),
          ...livePieces(reply, ["Streaming a reply, ", "word by word, ", "as it is written."]),
          {
            kind: "agent_message",
            data: {
              messageId: reply,
              final: true,
              purpose: "answer",
              text: "Streaming a reply, word by word, as it is written.",
            },
          },
          {
            kind: "agent_status",
            data: {
              status: "completed",
              detail: "Streaming a reply, word by word, as it is written.",
            },
          },
          done,
        ],
      };
    }
    // mock only, the owner's coder chat (plan 10 S6): notes while it works, a surface, one answer.
    // `Draw` is the same in plain words, for the screenshots
    case "coder-notes":
    case "Draw":
      return { start: coderNotes(true) };
    // mock only: the same, still working when it stops (until cancelled): the last thing said is a
    // note, so the turn's line shows it as its ticker. `Sketch` is the same in plain words
    case "coder-notes-hold":
    case "Sketch":
      return { start: [...coderNotes(true).slice(0, 13), { pause: "cancel" }] };
    // the same turn as an older log or a plain A2A agent says it: no word is marked, so the screen
    // reads it by its rule (the last words of a turn that is over are its answer)
    case "coder-notes-legacy":
      return { start: coderNotes(false) };
    // mock only: the legacy rule while the turn runs. The first words are said and nothing else
    // happens until the test releases the run: they show as a draft of the answer. Then a step
    // starts after them, they fold into the steps, and the words that end the turn are its answer
    case "coder-notes-running":
      return {
        start: [
          working,
          said("Let me look at the failing test first.", undefined),
          { pause: "release" },
          tool(1, "read_file", "read", {
            input: { path: "test/login.test.js" },
            output: { text: "…" },
          }),
          said("Fixed: the redirect no longer loops.", undefined),
          {
            kind: "agent_status",
            data: { status: "completed", detail: "Fixed: the redirect no longer loops." },
          },
          done,
        ],
      };
    // the agent announces its answer with the `turn_output` tool (ADR 0031 amendment, the
    // `turn-output` golden): a sentence before a tool call (working), a step, the announced answer
    // (`purpose: answer, via: turn_output`; the real id is `out-<jti>-1`), then the closing line, which the core
    // writes as working text ahead of the status that keeps it
    case "turn-output": {
      const words = nextMessageId();
      const announced = nextMessageId();
      const closing = nextMessageId();
      return {
        start: [
          working,
          {
            kind: "agent_message",
            data: {
              messageId: words,
              final: true,
              purpose: "working",
              text: "Let me run the tests first.",
            },
          },
          agentStep("tool:c1", [], "command", "npm test", "completed", "end", "execute"),
          {
            kind: "agent_message",
            data: {
              messageId: announced,
              final: true,
              purpose: "answer",
              via: "turn_output",
              text: "The tests pass: 12 of 12.",
            },
          },
          {
            kind: "agent_message",
            data: {
              messageId: closing,
              final: true,
              purpose: "working",
              text: "Done; the result is above.",
            },
          },
          {
            kind: "agent_status",
            data: { status: "completed", detail: "Done; the result is above." },
          },
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
    // mock only: the reply is being written, and goes on when the test says so: a draft stays on the
    // screen as long as the test needs to look at it (a reload, a screenshot), then the rest comes
    case "stream-gate": {
      const id = nextMessageId();
      const first = LONG_PARTS.slice(0, 5);
      return {
        start: [
          working,
          ...livePieces(id, first, "open"),
          { pause: "release" },
          ...livePieces(id, LONG_PARTS.slice(5), "last", first.join("").length),
          { kind: "agent_message", data: { messageId: id, final: true, text: LONG_REPLY } },
          { kind: "agent_status", data: { status: "completed", detail: LONG_REPLY } },
          done,
        ],
      };
    }
    // mock only: the model fails halfway, when the test releases it: the stream is given up, and the
    // log has the words the agent says next, under another id
    case "stream-abandon": {
      const retry = "Sorry, let me say that again: it is forty-two.";
      const id = nextMessageId();
      const said = "The answer is forty-";
      return {
        start: [
          working,
          ...livePieces(id, ["The answer is ", "forty-"], "open"),
          // the model stalls: the half-written reply stays on the screen until the test lets it fail
          { pause: "release" },
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
          agentStep(
            "acp:c2:1",
            ["tool:c2"],
            "command",
            "npm test",
            "running",
            "start",
            "execute",
            undefined,
            // what the agent sent, as the orchestrator logs it: the token is redacted (ADR 0030)
            {
              input: { command: "npm test", cwd: "web", env: { CI: "1", NPM_TOKEN: "[redacted]" } },
            },
          ),
          agentStep(
            "acp:c2:1",
            ["tool:c2"],
            "command",
            "npm test",
            "failed",
            "end",
            "execute",
            "1 failed",
            {
              output: {
                text: "FAIL src/sum.test.ts\n  adds two numbers\n1 failed, 12 passed",
                error: true,
              },
            },
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
    case "steps-io":
      // what a tool step can carry (ADR 0030), one leaf step of each shape, for the web's step block:
      // a search with its arguments and result (start and end), a result the core cut, an input too
      // big to keep, a failure whose output is the error, a step the job's budget had no room for,
      // and a step that has nothing (an agent that sends no input or output)
      return {
        start: [
          working,
          agentStep(
            "tool:s1",
            [],
            "tool",
            "search__web_search",
            "running",
            "start",
            "search",
            undefined,
            { input: { query: "Stephane Segning", limit: 3, api_key: "[redacted]" } },
          ),
          agentStep(
            "tool:s1",
            [],
            "tool",
            "search__web_search",
            "completed",
            "end",
            "search",
            undefined,
            {
              output: {
                text: "1. Stephane Segning - vymalo\n   https://example.org/mock-search/1\n2. Another result\n   https://example.org/mock-search/2",
              },
            },
          ),
          agentStep(
            "tool:s2",
            [],
            "tool",
            "fetch__fetch_page",
            "running",
            "start",
            "fetch",
            undefined,
            { input: { url: "https://example.org/long-page" } },
          ),
          agentStep(
            "tool:s2",
            [],
            "tool",
            "fetch__fetch_page",
            "completed",
            "end",
            "fetch",
            undefined,
            {
              output: {
                text: `${"The first part of a long page. ".repeat(190)}\n\u2026 41808 bytes not kept \u2026\n${"The last part of it. ".repeat(95)}`,
                truncated: true,
                bytes: 50000,
              },
            },
          ),
          agentStep(
            "tool:s3",
            [],
            "tool",
            "files__write_many",
            "completed",
            "end",
            "edit",
            undefined,
            { input: { _cut: true, bytes: 18432 } as Record<string, unknown> },
          ),
          agentStep(
            "tool:s4",
            [],
            "command",
            "npm run build",
            "running",
            "start",
            "execute",
            undefined,
            { input: { command: "npm run build", cwd: "web" } },
          ),
          agentStep(
            "tool:s4",
            [],
            "command",
            "npm run build",
            "failed",
            "end",
            "execute",
            "exit 1",
            {
              output: {
                text: "> web@0.1.0 build\n> next build\n\nFailed to compile.\n\n./src/app/page.tsx:12:7\nType error: Property 'title' does not exist on type 'Props'.",
                error: true,
              },
            },
          ),
          agentStep(
            "tool:s5",
            [],
            "tool",
            "search__web_search",
            "completed",
            "end",
            "search",
            undefined,
            { ioDropped: true },
          ),
          agentStep("tool:s6", [], "tool", "no_details", "completed", "end", "tool"),
          {
            kind: "agent_message",
            data: { messageId: nextMessageId(), final: true, text: "Done." },
          },
          { kind: "agent_status", data: { status: "completed", detail: "Done." } },
          done,
        ],
      };
    case "relay":
      // calls of attached MCP servers, as the orchestrator reports them (docs/api/thread-tools-v1.md,
      // "The step of a call"): `icon: "mcp-server:<id>"`, the label "<server name> · <tool>", the input on
      // the start and the output on the end. One call of a server with an icon, one of a server the
      // list has no icon for (the generic one is drawn), one that failed, and one of a server the
      // deployment no longer lists.
      return {
        start: [
          working,
          agentStep(
            "tool:r1",
            [],
            "tool",
            "Web search \u00b7 search",
            "running",
            "start",
            "mcp-server:websearch",
            undefined,
            { input: { query: "Stephane Segning", limit: 3 } },
          ),
          agentStep(
            "tool:r1",
            [],
            "tool",
            "Web search \u00b7 search",
            "completed",
            "end",
            "mcp-server:websearch",
            undefined,
            {
              output: {
                text: "1. Stephane Segning - vymalo\n   https://example.org/mock-search/1",
              },
            },
          ),
          agentStep(
            "tool:r2",
            [],
            "tool",
            "GitHub \u00b7 get_issue",
            "completed",
            "end",
            "mcp-server:github",
            undefined,
            { input: { repo: "vymalo/demo", number: 7 }, output: { text: "Issue 7: Fix login" } },
          ),
          agentStep(
            "tool:r3",
            [],
            "tool",
            "Team docs \u00b7 find",
            "failed",
            "end",
            "mcp-server:docs",
            "the server did not answer",
            { input: { query: "deploy" }, output: { text: "unreachable", error: true } },
          ),
          agentStep(
            "tool:r4",
            [],
            "tool",
            "retired \u00b7 lookup",
            "completed",
            "end",
            "mcp-server:retired",
          ),
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
    // a file the agent handed over (ADR 0032, the `file` golden): the artifact holds the reference
    // to the file the artifact store keeps, and the agent says nothing else
    case "file":
      return {
        start: [
          working,
          {
            kind: "artifact",
            data: {
              name: "chart",
              mimeType: CHART.mimeType,
              file: { sha256: CHART.sha256, size: CHART.bytes.length, filename: CHART.filename },
            },
          },
          ...finishQuietly,
        ],
      };
    // the same file, placed in the answer by an `Image` of the catalog (v4): by its hash, never a URL
    case "file-image":
      return {
        start: [
          working,
          {
            kind: "agent_message",
            data: {
              messageId: nextMessageId(),
              final: true,
              text: "I drew the chart of the results. It is a file of this chat, so you can place it, or download it.",
            },
          },
          keptFile(RESULTS, "chart"),
          ownSurface([
            { id: "root", component: "Column", children: ["intro", "chart"] },
            { id: "intro", component: "Text", text: "The results at a glance" },
            {
              id: "chart",
              component: "Image",
              artifact: RESULTS.sha256,
              alt: "A chart of the results of the run",
              caption: "Figure 1: the results of the run",
            },
          ]),
          ...finishQuietly,
        ],
      };
    // the same Image, with a hash this thread does not hold: the whole surface is refused
    case "file-image-foreign":
      return {
        start: [
          working,
          keptFile(CHART, "chart"),
          ownSurface([
            { id: "root", component: "Column", children: ["chart"] },
            {
              id: "chart",
              component: "Image",
              artifact: "0".repeat(64),
              alt: "A file of another thread",
            },
          ]),
          ...finishQuietly,
        ],
      };
    // three kept files of the three kinds: an image, a text file and an archive (preview `image`, `text`, none)
    case "files":
      return {
        start: [
          working,
          {
            kind: "agent_message",
            data: {
              messageId: nextMessageId(),
              final: true,
              text: "I made three files: a chart, my notes and an export of everything.",
            },
          },
          keptFile(RESULTS, "chart"),
          keptFile(NOTES, "notes"),
          keptFile(EXPORT, "export"),
          ...finishQuietly,
        ],
      };
    // an SVG that tries to run and to load things, kept as a file: drawn as an `<img>`, it does neither
    case "file-svg":
      return { start: [working, keptFile(HOSTILE_SVG, "diagram"), ...finishQuietly] };
    // a file the store did not keep: an artifact without `file`, and the error that says why
    case "file-lost":
      return {
        start: [
          working,
          { kind: "artifact", data: { name: "dump", mimeType: "application/octet-stream" } },
          {
            kind: "error",
            system: true,
            data: { message: "the file is too large to keep", retryable: false },
          },
          ...finishQuietly,
        ],
      };
    case "slow":
      return { start: [working, { pause: "cancel" }] };
    // holds until a test releases the run (`POST /__mock/release`), then finishes: the agent that is
    // working while a message is sent to it (ADR 0036)
    case "gate":
      return { start: [working, { pause: "release" }, ...finish(`echo: ${text}`)] };
    // as `gate`, for an agent that lists `steer/v1`: a message sent while it works is read by the
    // running task at its next step (the server says "steered: <text>"), and the job is still one job
    case "steerable":
      return {
        start: [working, { pause: "release" }, ...finish(`echo: ${text}`)],
        steerable: true,
      };
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
    case "describe":
    case "describe-long":
    case "Plan": // plain words, for the screenshots and the e2e of descriptions
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
          {
            kind: "thread_described",
            system: true,
            data: {
              description: word === "describe" ? DESCRIPTION : LONG_DESCRIPTION,
              source: "model",
            },
          },
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
