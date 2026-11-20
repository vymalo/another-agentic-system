/**
 * Child sessions as the mock serves them (ADR 0072, docs/api/child-sessions-v1.md, docs/api/agui.md
 * "Child sessions as subagents" and "A person starts, sends and closes a session"): the offer an agent
 * lists, the child thread each helper session is, the three events of the parent and the two of the
 * child, and the routes a person starts and closes a session with. Written from the contract and not
 * from the web's own parsers (`src/features/sessions`), so that the two are two readings of one
 * document: the web's tests and the contract tests do not agree by sharing code.
 *
 * The scripted agent opens sessions by `{ session: ... }` steps (`helpersScript`), which the server
 * plays on the parent's run; a person's start, message and close arrive through `createSessions`.
 */
import { randomUUID } from "node:crypto";
import type http from "node:http";
import type { components } from "../src/lib/api/schema";
import type { Step } from "./scripts";
import type { Tier } from "./tier";

type Thread = components["schemas"]["Thread"];
type Event = components["schemas"]["Event"];
type Actor = components["schemas"]["Actor"];
type Me = components["schemas"]["Me"];
type ThreadState = components["schemas"]["ThreadState"];

/** The URI an agent lists on its card, and the key of the capabilities document's `custom` under it. */
export const CHILD_SESSIONS_URI = "https://agents.vymalo.com/a2a/extensions/child-sessions/v1";

/** The agents whose card lists `child-sessions/v1` while the session's offer is on. */
export const SESSION_AGENTS: ReadonlySet<string> = new Set(["adam"]);

/** The most sessions one thread notes in its life (`too_many_sessions`). */
export const MAX_NOTED = 256;
/** How many starts of one thread may wait for their outcome (`too_many_starts`). */
export const MAX_WAITING_STARTS = 4;
/** The longest description, in characters (code points). */
export const MAX_DESCRIPTION = 80;
const MAX_TEXT = 100_000;

export type SessionOffer = { agents: { name: string; description: string }[]; maxOpen: number };

/** What the mock's agent offers: two helpers, and four sessions open at once. */
export const DEFAULT_OFFER: SessionOffer = {
  agents: [
    { name: "planner", description: "Breaks a request into steps." },
    { name: "researcher", description: "Looks things up on the web." },
  ],
  maxOpen: 4,
};

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * `params` of a card's entry, as the contract reads it (section 1): `agents` non-empty, each name a
 * string that is not blank and once, each description a string, and `maxOpen` a whole number from 1
 * to 256 (a whole double counts). Anything else is no offer: a card that lists the extension badly
 * does not list it, and the capabilities document has no key.
 */
export function readOffer(params: unknown): SessionOffer | null {
  if (!isRecord(params)) return null;
  const listed = params.agents;
  if (!Array.isArray(listed) || listed.length === 0) return null;
  const agents: SessionOffer["agents"] = [];
  for (const entry of listed) {
    if (!isRecord(entry)) return null;
    const { name, description } = entry;
    if (typeof name !== "string" || name.trim() === "") return null;
    if (typeof description !== "string") return null;
    if (agents.some((a) => a.name === name)) return null;
    agents.push({ name, description });
  }
  const max = params.maxOpen;
  if (typeof max !== "number" || !Number.isInteger(max) || max < 1 || max > 256) return null;
  return { agents, maxOpen: max };
}

/** Characters that read as nothing or reorder what is around them: refused in a description as a control character is. */
const isFormat = (c: number): boolean =>
  c === 0xad ||
  (c >= 0x200b && c <= 0x200f) ||
  (c >= 0x202a && c <= 0x202e) ||
  (c >= 0x2060 && c <= 0x2064) ||
  (c >= 0x2066 && c <= 0x2069) ||
  c === 0xfeff;

const isSpace = (c: number): boolean =>
  (c >= 0x09 && c <= 0x0d) ||
  c === 0x20 ||
  c === 0x85 ||
  c === 0xa0 ||
  c === 0x1680 ||
  (c >= 0x2000 && c <= 0x200a) ||
  c === 0x2028 ||
  c === 0x2029 ||
  c === 0x202f ||
  c === 0x205f ||
  c === 0x3000;

/**
 * The description of a start (`check_start_description`): trimmed, 1 to 80 characters counted in code
 * points, one line, with no control or Unicode format character. A missing one is the agent's own
 * refusal, `bad_description`, as is every other fault of it.
 */
export function checkDescription(
  raw: unknown,
): { ok: true; value: string } | { ok: false; detail: string } {
  if (typeof raw !== "string") {
    return { ok: false, detail: "`start.description` is missing: it is a string" };
  }
  const points = [...raw];
  let from = 0;
  let to = points.length;
  while (from < to && isSpace(points[from]?.codePointAt(0) ?? 0)) from += 1;
  while (to > from && isSpace(points[to - 1]?.codePointAt(0) ?? 0)) to -= 1;
  const kept = points.slice(from, to);
  if (kept.length === 0) return { ok: false, detail: "`start.description` is blank" };
  if (kept.some((c) => /\p{Cc}/u.test(c) || isFormat(c.codePointAt(0) ?? 0))) {
    return { ok: false, detail: "`start.description` is one line without control characters" };
  }
  if (kept.length > MAX_DESCRIPTION) {
    return {
      ok: false,
      detail: `\`start.description\` is ${kept.length} characters; at most ${MAX_DESCRIPTION}`,
    };
  }
  return { ok: true, value: kept.join("") };
}

// ---- what the scripted agent does ----------------------------------------------------------

export type Report = "done" | "failed" | "canceled" | "blocked";

/** One action of a scripted agent on its helper sessions; the server plays it as a step of the run. */
export type SessionAction =
  /** The parent's model starts a helper session (round 1). `key` names it for the later steps of the script. */
  | { op: "open"; key: string; agent: string; description: string; keepOpen: boolean }
  /** The parent's model sends the idle session `key` a message: its next round. */
  | { op: "round"; key: string }
  /** The helper's round ended: the child's log says it, then the parent's `child_session_reported`. */
  | { op: "report"; key: string; report: Report; text?: string }
  /** The parent's model closes the session `key`. */
  | { op: "close"; key: string }
  /** A round played on a child thread ended, how it ended (a person's rounds and a canceled one). */
  | { op: "end"; outcome: Report };

const action = (a: SessionAction): Step => ({ session: a });

const say = (text: string): Step => ({
  kind: "agent_message",
  data: { messageId: `msg-${randomUUID()}`, final: true, text },
});

const working: Step = { kind: "agent_status", data: { status: "working" }, setState: "working" };

const finish = (detail: string): Step[] => [
  { kind: "agent_status", data: { status: "completed", detail } },
  { kind: "thread_state", data: { state: "done" }, setState: "done", system: true },
];

/**
 * The scripts of the parent's first message, by its first word: `helpers` (a kept-open session and one
 * that closes when done: one round blocked, then done; one canceled; a report and a close after the
 * agent finished) and `helpers-fail` (a helper whose round fails). They read the agent's own offer
 * (planner, researcher), whatever the session's card says.
 */
export function helpersScript(word: string | undefined): { start: Step[] } | undefined {
  if (word === "helpers") {
    return {
      start: [
        working,
        action({
          op: "open",
          key: "plan",
          agent: "planner",
          description: "Plan the migration",
          keepOpen: true,
        }),
        action({
          op: "open",
          key: "sources",
          agent: "researcher",
          description: "Check the sources",
          keepOpen: false,
        }),
        action({ op: "report", key: "sources", report: "blocked", text: "Which source wins?" }),
        action({ op: "report", key: "sources", report: "done", text: "The sources agree." }),
        action({
          op: "report",
          key: "plan",
          report: "done",
          text: "Three steps: schema, code, data.",
        }),
        action({ op: "round", key: "plan" }),
        action({ op: "report", key: "plan", report: "canceled" }),
        say("Both subagents reported."),
        ...finish("Both subagents reported."),
        // after the agent finished: the helper went on and its report comes late, as a run of its own
        action({ op: "round", key: "plan" }),
        action({ op: "report", key: "plan", report: "blocked", text: "Which database?" }),
        action({ op: "report", key: "plan", report: "done", text: "Postgres." }),
        action({ op: "close", key: "plan" }),
      ],
    };
  }
  if (word === "helpers-fail") {
    return {
      start: [
        working,
        action({
          op: "open",
          key: "build",
          agent: "researcher",
          description: "Build the crate",
          keepOpen: false,
        }),
        action({ op: "report", key: "build", report: "failed", text: "cargo build failed." }),
        say("The subagent failed; I built it myself."),
        ...finish("The subagent failed; I built it myself."),
      ],
    };
  }
  return undefined;
}

/**
 * A round of a child thread a person (or the agent's own message) started with `text`. A first word of
 * `hold` makes the round wait for a test's `POST /__mock/release?thread=<child>` before it answers, so
 * that a round that works can be looked at, steered or stopped for as long as it takes.
 */
export function childRound(text: string, round: number): Step[] {
  const hold = text.trim().split(/\s+/)[0] === "hold";
  return [
    working,
    { kind: "agent_status", data: { status: "working", detail: `Round ${round}` } },
    ...(hold ? ([{ pause: "release" }] as Step[]) : []),
    say(`Round ${round}: ${text.trim() === "" ? "done" : text.trim()}`),
    { kind: "agent_status", data: { status: "completed", detail: `Round ${round} done.` } },
    { kind: "thread_state", data: { state: "done" }, setState: "done", system: true },
    action({ op: "end", outcome: "done" }),
  ];
}

/** The scripted reason a start fails with under `sessionStarts=refuse`: adam's own words for its limit. */
export const SCRIPTED_REFUSAL =
  "max_working: 4 rounds are already working in this context; wait for one to finish";

// ---- the state of the sessions --------------------------------------------------------------

/** What a test sets for the offer and the outcome of a person's starts (`POST /__mock/config`). */
export type SessionsConfig = {
  /**
   * The card's entry (`custom[URI]`), taken as given so a malformed one can be tried; `null`: the card
   * lists nothing; `"unreadable"`: the card cannot be read (a start and a close are 503, the capabilities list nothing).
   */
  offer: unknown;
  /** `open`: a start opens after a step; `refuse`: the agent refuses it, in `SCRIPTED_REFUSAL`; `hold`: it waits for a release. */
  starts: "open" | "refuse" | "hold";
  /** The most sessions one thread notes (256 in the contract); lower it to try `too_many_sessions`. */
  noted: number;
};

export const newSessionsConfig = (): SessionsConfig => ({
  offer: DEFAULT_OFFER,
  starts: "open",
  noted: MAX_NOTED,
});

/** What a session's `child_session_state` last said, to write one only when it changes. */
type Said = { state: SessionState; closing: boolean };
type SessionState = "working" | "idle" | "closed";

type Session = {
  id: string;
  parent: string;
  child: string;
  agent: string;
  description: string;
  keepOpen: boolean;
  by: "agent" | "person";
  state: SessionState;
  /** A close waits for the round to end. */
  closing: boolean;
  /** A person asked for the close and it is not answered yet; `closeCancel` says they asked to stop the round. */
  closeAsked: boolean;
  closeCancel: boolean;
  /** The newest round, and whether it is open (working or waiting for the person). */
  round: number;
  roundOpen: boolean;
  said: Said;
  closedSaid: boolean;
};

/** What the sessions need of the server: its stores, its writer of events and its scripted runs. */
export type SessionsHost = {
  threads: Map<string, Thread>;
  log: (threadId: string) => Event[];
  append: (threadId: string, kind: Event["kind"], actor: Actor, data: Event["data"]) => Event;
  addThread: (thread: Thread, under?: string) => void;
  setState: (thread: Thread, state: ThreadState) => void;
  agentActor: (thread: Thread) => Actor;
  /** Makes the thread's run entry (no step queued), so a steer, a stop or an answer finds one. */
  openRun: (
    thread: Thread,
    run: { steerable: boolean; resume?: (answer: string) => Step[] },
  ) => void;
  /** Stops what the thread's run has queued, as a stop of the person does. */
  halt: (threadId: string) => void;
  /** Plays steps on the thread, `stepMs` apart. */
  play: (thread: Thread, steps: Step[]) => void;
  stepMs: number;
  respond: {
    json: (
      res: http.ServerResponse,
      status: number,
      body: unknown,
      headers?: Record<string, string>,
    ) => void;
    problem: (
      res: http.ServerResponse,
      status: number,
      title: string,
      detail?: string,
      code?: string,
    ) => void;
    forbid: (res: http.ServerResponse, permission: string, agentId?: string) => void;
    body: (req: http.IncomingMessage) => Promise<unknown>;
  };
  accessible: (
    req: http.IncomingMessage,
    res: http.ServerResponse,
    id: string,
    act: boolean,
  ) => Thread | undefined;
  me: (req: http.IncomingMessage) => Me;
  mayInvoke: (req: http.IncomingMessage, agentId: string) => boolean;
  agentOrRefuse: (req: http.IncomingMessage, res: http.ServerResponse, agentId: string) => boolean;
  config: (req: http.IncomingMessage) => SessionsConfig;
  tier: (
    req: http.IncomingMessage,
    res: http.ServerResponse,
    agentId: string,
    named: unknown,
  ) => { tier: Tier | undefined } | undefined;
};

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const SYSTEM: Actor = { type: "system", name: "orchestrator" };

export type Sessions = ReturnType<typeof createSessions>;

export function createSessions(host: SessionsHost) {
  /** Every session, by its child thread. */
  const byChild = new Map<string, Session>();
  /** The session each key of a scripted parent names: `<parent>/<key>`. */
  const byKey = new Map<string, Session>();
  /** A start's request id, with the parent it was asked of: a repeat writes nothing. */
  const requests = new Map<string, string>();
  /** The starts of each parent that wait for their outcome, by request. */
  const waiting = new Map<string, Map<string, () => void>>();
  let counter = 0;

  const parentOf = (s: Session): Thread | undefined => host.threads.get(s.parent);
  const childThread = (s: Session): Thread | undefined => host.threads.get(s.child);
  const sessionsOf = (parent: string): Session[] =>
    [...byChild.values()].filter((s) => s.parent === parent);

  /** `childOf` of the child's thread: what a client reads to know that it is a helper's. */
  function sync(s: Session) {
    const t = childThread(s);
    if (!t) return;
    t.childOf = {
      threadId: s.parent,
      session: s.id,
      agent: s.agent,
      state: s.state,
      keepOpen: s.keepOpen,
      ...(s.closing ? { closing: true } : {}),
      ...(s.closeAsked ? { closeAsked: true } : {}),
    };
  }

  /** The child's `child_session_state`, written when the state or the close in progress changed. */
  function tell(s: Session) {
    sync(s);
    if (s.said.state === s.state && s.said.closing === s.closing) return;
    s.said = { state: s.state, closing: s.closing };
    host.append(s.child, "child_session_state", SYSTEM, {
      session: s.id,
      state: s.state,
      closing: s.closing,
      ...(s.closeAsked ? { closeAsked: true } : {}),
    });
  }

  const toParent = (s: Session, kind: Event["kind"], data: Event["data"]) => {
    const parent = parentOf(s);
    if (parent) host.append(parent.id, kind, host.agentActor(parent), data);
  };

  function reportToParent(s: Session, report: Report) {
    toParent(s, "child_session_reported", {
      session: s.id,
      thread: s.child,
      round: s.round,
      report,
    });
  }

  function closeInParent(s: Session) {
    if (s.closedSaid) return;
    s.closedSaid = true;
    toParent(s, "child_session_closed", { session: s.id, thread: s.child });
  }

  /** The round ended. `blocked` waits for the person and leaves the session as it is; any other end closes or idles it. */
  function endRound(s: Session, outcome: Report) {
    if (!s.roundOpen || s.state === "closed") return;
    if (outcome === "blocked") return reportToParent(s, "blocked");
    s.roundOpen = false;
    const closes = !s.keepOpen || s.closing;
    s.state = closes ? "closed" : "idle";
    s.closing = false;
    tell(s);
    reportToParent(s, outcome);
    if (closes) closeInParent(s);
  }

  /** A new round: the session works. */
  function beginRound(s: Session) {
    s.round += 1;
    s.roundOpen = true;
    s.state = "working";
    tell(s);
  }

  /** The child thread of a new session: the parent's agent, titled by its description, nested under the parent. */
  function makeChild(
    parent: Thread,
    o: { agent: string; description: string; keepOpen: boolean; by: "agent" | "person" },
  ): Session {
    counter += 1;
    const at = new Date().toISOString();
    const child: Thread = {
      id: randomUUID(),
      owner: parent.owner,
      title: o.description,
      target: { agentId: parent.target.agentId },
      state: "queued",
      createdAt: at,
      updatedAt: at,
      lastSeq: 0,
    };
    const s: Session = {
      id: `s${counter}`,
      parent: parent.id,
      child: child.id,
      agent: o.agent,
      description: o.description,
      keepOpen: o.keepOpen,
      by: o.by,
      state: "working",
      closing: false,
      closeAsked: false,
      closeCancel: false,
      round: 1,
      roundOpen: true,
      said: { state: "working", closing: false },
      closedSaid: false,
    };
    host.addThread(child, parent.id);
    byChild.set(child.id, s);
    sync(s);
    return s;
  }

  const childState = (child: Thread, state: ThreadState) => {
    host.append(child.id, "thread_state", SYSTEM, { state });
    host.setState(child, state);
  };

  /** What a blocked round of the parent's model is answered with when the person writes in the child. */
  const answerSteps = (answer: string): Step[] => [
    working,
    say(`Thanks: ${answer}`),
    { kind: "agent_status", data: { status: "completed" } },
    { kind: "thread_state", data: { state: "done" }, setState: "done", system: true },
    action({ op: "end", outcome: "done" }),
  ];

  // ---- the scripted agent's steps ------------------------------------------------------------

  /** One action of the scripted agent on the run of `parent`. */
  function step(t: Thread, a: SessionAction) {
    if (a.op === "end") return end(t, a.outcome);
    const parent = t;
    const keyed = `${parent.id}/${"key" in a ? a.key : ""}`;
    if (a.op === "open") {
      if (sessionsOf(parent.id).length >= MAX_NOTED) return;
      const s = makeChild(parent, { ...a, by: "agent" });
      byKey.set(keyed, s);
      const child = childThread(s);
      if (!child) return;
      host.append(child.id, "job_started", SYSTEM, { job: 1, round: { number: 1, by: "agent" } });
      host.append(child.id, "agent_status", host.agentActor(child), { status: "working" });
      host.setState(child, "working");
      host.openRun(child, { steerable: true, resume: answerSteps });
      toParent(s, "child_session_opened", {
        session: s.id,
        thread: s.child,
        agent: s.agent,
        description: s.description,
        keepOpen: s.keepOpen,
        by: "agent",
        round: 1,
      });
      return;
    }
    const s = byKey.get(keyed);
    const child = s ? childThread(s) : undefined;
    if (!s || !child) return;
    if (a.op === "round") {
      if (s.state !== "idle") return;
      beginRound(s);
      host.append(child.id, "job_started", SYSTEM, {
        job: s.round,
        round: { number: s.round, by: "agent" },
      });
      host.append(child.id, "agent_status", host.agentActor(child), { status: "working" });
      host.setState(child, "working");
      host.openRun(child, { steerable: true, resume: answerSteps });
      return;
    }
    if (a.op === "close") return closeNow(s);
    // report: the helper's round ended (a round that a person stopped or answered has ended already)
    if (!s.roundOpen) return;
    const who = host.agentActor(child);
    if (a.report === "blocked") {
      host.append(child.id, "agent_status", who, {
        status: "input_required",
        ...(a.text ? { detail: a.text } : {}),
      });
      childState(child, "blocked");
    } else if (a.report === "done") {
      if (child.state === "blocked") {
        host.append(child.id, "agent_status", who, { status: "working" });
      }
      host.append(child.id, "agent_message", who, {
        messageId: `msg-${randomUUID()}`,
        final: true,
        text: a.text ?? `${s.agent} finished round ${s.round}.`,
      });
      host.append(child.id, "agent_status", who, {
        status: "completed",
        ...(a.text ? { detail: a.text } : {}),
      });
      childState(child, "done");
    } else if (a.report === "canceled") {
      host.append(child.id, "agent_status", who, { status: "canceled", detail: "canceled" });
      childState(child, "cancelled");
    } else {
      host.append(child.id, "agent_status", who, {
        status: "failed",
        detail: a.text ?? `${s.agent} failed round ${s.round}.`,
      });
      childState(child, "failed");
    }
    endRound(s, a.report);
  }

  /** A round played on `child` ended: how it ended says what the session becomes. */
  function end(child: Thread, outcome: Report) {
    const s = byChild.get(child.id);
    if (s) endRound(s, outcome);
  }

  /** The session closes now (it is idle, or its round ended): the child says so, then the parent. */
  function closeNow(s: Session) {
    if (s.state === "closed") return;
    s.state = "closed";
    s.closing = false;
    s.roundOpen = false;
    tell(s);
    closeInParent(s);
  }

  // ---- what a person does ---------------------------------------------------------------------

  const unprocessable = (res: http.ServerResponse, code: string, detail: string) =>
    host.respond.problem(res, 422, "Unprocessable", detail, code);

  /** The card of the thread's agent: its offer under the session's switch, or none. */
  const offerOf = (req: http.IncomingMessage, agentId: string): SessionOffer | null =>
    SESSION_AGENTS.has(agentId) ? readOffer(host.config(req).offer) : null;

  /** The card cannot be read: the answer is a 503 (a start and a close read it live). */
  const unreadable = (req: http.IncomingMessage, res: http.ServerResponse): boolean => {
    if (host.config(req).offer !== "unreadable") return false;
    res.setHeader("Retry-After", "5");
    host.respond.problem(res, 503, "Service Unavailable", "the agent's card cannot be read");
    return true;
  };

  /**
   * `POST /api/threads/{id}/sessions` (`startChildSession`): refused in the server's order (`start_child_session`), then
   * 202 with the request's id. The request is written at once (`child_session_start_asked`); the
   * outcome comes after a step: the child thread, the parent's `child_session_opened` (by the
   * person) and `child_session_start_ended`, or the agent's refusal.
   */
  async function start(req: http.IncomingMessage, res: http.ServerResponse, id: string) {
    const { problem, json } = host.respond;
    let body: unknown;
    try {
      body = await host.respond.body(req);
    } catch {
      return problem(res, 400, "Bad Request", "the body is not JSON");
    }
    const bad = badStart(body);
    if (bad) return problem(res, 400, "Bad Request", bad);
    const parent = host.accessible(req, res, id, true);
    if (!parent) return;
    const b = body as Record<string, unknown>;
    // the server's order (`start_child_session`): the thread, then each refusal before anything is written
    if (parent.childOf !== undefined) {
      return unprocessable(res, "child_thread", "a helper session cannot start another one");
    }
    if (!host.mayInvoke(req, parent.target.agentId)) {
      return host.respond.forbid(res, "agent.invoke", parent.target.agentId);
    }
    const text = b.text as string;
    if (text.trim() === "" || [...text].length > MAX_TEXT) {
      return problem(
        res,
        400,
        "Bad Request",
        text.trim() === ""
          ? "text must not be empty"
          : `text must be at most ${MAX_TEXT} characters`,
      );
    }
    const description = checkDescription(b.description);
    if (!description.ok) return unprocessable(res, "bad_description", description.detail);
    // the agent the registry lists, then its live card
    if (!host.agentOrRefuse(req, res, parent.target.agentId)) return;
    if (unreadable(req, res)) return;
    const offer = offerOf(req, parent.target.agentId);
    if (!offer) {
      return unprocessable(res, "no_child_sessions", "the agent does not offer helper sessions");
    }
    if (!offer.agents.some((a) => a.name === b.agent)) {
      return unprocessable(
        res,
        "unknown_helper",
        `no helper \`${String(b.agent)}\`; \`start.agent\` is one of: ${offer.agents.map((a) => a.name).join(", ")}`,
      );
    }
    const tier = host.tier(req, res, parent.target.agentId, b.tier);
    if (!tier) return;
    const request = b.id as string;
    const hasContext = host.log(parent.id).some((e) => e.actor.type === "agent");
    if (!hasContext) {
      return problem(res, 409, "Conflict", "send the agent a message first", "no_context");
    }
    if (sessionsOf(parent.id).length >= host.config(req).noted) {
      return problem(
        res,
        409,
        "Conflict",
        "this thread has started as many helper sessions as one thread may note",
        "too_many_sessions",
      );
    }
    // the same request again changes nothing (the apply's key), so the limit on waiting starts is not asked of it
    if (requests.has(request)) {
      return json(res, 202, { request }, { Location: `/api/threads/${parent.id}` });
    }
    if ((waiting.get(parent.id)?.size ?? 0) >= MAX_WAITING_STARTS) {
      return problem(
        res,
        409,
        "Conflict",
        "wait for the helpers you started to open",
        "too_many_starts",
      );
    }
    requests.set(request, parent.id);
    const keepOpen = b.keepOpen === true;
    const who = host.me(req).user;
    host.append(
      parent.id,
      "child_session_start_asked",
      { type: "user", name: who },
      {
        request,
        agent: b.agent as string,
        description: description.value,
        keepOpen,
        ...(tier.tier ? { tier: tier.tier } : {}),
      },
    );
    const cfg = host.config(req);
    const outcome = () => {
      waiting.get(parent.id)?.delete(request);
      const current = host.threads.get(parent.id);
      if (!current) return;
      const open = sessionsOf(parent.id).filter((s) => s.state !== "closed").length;
      const refusal =
        cfg.starts === "refuse"
          ? SCRIPTED_REFUSAL
          : open >= offer.maxOpen
            ? `max_open: ${offer.maxOpen} sessions are open in this context; close one first`
            : undefined;
      if (refusal !== undefined) {
        host.append(parent.id, "child_session_start_ended", SYSTEM, {
          request,
          outcome: "failed",
          reason: refusal,
        });
        return;
      }
      opened(current, {
        request,
        agent: b.agent as string,
        description: description.value,
        keepOpen,
        text,
        who,
        ...(tier.tier ? { tier: tier.tier } : {}),
      });
    };
    const queue = waiting.get(parent.id) ?? new Map<string, () => void>();
    waiting.set(parent.id, queue);
    // `hold`: the outcome waits for a test (`release`); otherwise it comes after a step
    queue.set(request, outcome);
    if (cfg.starts !== "hold") setTimeout(() => queue.get(request)?.(), host.stepMs).unref();
    return json(res, 202, { request }, { Location: `/api/threads/${parent.id}` });
  }

  /** What the body of a start must be: every member of `ChildSessionStart` and no other (a 400 otherwise). */
  function badStart(body: unknown): string | undefined {
    if (!isRecord(body)) return "the body must be an object";
    const members = ["id", "agent", "description", "text", "keepOpen", "tier"];
    const extra = Object.keys(body).find((k) => !members.includes(k));
    if (extra !== undefined) return `unknown member \`${extra}\``;
    if (typeof body.id !== "string" || !UUID.test(body.id)) return "`id` must be a UUID";
    if (typeof body.agent !== "string" || body.agent === "" || body.agent.length > 128) {
      return "`agent` is a string of 1 to 128 characters";
    }
    if (body.description !== undefined && typeof body.description !== "string") {
      return "`description` must be a string";
    }
    if (typeof body.text !== "string") return "`text` must be a string";
    if (body.keepOpen !== undefined && typeof body.keepOpen !== "boolean") {
      return "`keepOpen` must be a boolean";
    }
    if (
      body.tier !== undefined &&
      body.tier !== null &&
      !["plan", "edits", "auto", "bypass"].includes(body.tier as string)
    ) {
      return "`tier` must be one of plan, edits, auto, bypass";
    }
    return undefined;
  }

  /** The agent opened the session a person started: the child thread with the prompt, the parent's events. */
  function opened(
    parent: Thread,
    o: {
      request: string;
      agent: string;
      description: string;
      keepOpen: boolean;
      text: string;
      who: string;
      tier?: Tier;
    },
  ) {
    const s = makeChild(parent, { ...o, by: "person" });
    const child = childThread(s);
    if (!child) return;
    host.append(
      child.id,
      "user_message",
      { type: "user", name: o.who },
      {
        text: o.text,
        messageId: `msg-${randomUUID()}`,
        runId: randomUUID(),
        ...(o.tier ? { tier: o.tier } : {}),
      },
    );
    toParent(s, "child_session_opened", {
      session: s.id,
      thread: s.child,
      agent: s.agent,
      description: s.description,
      keepOpen: s.keepOpen,
      by: "person",
      round: 1,
    });
    host.append(parent.id, "child_session_start_ended", SYSTEM, {
      request: o.request,
      outcome: "opened",
      session: s.id,
      thread: s.child,
    });
    host.openRun(child, { steerable: true });
    host.setState(child, "queued");
    host.play(child, childRound(o.text, 1));
  }

  /** A test lets the starts of `parent` that wait (`sessionStarts=hold`) come to their outcome. */
  function release(parent: string): number {
    const queue = waiting.get(parent);
    if (!queue) return 0;
    const all = [...queue.values()];
    for (const outcome of all) outcome();
    return all.length;
  }

  /**
   * `POST /api/threads/{id}/session/close` (`closeChildSession`): an idle session closes at once, a
   * working one when its round ends, and with `cancel` its round is stopped first.
   */
  async function close(req: http.IncomingMessage, res: http.ServerResponse, id: string) {
    const { problem, json } = host.respond;
    let body: unknown;
    try {
      body = await host.respond.body(req);
    } catch {
      return problem(res, 400, "Bad Request", "the body is not JSON");
    }
    let cancel = false;
    if (body !== null && body !== undefined) {
      if (
        !isRecord(body) ||
        Object.keys(body).some((k) => k !== "cancel") ||
        (body.cancel !== undefined && typeof body.cancel !== "boolean")
      ) {
        return problem(res, 400, "Bad Request", "the body is `{cancel?: boolean}`");
      }
      cancel = body.cancel === true;
    }
    const child = host.accessible(req, res, id, true);
    if (!child) return;
    const s = byChild.get(child.id);
    if (!s || child.childOf === undefined) {
      return unprocessable(res, "not_child_thread", "only a helper session's thread can be closed");
    }
    if (s.state === "closed") return json(res, 202, { state: "closed" });
    // the agent is the registry's to list (404), and its card is read live (503 while it cannot be)
    if (!host.agentOrRefuse(req, res, child.target.agentId)) return;
    if (unreadable(req, res)) return;
    if (!offerOf(req, child.target.agentId)) {
      return unprocessable(res, "no_child_sessions", "the agent does not offer helper sessions");
    }
    // a close asked with the same or a stronger `cancel` writes nothing
    if (s.closeAsked && (s.closeCancel || !cancel)) {
      return json(res, 202, { state: "closing" });
    }
    s.closeAsked = true;
    s.closeCancel = cancel;
    host.append(
      child.id,
      "child_session_close_asked",
      { type: "user", name: host.me(req).user },
      {
        session: s.id,
        cancel,
      },
    );
    if (s.state === "idle") {
      closeNow(s);
      return json(res, 202, { state: "closed" });
    }
    s.closing = true;
    tell(s);
    if (cancel && s.roundOpen) stop(child);
    return json(res, 202, { state: "closing" });
  }

  /** Stops the round of the child that works, as a person's Stop does: canceled, and how that leaves the session. */
  function stop(child: Thread) {
    host.halt(child.id);
    host.play(child, [
      { kind: "agent_status", data: { status: "canceled", detail: "canceled" } },
      { kind: "thread_state", data: { state: "cancelled" }, setState: "cancelled", system: true },
      action({ op: "end", outcome: "canceled" }),
    ]);
  }

  /**
   * What a run on a child thread meets before anything is written: a closed session takes no
   * message (409 `session_closed`), a message that mentions agents is refused (422 `child_mentions`:
   * a helper cannot ask other agents) and so is a Stop & send on a session that closes when its round
   * stops (422 `session_closes_on_stop`). `undefined` when the run may go on.
   */
  function guardRun(
    child: Thread,
    run: { mentions: boolean; message: boolean; how: "steer" | "interrupt" | undefined },
  ): { status: 409 | 422; title: string; detail: string; code: string } | undefined {
    const s = byChild.get(child.id);
    if (!s) return undefined;
    if (s.state === "closed") {
      return {
        status: 409,
        title: "Conflict",
        detail: "the session is closed and takes no more messages",
        code: "session_closed",
      };
    }
    if (run.message && run.mentions) {
      return {
        status: 422,
        title: "Unprocessable",
        detail: "a helper cannot ask other agents: the message mentions none",
        code: "child_mentions",
      };
    }
    if (run.message && run.how === "interrupt" && !s.keepOpen) {
      return {
        status: 422,
        title: "Unprocessable",
        detail: "the session closes when its round is stopped: it is not kept open",
        code: "session_closes_on_stop",
      };
    }
    return undefined;
  }

  /**
   * The next round of a child thread, started by a person's message (or by a Stop & send): the
   * session works again, and the steps are the round's. An interrupted round is reported canceled first.
   */
  function nextRound(child: Thread, text: string, interrupted: boolean): Step[] | undefined {
    const s = byChild.get(child.id);
    if (!s) return undefined;
    if (interrupted) endRound(s, "canceled");
    beginRound(s);
    return childRound(text, s.round);
  }

  /** The step that ends a round played on `child` when it is stopped, for the generic `cancel`. */
  const stopSteps = (child: Thread): Step[] | undefined =>
    byChild.has(child.id) ? [action({ op: "end", outcome: "canceled" })] : undefined;

  /** A round of the child that is waiting for a person (`blocked`) takes the answer as its own. */
  const isChild = (id: string): boolean => byChild.has(id);

  /** The helper of a child thread, for the projection of its own card. */
  const childMeta = (id: string): { agent: string; keepOpen: boolean } | undefined => {
    const s = byChild.get(id);
    return s ? { agent: s.agent, keepOpen: s.keepOpen } : undefined;
  };

  /** The child threads of a thread, all of them. */
  const childrenOf = (parent: string): string[] => sessionsOf(parent).map((s) => s.child);

  /** A thread went (a delete): what it kept of sessions goes with it. */
  function forget(id: string) {
    byChild.delete(id);
    for (const [key, s] of byKey) if (s.child === id || s.parent === id) byKey.delete(key);
    waiting.delete(id);
    for (const [request, parent] of requests) if (parent === id) requests.delete(request);
  }

  function reset() {
    byChild.clear();
    byKey.clear();
    requests.clear();
    waiting.clear();
    counter = 0;
  }

  return {
    step,
    start,
    close,
    release,
    guardRun,
    nextRound,
    stopSteps,
    stop,
    isChild,
    childMeta,
    childrenOf,
    offerOf,
    forget,
    reset,
  };
}
