/**
 * A small stateful mock of the orchestrator for `pnpm dev:mock` and the e2e tests: the REST
 * resource API of docs/api/chat-api.yaml (agents, threads, rename, cancel) and the AG-UI operations
 * (`POST /agui/agents/{agentId}`, `GET /agui/threads/{id}/connect`, capabilities).
 *
 * It is typed from the generated contract types and checked against the contract by
 * server.contract.test.ts; the AG-UI frames are the orchestrator's (mock/projection.ts, checked
 * against the goldens of docs/api/examples/agui by golden.test.ts). Authentication is not enforced
 * (oauth2-proxy's job in production), but who a session is, is: `GET /api/me`, and the 403s and 404s
 * its permissions give (ADR 0033), switched per session by `POST /__mock/config?me=` (fixtures.ts
 * `PROFILES`). The first word of the first message picks a scripted agent
 * behaviour: see scripts.ts and web/README.md.
 */
import { createHash, randomBytes, randomUUID } from "node:crypto";
import http from "node:http";
import { pathToFileURL } from "node:url";
import { catalogDigest } from "../src/features/chat/lib/a2ui/catalog/digest";
import type { components } from "../src/lib/api/schema";
import { FILES, fileHeaders } from "./files";
import {
  AGENTS,
  DEV_USER,
  isProfileName,
  MENTIONS_AGENTS,
  MENTIONS_URI,
  PROFILES,
  type ProfileName,
  REGISTRY_UNREACHABLE,
  STEER_AGENTS,
  STEER_URI,
  THREAD_TOOLS_AGENTS,
  THREAD_TOOLS_URI,
  TOOL_SERVERS,
} from "./fixtures";
import { type BrowserAuthOptions, createIssuer } from "./issuer";
import { LiveOverlay, type LivePiece } from "./live";
import { checkAgainstText, checkAgents, type MentionRef, readMentions } from "./mentions";
import {
  type Audience,
  type CatalogRef,
  catalogRefOf,
  type Frame,
  type GateInfo,
  Projector,
  surfacesOf,
} from "./projection";
import { cancelSteps, nextMessageId, type Step, scriptFor } from "./scripts";

type Thread = components["schemas"]["Thread"];
type Event = components["schemas"]["Event"];
type Actor = components["schemas"]["Actor"];
type Agent = components["schemas"]["Agent"];
type Me = components["schemas"]["Me"];
type ToolServer = components["schemas"]["ToolServer"];
type ThreadState = components["schemas"]["ThreadState"];
type Sharing = NonNullable<Me["sharing"]>;
type ShareLevel = "private" | "internal" | "public";
type ShareLink = components["schemas"]["ThreadShareLink"];
type SharedThread = components["schemas"]["SharedThread"];

/** One thread's share (ADR 0040): what it is stored as, the link's token, and when it was last set. */
type Share = { visibility: "internal" | "public"; token: string; sharedAt: string };
/** What a reader of a link is: signed in (`internal`) or anybody (`public`). */
type Reader = "internal" | "public";
const SHARE_TOKEN = /^[A-Za-z0-9_-]{43}$/;
const RANK: Record<Sharing | ShareLevel, number> = {
  disabled: 0,
  private: 0,
  internal: 1,
  public: 2,
};

/** A run is open while the thread is queued, working or being verified. */
const isActiveState = (s: ThreadState): boolean =>
  s === "queued" || s === "working" || s === "verifying";

const RELEASE_CHANNELS_URI = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";
const TOOLS_PROP = "vymalo.tools";
const MENTIONS_PROP = "vymalo.mentions";
/** `ToolServer.id`, and the pattern of a thread's `tools` and of a request's `servers`. */
const SERVER_ID = /^[a-z0-9][a-z0-9-]{0,30}$/;
/** At most this many distinct servers on a thread. */
const MAX_SERVERS = 16;
const UI_CATALOG_PROP = "vymalo.uiCatalog";
const OWN_CATALOG_ID = "https://agents.vymalo.com/a2ui/catalogs/chat";
const MAX_CATALOG_BYTES = 64 * 1024;
const MAX_CATALOG_COMPONENTS = 64;
class BadCatalog extends Error {
  constructor(
    readonly status: 400 | 413,
    message: string,
  ) {
    super(message);
  }
}

/**
 * What a Choices answer says, for the scripted agent's echo: ` db=pg auth=none deploy=k8s,compose`
 * (the chosen values, and `other:<text>`, in question order); nothing for any other action.
 */
function chosen(context: Record<string, unknown>): string {
  const answers = context.answers;
  if (!Array.isArray(answers)) return "";
  return answers
    .filter(isRecord)
    .map((a) => {
      const values = Array.isArray(a.values) ? a.values.map(String) : [];
      const other = typeof a.other === "string" ? [`other:${a.other}`] : [];
      return ` ${String(a.id)}=${[...values, ...other].join(",")}`;
    })
    .join("");
}

/** A catalog as the screen sent it: what a `ui_catalog` event holds. */
type CatalogSent = CatalogRef & { catalog: Record<string, unknown> };

/**
 * `forwardedProps["vymalo.uiCatalog"]` (docs/api/agui.md "Inbound"), checked as the orchestrator
 * checks it, less the JSON Schema compilation: the shape, the id, the version, the size and the
 * keys, and the digest recomputed. Returns what the thread records (the event's data), or
 * undefined when the run carries none.
 */
async function readCatalog(props: unknown): Promise<CatalogSent | undefined> {
  if (!isRecord(props) || !(UI_CATALOG_PROP in props)) return undefined;
  const v = props[UI_CATALOG_PROP];
  if (!isRecord(v)) throw new BadCatalog(400, "vymalo.uiCatalog must be an object");
  const { catalogId, version, digest, catalog } = v;
  if (
    typeof catalogId !== "string" ||
    !/^https:\/\/\S+$/.test(catalogId) ||
    catalogId.length > 256
  ) {
    throw new BadCatalog(400, "vymalo.uiCatalog.catalogId must be an absolute https URL");
  }
  if (!Number.isInteger(version) || (version as number) < 1 || (version as number) > 1_000_000) {
    throw new BadCatalog(400, "vymalo.uiCatalog.version must be 1 to 1000000");
  }
  if (typeof digest !== "string" || !/^sha256:[0-9a-f]{64}$/.test(digest)) {
    throw new BadCatalog(400, "vymalo.uiCatalog.digest must be sha256: and 64 hex digits");
  }
  if (!isRecord(catalog) || catalog.catalogId !== catalogId || !isRecord(catalog.components)) {
    throw new BadCatalog(400, "vymalo.uiCatalog.catalog must be a catalog of the same catalogId");
  }
  if (Buffer.byteLength(JSON.stringify(catalog)) > MAX_CATALOG_BYTES) {
    throw new BadCatalog(413, "vymalo.uiCatalog.catalog is over 64 KiB");
  }
  const names = Object.keys(catalog.components);
  if (
    names.length > MAX_CATALOG_COMPONENTS ||
    !names.every((n) => /^[A-Z][A-Za-z0-9]{0,63}$/.test(n))
  ) {
    throw new BadCatalog(400, "vymalo.uiCatalog.catalog has too many components, or a bad name");
  }
  let actual: string;
  try {
    actual = await catalogDigest(catalog);
  } catch {
    throw new BadCatalog(400, "vymalo.uiCatalog.catalog cannot be canonicalised");
  }
  if (actual !== digest)
    throw new BadCatalog(400, "vymalo.uiCatalog.digest does not match the catalog");
  return { catalogId, version: version as number, digest, catalog };
}
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

type Run = {
  timer: NodeJS.Timeout | undefined;
  pending: Step[];
  resume: ((answer: string) => Step[]) | undefined;
  /** Set while the run waits at a `{ pause: "release" }` step: goes on with the steps after it. */
  release?: () => void;
  /**
   * Messages sent while the job runs (`vymalo.send: "steer"`, ADR 0036) to an agent that does not
   * list `steer/v1` (`steerable` unset) reach it after its turn: the first of them starts the
   * thread's next job when this one is done.
   */
  held?: string[];
  /** The agent lists `steer/v1`: a steer is read by the running task, in the same job. */
  steerable?: boolean;
};

/** One open response that gets the frames of a thread as its log grows. */
type Viewer = {
  res: http.ServerResponse;
  projector: Projector;
  /** The live text of this connection (docs/api/agui.md "Live text"), beside the projection. */
  overlay: LiveOverlay;
  audience: Audience;
  /** When the response ends: `first-close` at the first run close (a run response), `after-replay` when the replay is over and no run is open (`?mode=run`), else never. */
  end: "first-close" | "after-replay" | "never";
  keepalive: NodeJS.Timeout;
  /** Test hook: frames left before the connection is cut. */
  cutAfter: number | undefined;
  /** A reader of a share link (ADR 0040): the reader projection, and whether the link still serves them. */
  reader?: { kind: Reader; valid: () => boolean };
};

export type MockOptions = {
  stepMs?: number;
  keepaliveMs?: number;
  /** How often the text so far of a reply being written is said again from its start (default 1000). */
  refreshMs?: number;
  /**
   * Browser mode (ADR 0054): the mock is also the issuer, `GET /api/public/auth` names it, and every
   * route of the API wants a DPoP-bound token with a proof it checks (mock/issuer.ts). Without it the
   * mock is the edge deployment: the cookie, `/oauth2/*`, and a 404 for `/api/public/auth`.
   */
  browserAuth?: BrowserAuthOptions;
};

/** A run response ends with `RUN_FINISHED` or `RUN_ERROR`. */
const isTerminalEvent = (event: { type?: unknown }) =>
  event.type === "RUN_FINISHED" || event.type === "RUN_ERROR";

/**
 * One event as a reader of a share link may have it (ADR 0040, the reader projection of the
 * orchestrator): a person's messages are by "the owner", never an e-mail address; the events a reader
 * may not see are not dropped but replaced by an inert one with the same `seq`, so the numbering holds
 * (the projection says nothing for it); and a public reader has no step input, output or detail, and
 * no file (the deployment's `sharing.public.stepIo` and `files` are off, their default): an artifact
 * that is only a file is not there at all, one that says more (a link, a text) is without the file.
 */
export function readerEvent(e: Event, reader: Reader): Event {
  if (["thread_forked", "ui_catalog", "thread_shared", "thread_unshared"].includes(e.kind)) {
    return {
      ...e,
      kind: "thread_unshared",
      actor: { type: "system", name: "orchestrator" },
      data: {},
    };
  }
  const actor = e.actor.type === "user" ? { ...e.actor, name: "the owner" } : e.actor;
  let data = e.data;
  if (reader === "public" && e.kind === "agent_step") {
    const { input: _i, output: _o, detail: _d, ...rest } = data;
    data = rest;
  }
  if (reader === "public" && e.kind === "artifact" && data.file !== undefined) {
    // what it says besides the file (a link, a text), else it is not there at all
    if (data.uri === undefined && data.text === undefined) {
      return {
        ...e,
        kind: "thread_unshared",
        actor: { type: "system", name: "orchestrator" },
        data: {},
      };
    }
    const { file: _f, ...rest } = data;
    data = rest;
  }
  return { ...e, actor, data };
}

export function createMockServer(options: MockOptions = {}): http.Server {
  const stepMs = options.stepMs ?? 400;
  const keepaliveMs = options.keepaliveMs ?? 15_000;
  const refreshMs = options.refreshMs ?? 1000;
  const issuer = options.browserAuth ? createIssuer(options.browserAuth) : undefined;
  /** Who the verified token of a request is (browser mode): the profile the issuer approved. */
  const tokenProfiles = new WeakMap<http.IncomingMessage, ProfileName>();

  const threads = new Map<string, Thread>();
  /** The share of each thread that has one (ADR 0040); a thread that is not here is private. */
  const shares = new Map<string, Share>();
  const events = new Map<string, Event[]>();
  const viewers = new Map<string, Set<Viewer>>();
  const runs = new Map<string, Run>();
  /** The gate each verified thread's job runs under (from its script); absent: none. */
  const gates = new Map<string, GateInfo>();
  /**
   * How each fork was made (ADR 0029): the parent, the cut (the last event copied) and, for an
   * edit, the `seq` of the message that replaces the parent's. The edits of one message are the
   * versions `listBranches` says.
   */
  type Link = { parent: string; cut: number; kind: "fork" | "edit"; replacing?: number };
  const links = new Map<string, Link>();
  /**
   * The reply each thread's agent is writing right now (live text): what the relay says again from
   * the start every `refreshMs`, so a viewer that connects mid-stream, or lost a piece, has the text
   * so far within a second (ADR 0027). Gone when the log says the reply, when the stream is given
   * up, and with the run.
   */
  const writing = new Map<
    string,
    { messageId: string; agent: string; text: string; kind?: "reasoning" }
  >();
  let cutNextConnectAfter: number | undefined;
  /**
   * The platform's agent registry (ADR 0022), as a test sets it: the agents it lists (after the
   * configured ones, `source: "registry"`) and whether it can be read. A registry that cannot be
   * read lists none of its agents, and a run on one of them is a 503, never a 404.
   *
   * The state is kept per session, so that e2e tests running in parallel against one mock do not
   * see each other's registry: a browser says which in the cookie `mock-registry`, a test hook in
   * `?session=`. No session is the `default` one. The `ui` configuration (`GET /api/config`) is kept
   * the same way, so that a test can switch descriptions off for its own browser. So is who the
   * session is (`me`, a profile of fixtures.ts): the default is a person with every permission over
   * their own threads, which is what a session was before roles.
   */
  type Registry = {
    agents: Agent[];
    down: boolean;
    showDescriptions: boolean;
    me: ProfileName;
    /** The MCP servers the deployment offers (`GET /api/tool-servers`), in its order. */
    toolServers: ToolServer[];
    /** The deployment's cap on sharing (`sharing.mode`, ADR 0040); what `/api/me` says is the cap for a role that may share. */
    sharing: Sharing;
    /** The session has an identity: false, every route but the public ones is a 401, as for a person who is not signed in. */
    signedIn: boolean;
    /**
     * The session's token has run out and can be refreshed: every route but the public ones is a 401, as the edge's
     * `forward_auth` answers, until the browser itself asks `GET /oauth2/userinfo`, which refreshes it (oauth2-proxy does, and
     * is the only request whose renewed cookie reaches the browser).
     */
    stale: boolean;
    /** What the stand-in for the edge did, for a test to read (`GET /__mock/edge`). */
    edge: { refreshes: number; signIns: number };
  };
  const registries = new Map<string, Registry>();
  const registryOf = (session: string): Registry => {
    let registry = registries.get(session);
    if (!registry) {
      registry = {
        agents: [],
        down: false,
        showDescriptions: true,
        me: "user",
        toolServers: [...TOOL_SERVERS],
        sharing: "internal",
        signedIn: true,
        stale: false,
        edge: { refreshes: 0, signIns: 0 },
      };
      registries.set(session, registry);
    }
    return registry;
  };
  const sessionOf = (req: http.IncomingMessage): string =>
    /(?:^|;\s*)mock-registry=([^;]+)/.exec(req.headers.cookie ?? "")?.[1] ?? "default";
  const listedAgents = (req: http.IncomingMessage): Agent[] => {
    const registry = registryOf(sessionOf(req));
    const read = meOf(req).agents.read;
    return [...AGENTS, ...(registry.down ? [] : registry.agents)].filter((a) => covers(read, a.id));
  };
  const findAgent = (req: http.IncomingMessage, id: string): Agent | undefined =>
    listedAgents(req).find((a) => a.id === id || a.aliases?.includes(id));
  /** Who the session is (`GET /api/me`). */
  const meOf = (req: http.IncomingMessage): Me => {
    const state = registryOf(sessionOf(req));
    const me = PROFILES[tokenProfiles.get(req) ?? state.me];
    // the cap for a role that holds `thread.share`, else `disabled` (ADR 0040)
    return { ...me, sharing: holds(me, "thread.share") ? state.sharing : "disabled" };
  };
  const capOf = (req: http.IncomingMessage): Sharing => registryOf(sessionOf(req)).sharing;
  const scopeOf = (me: Me, permission: string): string | undefined =>
    me.permissions.find((p) => p.permission === permission)?.scope;
  const holds = (me: Me, permission: string): boolean =>
    me.permissions.some((p) => p.permission === permission);
  const covers = (ids: string[], agentId: string): boolean =>
    ids.includes("*") || ids.includes(agentId);
  /** The 403 of a permission no role of the person holds, whatever is asked for (`code: forbidden`). */
  const forbid = (res: http.ServerResponse, permission: string, agentId?: string) =>
    problem(
      res,
      403,
      "Forbidden",
      agentId === undefined
        ? `your roles do not grant ${permission}`
        : `your roles do not grant ${permission} for the agent ${agentId}`,
      "forbidden",
    );
  /**
   * The thread the caller reads (`act` false) or changes (`act` true), or the answer that refuses
   * it, as the orchestrator gives it: a permission no role holds is a 403; a thread that is not the
   * caller's is a 404, whatever their roles (it does not leak that it exists; ADR 0039).
   */
  const accessible = (
    req: http.IncomingMessage,
    res: http.ServerResponse,
    id: string,
    act: boolean,
  ): Thread | undefined => {
    const me = meOf(req);
    if (act && !holds(me, "thread.write")) return void forbid(res, "thread.write");
    const read = scopeOf(me, "thread.read");
    if (read === undefined) return void forbid(res, "thread.read");
    const thread = threads.get(id);
    if (!thread || thread.owner !== me.user) return void problem(res, 404, "Thread not found");
    return thread;
  };
  /** Every agent any session's registry lists: a thread keeps its agent when a session's registry goes. */
  const everyAgent = (): Agent[] => [
    ...AGENTS,
    ...[...registries.values()].flatMap((r) => r.agents),
  ];

  const reset = () => {
    writing.clear();
    for (const r of runs.values()) clearTimeout(r.timer);
    for (const set of viewers.values()) for (const v of set) closeViewer(v, true);
    threads.clear();
    shares.clear();
    events.clear();
    viewers.clear();
    runs.clear();
    gates.clear();
    links.clear();
    cutNextConnectAfter = undefined;
    registries.clear();
  };

  // ---- helpers -------------------------------------------------------------------------

  const sendJson = (
    res: http.ServerResponse,
    status: number,
    body: unknown,
    type = "application/json",
  ) => {
    res.writeHead(status, { "Content-Type": type, "Cache-Control": "no-store" });
    res.end(JSON.stringify(body));
  };
  const problem = (
    res: http.ServerResponse,
    status: number,
    title: string,
    detail?: string,
    code?: string,
  ) =>
    sendJson(
      res,
      status,
      { title, status, ...(detail ? { detail } : {}), ...(code ? { code } : {}) },
      "application/problem+json",
    );

  const readJson = (req: http.IncomingMessage): Promise<unknown> =>
    new Promise((resolve, reject) => {
      const chunks: Buffer[] = [];
      req.on("data", (c: Buffer) => chunks.push(c));
      req.on("end", () => {
        try {
          resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "null"));
        } catch (e) {
          reject(e);
        }
      });
      req.on("error", reject);
    });

  const touch = (t: Thread) => {
    t.updatedAt = new Date().toISOString();
  };

  const infoOf = (t: Thread) => ({
    threadId: t.id,
    title: t.title,
    target: {
      agentId: t.target.agentId,
      ...(t.target.release ? { release: t.target.release } : {}),
    },
    ...(gates.has(t.id) ? { gate: gates.get(t.id) } : {}),
  });

  /** What a share is served as under a cap (ADR 0040): the narrower of what it is stored as and the cap. */
  const effectiveOf = (share: Share, cap: Sharing): ShareLevel =>
    RANK[share.visibility] <= RANK[cap] ? share.visibility : cap === "disabled" ? "private" : cap;
  /** The link, `/s/<token>`; none while the cap is `disabled` (the deployment holds no keys then). */
  const linkOf = (share: Share, cap: Sharing): string | undefined =>
    cap === "disabled" ? undefined : `/s/${share.token}`;
  const linkView = (share: Share, cap: Sharing): ShareLink => {
    const url = linkOf(share, cap);
    return {
      visibility: share.visibility,
      effective: effectiveOf(share, cap),
      ...(url ? { url } : {}),
      sharedAt: share.sharedAt,
    };
  };

  /**
   * The thread as the resource API shows it: under a gate, with where its job stands (`job`). With
   * `sharing` (the caller's cap), a thread that is shared says so (`share`); the link (`url`) only
   * in the answers about one thread, never in the list or the export.
   */
  const viewOf = (t: Thread, sharing?: { cap: Sharing; link: boolean }): Thread => {
    const share = sharing ? shares.get(t.id) : undefined;
    const shared: Thread =
      share && sharing
        ? {
            ...t,
            share: {
              visibility: share.visibility,
              effective: effectiveOf(share, sharing.cap),
              ...(sharing.link && linkOf(share, sharing.cap)
                ? { url: linkOf(share, sharing.cap) as string }
                : {}),
            },
          }
        : t;
    if (!gates.has(t.id)) return shared;
    const projector = new Projector(infoOf(t));
    for (const e of events.get(t.id) ?? []) projector.apply(e);
    const job = projector.job();
    return job ? { ...shared, job } : shared;
  };

  // ---- streams -------------------------------------------------------------------------

  const frameText = ({ id, event }: Frame) =>
    `${id === undefined ? "" : `id: ${id}\n`}data: ${JSON.stringify(event)}\n\n`;

  function closeViewer(v: Viewer, abrupt = false) {
    clearInterval(v.keepalive);
    for (const set of viewers.values()) set.delete(v);
    if (abrupt) v.res.destroy();
    else v.res.end();
  }

  function write(v: Viewer, frames: Frame[]) {
    for (const f of frames) {
      if (v.cutAfter !== undefined) {
        if (v.cutAfter <= 0) return closeViewer(v, true);
        v.cutAfter -= 1;
      }
      v.res.write(frameText(f));
    }
    if (v.cutAfter !== undefined && v.cutAfter <= 0) closeViewer(v, true);
  }

  /**
   * Starts an SSE response for `thread`: the frames of the events after `fromSeq` (the ones up to
   * it are folded and not written, like the orchestrator does), then the live ones.
   */
  function startViewer(
    res: http.ServerResponse,
    thread: Thread,
    opts: {
      fromSeq: number;
      audience?: Audience;
      end: Viewer["end"];
      /**
       * Start at the `RUN_STARTED` of this run: the response to a message that ended the run that
       * was open (ADR 0036) is the run the message opened, and nothing of the one it ended.
       */
      fromRun?: string;
      /** A reader of a share link: the log goes through the reader projection (`readerEvent`). */
      reader?: { kind: Reader; valid: () => boolean };
    },
  ) {
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    });
    const projector = new Projector(infoOf(thread));
    const log = events.get(thread.id) ?? [];
    const seen = (e: Event): Event => (opts.reader ? readerEvent(e, opts.reader.kind) : e);
    for (const e of log) if (e.seq <= opts.fromSeq) projector.apply(seen(e));
    const viewer: Viewer = {
      res,
      projector,
      overlay: new LiveOverlay(),
      ...(opts.reader ? { reader: opts.reader } : {}),
      audience: opts.audience ?? {},
      end: opts.end,
      keepalive: setInterval(() => res.write(": keepalive\n\n"), keepaliveMs),
      cutAfter: cutNextConnectAfter,
    };
    viewer.keepalive.unref();
    cutNextConnectAfter = undefined;
    res.on("close", () => {
      clearInterval(viewer.keepalive);
      viewers.get(thread.id)?.delete(viewer);
    });
    if (opts.fromRun === undefined) write(viewer, projector.preamble());
    let writing = opts.fromRun === undefined;
    for (const e of log) {
      if (e.seq <= opts.fromSeq) continue;
      const wasOpen = projector.runOpen;
      let frames = viewer.overlay.logged(projector, projector.apply(seen(e), viewer.audience));
      if (!writing) {
        const at = frames.findIndex(
          (f) => f.event.type === "RUN_STARTED" && f.event.runId === opts.fromRun,
        );
        if (at < 0) continue;
        frames = frames.slice(at);
        writing = true;
      }
      if (viewer.end === "first-close") {
        // a run response ends at its terminal event, whatever the event goes on to open
        const last = frames.findIndex((f) => isTerminalEvent(f.event));
        if (last >= 0) {
          write(viewer, frames.slice(0, last + 1));
          return closeViewer(viewer);
        }
      }
      write(viewer, frames);
      if (viewer.end === "first-close" && wasOpen && !projector.runOpen) return closeViewer(viewer);
    }
    if (viewer.end === "after-replay" && !projector.runOpen) return closeViewer(viewer);
    const set = viewers.get(thread.id) ?? new Set();
    viewers.set(thread.id, set);
    set.add(viewer);
  }

  function append(threadId: string, kind: Event["kind"], actor: Actor, data: Event["data"]): Event {
    const log = events.get(threadId) ?? [];
    events.set(threadId, log);
    const event: Event = {
      seq: log.length + 1,
      threadId,
      at: new Date().toISOString(),
      kind,
      actor,
      data,
    };
    log.push(event);
    const t = threads.get(threadId);
    if (t) {
      t.lastSeq = event.seq;
      touch(t);
      // the thread's description is what its last `thread_described` says; empty clears it
      if (kind === "thread_described") {
        const description = typeof data.description === "string" ? data.description : "";
        if (description === "") delete t.description;
        else t.description = description;
      }
    }
    for (const v of [...(viewers.get(threadId) ?? [])]) {
      const wasOpen = v.projector.runOpen;
      const seen = v.reader ? readerEvent(event, v.reader.kind) : event;
      const frames = v.overlay.logged(v.projector, v.projector.apply(seen, v.audience));
      if (v.end === "first-close") {
        // a run response ends at its terminal event, even when the event opens the next run (a
        // message sent while the agent works, ADR 0036)
        const last = frames.findIndex((f) => isTerminalEvent(f.event));
        if (last >= 0) {
          write(v, frames.slice(0, last + 1));
          closeViewer(v);
          continue;
        }
      }
      write(v, frames);
      if (v.end !== "never" && wasOpen && !v.projector.runOpen) closeViewer(v);
    }
    return event;
  }

  /**
   * A piece of the reply the agent is still writing (live text): every open response of the thread
   * hears it through its own overlay, as the orchestrator's processes do. It is not in the log, and
   * a viewer that connects later does not hear it.
   */
  function relay(threadId: string, piece: LivePiece) {
    for (const v of [...(viewers.get(threadId) ?? [])])
      write(v, v.overlay.live(v.projector, piece));
  }

  /** The sender's side: a piece goes to the viewers and is kept, to be said again from the start. */
  function send(threadId: string, piece: LivePiece) {
    const known = writing.get(threadId);
    if (piece.end === "abandoned") writing.delete(threadId);
    else if (piece.offset === 0 || known?.messageId !== piece.messageId) {
      writing.set(threadId, {
        messageId: piece.messageId,
        agent: piece.agent,
        text: piece.text,
        ...(piece.kind ? { kind: piece.kind } : {}),
      });
    } else if (known) known.text = known.text.slice(0, piece.offset) + piece.text;
    relay(threadId, piece);
  }

  const refresher = setInterval(() => {
    for (const [threadId, w] of writing) {
      const closed = ["done", "failed", "cancelled"].includes(threads.get(threadId)?.state ?? "");
      if (closed || (events.get(threadId) ?? []).some((e) => e.data.messageId === w.messageId)) {
        writing.delete(threadId);
        continue;
      }
      relay(threadId, {
        messageId: w.messageId,
        agent: w.agent,
        offset: 0,
        text: w.text,
        end: "open",
        ...(w.kind ? { kind: w.kind } : {}),
      });
    }
  }, refreshMs);
  refresher.unref();

  /** The seq of the last event of the thread: where the response to what is appended next starts. */
  const lastSeq = (threadId: string) => events.get(threadId)?.length ?? 0;

  /**
   * Records the catalog an input carried as a `ui_catalog` event of the user, first in the
   * commit of the input, unless the log holds its digest already (once per digest; no frame, so
   * the ledger of the projection moves and nothing is said).
   */
  function recordCatalog(threadId: string, sent: CatalogSent | undefined) {
    if (!sent) return;
    const known = (events.get(threadId) ?? []).some(
      (e) => e.kind === "ui_catalog" && catalogRefOf(e.data)?.digest === sent.digest,
    );
    if (known) return;
    append(threadId, "ui_catalog", { type: "user", name: DEV_USER }, sent as Event["data"]);
  }

  /** A person wrote (or cleared) the thread's description: the last `thread_described` is theirs. */
  const describedByPerson = (threadId: string): boolean =>
    (events.get(threadId) ?? []).findLast((e) => e.kind === "thread_described")?.data.source ===
    "user";

  const setState = (t: Thread, state: ThreadState) => {
    t.state = state;
    touch(t);
  };

  function agentActor(t: Thread): Actor {
    const agent = everyAgent().find((a) => a.id === t.target.agentId);
    const releases = agent?.releases;
    let revision: string | undefined;
    if (releases) {
      const release = t.target.release ?? releases.defaultChannel;
      revision = releases.channels[release] ?? release;
    }
    return { type: "agent", name: t.target.agentId, ...(revision ? { revision } : {}) };
  }

  // ---- MCP servers attached to a thread (ADR 0024) -------------------------------------

  /** What the deployment offers for this agent, in its order (`agents` absent: every agent). */
  const offeredFor = (req: http.IncomingMessage, agentId: string): ToolServer[] =>
    registryOf(sessionOf(req)).toolServers.filter(
      (t) => t.agents === undefined || t.agents.includes(agentId),
    );

  /**
   * Why `ids` cannot be attached to a thread of `agentId`, or undefined: a server the deployment
   * does not list, or does not offer for the agent, or more than 16 distinct ones. A server the
   * thread has already is not checked again (a deployment that stopped listing it leaves the thread
   * its attachment). The words name the server's id, never a URL.
   */
  function refusedServers(
    req: http.IncomingMessage,
    ids: readonly string[],
    agentId: string,
    attached: ReadonlySet<string>,
  ): string | undefined {
    if (new Set(ids).size > MAX_SERVERS) return `more than ${MAX_SERVERS} servers`;
    const listed = registryOf(sessionOf(req)).toolServers;
    const offered = offeredFor(req, agentId);
    for (const id of ids) {
      if (attached.has(id)) continue;
      if (!listed.some((t) => t.id === id)) return `the server ${id} is not offered`;
      if (!offered.some((t) => t.id === id)) {
        return `the server ${id} is not offered for the agent ${agentId}`;
      }
    }
    return undefined;
  }

  /**
   * Makes `wanted` the whole set of servers attached to the thread: what differs is one
   * `tools_attached` and one `tools_detached` event (ids sorted, never empty) of `actor`, and the
   * same set writes nothing. The thread's `tools` is the sorted set, and absent when there are none.
   */
  function setTools(thread: Thread, wanted: readonly string[], actor: Actor) {
    const have = new Set(thread.tools ?? []);
    const want = new Set(wanted);
    const attach = [...want].filter((id) => !have.has(id)).sort();
    const detach = [...have].filter((id) => !want.has(id)).sort();
    const after = [...want].sort();
    if (attach.length > 0) append(thread.id, "tools_attached", actor, { servers: attach });
    if (detach.length > 0) append(thread.id, "tools_detached", actor, { servers: detach });
    if (after.length > 0) thread.tools = after;
    else delete thread.tools;
  }

  /** The servers a log leaves attached: `tools_attached` adds and `tools_detached` takes away, in order. */
  const attachedBy = (log: readonly Event[]): Set<string> => {
    const set = new Set<string>();
    for (const e of log) {
      const ids = Array.isArray(e.data.servers) ? (e.data.servers as string[]) : [];
      if (e.kind === "tools_attached") for (const id of ids) set.add(id);
      if (e.kind === "tools_detached") for (const id of ids) set.delete(id);
    }
    return set;
  };

  /**
   * `PUT /api/threads/{id}/tools` (`putThreadTools`): the whole set wanted, in any state of the
   * thread. A body that is not exactly `{servers: [ids]}` of ids that are server ids is a 400; a server
   * the deployment does not list or does not offer for the thread's agent, or more than 16, is a 422.
   */
  async function putThreadTools(
    req: http.IncomingMessage,
    res: http.ServerResponse,
    thread: Thread,
  ) {
    let body: unknown;
    try {
      body = await readJson(req);
    } catch {
      return problem(res, 400, "Bad Request", "the body is not JSON");
    }
    if (
      !isRecord(body) ||
      Object.keys(body).some((k) => k !== "servers") ||
      !Array.isArray(body.servers) ||
      !body.servers.every((id) => typeof id === "string")
    ) {
      return problem(
        res,
        400,
        "Bad Request",
        "the body is exactly an object with a `servers` array",
      );
    }
    const ids = body.servers as string[];
    const bad = ids.find((id) => !SERVER_ID.test(id));
    if (bad !== undefined) {
      return problem(res, 400, "Bad Request", "a server id is lower-case letters, digits and -");
    }
    const refused = refusedServers(req, ids, thread.target.agentId, new Set(thread.tools ?? []));
    if (refused !== undefined) return problem(res, 422, "Unprocessable", refused);
    setTools(thread, ids, { type: "user", name: meOf(req).user });
    return sendJson(res, 200, { servers: thread.tools ?? [] });
  }

  // ---- scripted agent ------------------------------------------------------------------

  function play(t: Thread, steps: Step[]) {
    const run = runs.get(t.id) ?? { timer: undefined, pending: [], resume: undefined };
    runs.set(t.id, run);
    clearTimeout(run.timer);
    run.release = undefined;
    run.pending = [...steps];
    // `{ pause: "release" }`: waits for the test's `POST /__mock/release`, then goes on a step later
    const hold = () => {
      run.release = () => {
        run.release = undefined;
        run.timer = setTimeout(tick, stepMs);
      };
    };
    const tick = () => {
      const step = run.pending.shift();
      if (!step) return;
      if ("pause" in step) {
        if (step.pause === "release") hold();
        return; // `cancel`: waits for the cancel
      }
      if ("live" in step) {
        send(t.id, { ...step.live, agent: agentActor(t).name, end: step.live.end ?? "open" });
      } else {
        // a description a person wrote is final: the model's, written after it, is dropped
        if (
          step.kind === "thread_described" &&
          step.data.source === "model" &&
          describedByPerson(t.id)
        ) {
          return;
        }
        append(
          t.id,
          step.kind,
          step.system
            ? { type: "system", name: "orchestrator" }
            : step.as
              ? { type: "agent", name: step.as }
              : agentActor(t),
          step.data,
        );
        if (step.setState) setState(t, step.setState);
        // the job is done and a message was sent while it ran: it starts the next job
        if (step.kind === "thread_state" && step.data.state === "done" && run.held?.length) {
          const text = run.held.shift() as string;
          startNextJob(t, text);
          return;
        }
      }
      const next = run.pending[0];
      if (next && "pause" in next && next.pause === "release") {
        // held with the step before it, not a step later: a test that sees that step may release at once
        run.pending.shift();
        return hold();
      }
      run.timer = setTimeout(tick, next && "quick" in next && next.quick ? 0 : stepMs);
    };
    run.timer = setTimeout(tick, stepMs);
  }

  /** The thread's next job starts with `text` (ADR 0020): `job_started`, then the script of the text. */
  function startNextJob(t: Thread, text: string, before: Step[] = []) {
    const job = (events.get(t.id) ?? []).filter((e) => e.kind === "job_started").length + 2;
    const script = scriptFor(text);
    runs.set(t.id, {
      timer: undefined,
      pending: [],
      resume: script.resume,
      steerable: script.steerable,
    });
    play(t, [
      ...before,
      { kind: "job_started", data: { job }, system: true, setState: "queued" },
      ...script.start,
    ]);
  }

  // ---- routes --------------------------------------------------------------------------

  const server = http.createServer((req, res) => {
    handle(req, res).catch((e: unknown) => {
      if (!res.headersSent) problem(res, 500, "Internal error", String(e));
      else res.end();
    });
  });

  async function handle(req: http.IncomingMessage, res: http.ServerResponse) {
    const url = new URL(req.url ?? "/", "http://mock");
    const method = req.method ?? "GET";
    const path = url.pathname;

    if (path === "/healthz" || path === "/readyz") return void res.writeHead(200).end("ok");
    // the issuer's own routes, in browser mode (mock/issuer.ts)
    if (issuer && (await issuer.handle(req, res, url))) return;
    if (path === "/__mock/reset" && method === "POST") {
      reset();
      return void res.writeHead(204).end();
    }
    // Test hooks: cut every open stream (the network went away; `?thread=` cuts that thread's only,
    // so that tests running in parallel against one mock do not cut each other's), cut the next
    // connect stream after `frames` frames, mid-group, or let a run that waits at a `release` step
    // go on (`?thread=`; 409 when that thread's run is not waiting, so a test cannot release nothing).
    if (path === "/__mock/drop-streams" && method === "POST") {
      const only = url.searchParams.get("thread");
      for (const [id, set] of viewers) {
        if (only === null || id === only) for (const v of [...set]) closeViewer(v, true);
      }
      return void res.writeHead(204).end();
    }
    if (path === "/__mock/release" && method === "POST") {
      const run = runs.get(url.searchParams.get("thread") ?? "");
      if (!run?.release) return problem(res, 409, "Not held", "no run of that thread is waiting");
      run.release();
      return void res.writeHead(204).end();
    }
    if (path === "/__mock/cut-next-connect" && method === "POST") {
      cutNextConnectAfter = Number(url.searchParams.get("frames") ?? 0) || 0;
      return void res.writeHead(204).end();
    }
    // The registry as a test sets it (per session, see above): `?down=true` makes it unreachable,
    // a body adds an agent.
    const session = url.searchParams.get("session") ?? "default";
    if (issuer?.hooks(req, res, url, session)) return;
    if (path === "/__mock/registry" && method === "POST") {
      registryOf(session).down = url.searchParams.get("down") === "true";
      return void res.writeHead(204).end();
    }
    // What a test sets for its own session, each only when it is given: `?showDescriptions=false`
    // (`ui.showDescriptions`), and `?me=admin` (who the session is, `PROFILES`: `user`, `admin`,
    // `read-only`, `no-access`; 400 for another name), `?sharing=public` (the deployment's cap on
    // sharing, ADR 0040: `disabled`, `internal` (the default), `public`; `GET /api/me` says it as
    // `sharing` for a role that holds `thread.share`), `?signedIn=false` (no identity: every route
    // but the public ones is a 401) and `?stale=true` (a token that has run out: a 401 on every route
    // but the public ones until `GET /oauth2/userinfo` refreshes it).
    if (path === "/__mock/config" && method === "POST") {
      const state = registryOf(session);
      const shown = url.searchParams.get("showDescriptions");
      const who = url.searchParams.get("me");
      const cap = url.searchParams.get("sharing");
      const signedIn = url.searchParams.get("signedIn");
      const stale = url.searchParams.get("stale");
      if (who !== null && !isProfileName(who)) {
        return problem(res, 400, "Bad Request", `me is one of ${Object.keys(PROFILES).join(", ")}`);
      }
      if (cap !== null && !["disabled", "internal", "public"].includes(cap)) {
        return problem(res, 400, "Bad Request", "sharing is one of disabled, internal, public");
      }
      if (cap !== null) state.sharing = cap as Sharing;
      if (signedIn !== null) state.signedIn = signedIn !== "false";
      if (stale !== null) state.stale = stale === "true";
      if (shown !== null) state.showDescriptions = shown !== "false";
      if (who !== null) state.me = who as ProfileName;
      return void res.writeHead(204).end();
    }
    // A stand-in for the edge's own routes (oauth2-proxy, behind the same origin in a deployment), for the page's
    // sign-in and its refresh (web/README.md "Signing in again"). `GET /oauth2/userinfo` answers a session with 200
    // and refreshes a stale one, answers none with 401 (what oauth2-proxy does for a session it cannot load);
    // `GET /oauth2/start?rd=<path>` signs the session in (the issuer approves anybody) and sends the browser to `rd`,
    // a path of this origin as oauth2-proxy requires. `GET /__mock/edge?session=` says what they did.
    if (path === "/oauth2/userinfo" && method === "GET") {
      const state = registryOf(sessionOf(req));
      res.setHeader("Cache-Control", "no-store");
      if (!state.signedIn) {
        res.writeHead(401, { "Content-Type": "text/plain; charset=utf-8" });
        return void res.end("Unauthorized\n");
      }
      if (state.stale) {
        state.stale = false;
        state.edge.refreshes += 1;
      }
      return sendJson(res, 200, { user: meOf(req).user, email: meOf(req).user });
    }
    if (path === "/oauth2/start" && method === "GET") {
      const state = registryOf(sessionOf(req));
      state.signedIn = true;
      state.stale = false;
      state.edge.signIns += 1;
      const rd = url.searchParams.get("rd") ?? "/";
      res.writeHead(302, {
        Location: /^\/(?!\/)/.test(rd) ? rd : "/",
        "Cache-Control": "no-store",
      });
      return void res.end();
    }
    if (path === "/__mock/edge" && method === "GET") {
      const state = registryOf(session);
      return sendJson(res, 200, { signedIn: state.signedIn, stale: state.stale, ...state.edge });
    }
    // The person a thread belongs to, as a test says it (a thread is made by the session that runs
    // into it, so this is how a thread of someone else gets into the list): `?thread=<id>&owner=<e-mail>`.
    if (path === "/__mock/owner" && method === "POST") {
      const thread = threads.get(url.searchParams.get("thread") ?? "");
      const owner = url.searchParams.get("owner")?.trim().toLowerCase();
      if (!thread || !owner) return problem(res, 404, "Not found", "no such thread, or no owner");
      thread.owner = owner;
      return void res.writeHead(204).end();
    }
    // A thread that is shared as it was when the deployment let it be (a test cannot get there through
    // `PUT …/share` once the cap is lowered): `?thread=<id>&visibility=internal|public`.
    if (path === "/__mock/share" && method === "POST") {
      const thread = threads.get(url.searchParams.get("thread") ?? "");
      const visibility = url.searchParams.get("visibility");
      if (!thread || (visibility !== "internal" && visibility !== "public")) {
        return problem(
          res,
          404,
          "Not found",
          "no such thread, or visibility is not internal or public",
        );
      }
      const now = new Date().toISOString();
      shares.set(thread.id, { visibility, token: newToken(), sharedAt: now });
      append(
        thread.id,
        "thread_shared",
        { type: "user", name: thread.owner },
        shareEventData(thread.id),
      );
      endStaleViewers(thread.id);
      return sendJson(res, 200, { token: shares.get(thread.id)?.token });
    }
    // The MCP servers the deployment offers, as a test sets them for its own session: the body is the
    // whole list (a JSON array of `ToolServer`s, in the deployment's order). It is taken as it is,
    // an icon that is a URL included, so that a test can show the screen never fetches one.
    if (path === "/__mock/tool-servers" && method === "POST") {
      const list = await readJson(req);
      if (!Array.isArray(list)) return problem(res, 400, "Bad Request", "the body is an array");
      registryOf(session).toolServers = list as ToolServer[];
      return void res.writeHead(204).end();
    }
    if (path === "/__mock/registry/agents" && method === "POST") {
      const agent = (await readJson(req)) as Agent;
      registryOf(session).agents.push({ ...agent, source: "registry" });
      return void res.writeHead(204).end();
    }
    // A session that is not signed in (`?signedIn=false`): every route but the public ones is the 401
    // the edge's identity layer gives (the public routes are outside it, ADR 0040)
    const publicRoute = path.startsWith("/api/public/") || path.startsWith("/agui/public/");
    // which kind of deployment this is (ADR 0054, decision 7): the issuer the web signs in at, or none
    if (path === "/api/public/auth" && method === "GET") {
      res.setHeader("Cache-Control", "no-store");
      return issuer
        ? sendJson(res, 200, issuer.publicConfig)
        : problem(res, 404, "Not found", "this deployment has no sign-in of its own");
    }
    if (issuer && publicRoute) issuer.notePublic(req);
    if ((path.startsWith("/api/") || path.startsWith("/agui/")) && !publicRoute) {
      if (issuer) {
        // browser mode: a DPoP-bound token and a proof of the right request, as the orchestrator checks them
        const verified = issuer.verifyApi(req, url);
        if (!verified.ok) {
          res.setHeader(
            "WWW-Authenticate",
            `DPoP error="${verified.error}", algs="ES256 EdDSA", error_description="${verified.description.replace(/"/g, "'")}"`,
          );
          return problem(res, 401, "Unauthorized", verified.description);
        }
        tokenProfiles.set(req, verified.profile);
      } else if (!(registryOf(sessionOf(req)).signedIn && !registryOf(sessionOf(req)).stale)) {
        return problem(res, 401, "Unauthorized", "sign in to continue");
      }
    }
    // a share link, read (ADR 0040): the thread, its files and its stream, for a signed-in reader
    // and, outside the identity layer, for anybody
    const shared =
      /^\/(api|agui)\/(public\/)?shared\/([^/]+)(?:\/(connect|artifacts\/([^/]+)))?$/.exec(path);
    if (shared && method === "GET") {
      const what = shared[4];
      // `/api/…/shared/<token>` and `…/artifacts/<sha>` are the API's, `/agui/…/connect` is the stream
      if ((shared[1] === "agui") !== (what === "connect")) return problem(res, 404, "Not found");
      return readShared(req, res, url, {
        reader: shared[2] ? "public" : "internal",
        token: decodeURIComponent(shared[3] ?? ""),
        stream: what === "connect",
        ...(what?.startsWith("artifacts/") ? { sha256: decodeURIComponent(shared[5] ?? "") } : {}),
      });
    }
    // `GET /api/me` (`getMe`, ADR 0033): who the session is and what its roles let it do; the one
    // route that answers a person whose roles grant nothing. Every other route of the APIs refuses
    // such a person (`no_access`).
    if (path === "/api/me" && method === "GET") return sendJson(res, 200, meOf(req));
    if (
      (path.startsWith("/api/") || path.startsWith("/agui/")) &&
      meOf(req).permissions.length === 0
    ) {
      return problem(
        res,
        403,
        "Forbidden",
        "your roles do not grant access to this API",
        "no_access",
      );
    }
    if (path === "/api/agents" && method === "GET") {
      if (!holds(meOf(req), "agent.read")) return forbid(res, "agent.read");
      return sendJson(res, 200, listedAgents(req));
    }
    // `GET /api/config` (`getConfig`, ADR 0034): the public subset, every `ui` key with its value
    if (path === "/api/config" && method === "GET") {
      const { showDescriptions } = registryOf(sessionOf(req));
      return sendJson(res, 200, { ui: { showDescriptions } });
    }
    if (path === "/api/registry" && method === "GET") {
      if (!holds(meOf(req), "agent.read")) return forbid(res, "agent.read");
      return sendJson(res, 200, {
        sources: [
          { name: "static", status: "ok" },
          registryOf(sessionOf(req)).down
            ? { name: "platform", status: "unavailable", detail: REGISTRY_UNREACHABLE }
            : { name: "platform", status: "ok" },
        ],
      });
    }

    // `GET /api/tool-servers` (`listToolServers`, ADR 0024): what a person may attach, in the
    // deployment's order. It takes `thread.write`, as attaching does; never cached.
    if (path === "/api/tool-servers" && method === "GET") {
      if (!holds(meOf(req), "thread.write")) return forbid(res, "thread.write");
      return sendJson(res, 200, registryOf(sessionOf(req)).toolServers);
    }

    if (path === "/api/threads" && method === "GET") return listThreads(req, res, url);

    const run = /^\/agui\/agents\/([^/]+)$/.exec(path);
    if (run && method === "POST") return runAgent(req, res, decodeURIComponent(run[1] ?? ""));
    const caps = /^\/agui\/agents\/([^/]+)\/capabilities$/.exec(path);
    if (caps && method === "GET") return capabilities(req, res, decodeURIComponent(caps[1] ?? ""));
    const connect = /^\/agui\/threads\/([^/]+)\/connect$/.exec(path);
    if (connect && method === "GET") {
      return connectThread(req, res, url, decodeURIComponent(connect[1] ?? ""));
    }

    // `GET /api/threads/{id}/artifacts/{sha256}` (`getArtifact`, ADR 0032): a file of the thread
    const artifact = /^\/api\/threads\/([^/]+)\/artifacts\/([^/]+)$/.exec(path);
    if (artifact && method === "GET") {
      const me = meOf(req);
      if (!holds(me, "artifact.read")) return forbid(res, "artifact.read");
      const thread = threads.get(decodeURIComponent(artifact[1] ?? ""));
      const file = FILES.get(decodeURIComponent(artifact[2] ?? ""));
      if (!thread || !file) return problem(res, 404, "Not found");
      if (thread.owner !== me.user) return problem(res, 404, "Not found");
      const download = url.searchParams.get("download");
      if (download !== null && !["0", "1", "true", "false"].includes(download)) {
        return problem(res, 400, "Bad Request", "download must be 1");
      }
      res.writeHead(200, fileHeaders(file, download === "1" || download === "true"));
      return void res.end(file.bytes);
    }

    // the owner's share of a thread (ADR 0040): `shareThread`, `unshareThread`, `rotateThreadShare`
    const share = /^\/api\/threads\/([^/]+)\/share(\/rotate)?$/.exec(path);
    if (share) {
      const id = decodeURIComponent(share[1] ?? "");
      if (!share[2] && method === "PUT") return shareThread(req, res, id);
      if (!share[2] && method === "DELETE") return unshareThread(req, res, id);
      if (share[2] && method === "POST") return rotateShare(req, res, id);
      return problem(res, 404, "Not found");
    }

    const m = /^\/api\/threads\/([^/]+)(?:\/(cancel|export|fork|branches|tools))?$/.exec(path);
    if (m) {
      const id = decodeURIComponent(m[1] ?? "");
      const sub = m[2];
      // reading a thread takes `thread.read`; patching, cancelling and forking it take `thread.write`
      // over it (another's thread is a 404 for every role: ADR 0039)
      const acts =
        method === "PATCH" ||
        (method === "POST" && (sub === "cancel" || sub === "fork")) ||
        (method === "PUT" && sub === "tools");
      if (!acts && method !== "GET") return problem(res, 404, "Not found");
      const thread = accessible(req, res, id, acts);
      if (!thread) return;
      if (!sub && method === "GET") {
        return sendJson(res, 200, viewOf(thread, { cap: capOf(req), link: true }));
      }
      if (!sub && method === "PATCH") return patchThread(req, res, thread);
      if (sub === "cancel" && method === "POST") return cancel(res, thread);
      if (sub === "export" && method === "GET") return exportThread(req, res, thread);
      if (sub === "fork" && method === "POST") return forkThread(req, res, thread);
      if (sub === "branches" && method === "GET") return listBranches(res, thread);
      if (sub === "tools" && method === "PUT") return putThreadTools(req, res, thread);
    }
    return problem(res, 404, "Not found");
  }

  /**
   * `PATCH /api/threads/{id}` (`patchThread`): a person renames the thread or writes its description,
   * in any state. The body has `title` and/or `description` and nothing else, and both are checked
   * before either is written. A title is trimmed and one line of 1 to 200 characters; a description
   * is trimmed and one line of 0 to 500 characters, and an empty one clears it. Each is an event of
   * the person (`thread_titled`, `thread_described`, `source: user`) and final; the same title or
   * description again, once a person has written it, writes nothing.
   */
  async function patchThread(req: http.IncomingMessage, res: http.ServerResponse, thread: Thread) {
    let body: unknown;
    try {
      body = await readJson(req);
    } catch {
      return problem(res, 400, "Bad Request", "the body is not JSON");
    }
    if (!isRecord(body)) return problem(res, 400, "Bad Request", "the body must be an object");
    const unknown = Object.keys(body).find((k) => k !== "title" && k !== "description");
    if (unknown !== undefined) {
      return problem(res, 400, "Bad Request", `unknown member \`${unknown}\``);
    }
    if (body.title === undefined && body.description === undefined) {
      return problem(res, 400, "Bad Request", "`title` or `description` is required");
    }
    let title: string | undefined;
    if (body.title !== undefined) {
      if (typeof body.title !== "string") {
        return problem(res, 400, "Bad Request", "`title` must be a string");
      }
      title = body.title.trim();
      if (title === "") return problem(res, 400, "Bad Request", "the title is empty");
      if (/\p{Cc}/u.test(title)) {
        return problem(res, 400, "Bad Request", "the title has a control character");
      }
      if ([...title].length > 200) {
        return problem(res, 400, "Bad Request", "the title is longer than 200 characters");
      }
    }
    let description: string | undefined;
    if (body.description !== undefined) {
      if (typeof body.description !== "string") {
        return problem(res, 400, "Bad Request", "`description` must be a string");
      }
      description = body.description.trim();
      if (/\p{Cc}/u.test(description)) {
        return problem(res, 400, "Bad Request", "the description has a control character");
      }
      if ([...description].length > 500) {
        return problem(res, 400, "Bad Request", "the description is longer than 500 characters");
      }
    }
    if (title !== undefined) {
      const written = (events.get(thread.id) ?? []).some((e) => e.kind === "thread_titled");
      if (!(written && thread.title === title)) {
        thread.title = title;
        append(
          thread.id,
          "thread_titled",
          { type: "user", name: meOf(req).user },
          { title, source: "user" },
        );
      }
    }
    if (description !== undefined) {
      const same = (thread.description ?? "") === description && describedByPerson(thread.id);
      if (!same) {
        append(
          thread.id,
          "thread_described",
          { type: "user", name: meOf(req).user },
          { description, source: "user" },
        );
      }
    }
    return sendJson(res, 200, viewOf(thread, { cap: capOf(req), link: true }));
  }

  /** `GET /api/threads/{id}/export`: the thread, its job, its binding and its whole log, as a file. */
  function exportThread(req: http.IncomingMessage, res: http.ServerResponse, thread: Thread) {
    const view = viewOf(thread);
    // what the real server writes (ADR 0053): the web's revision is the caller's header when it is a plain
    // revision, `unknown` otherwise; the agents are the one the thread is bound to, whose card the mock never reads
    const header = req.headers["x-web-revision"];
    const web =
      typeof header === "string" && /^[A-Za-z0-9._+-]{1,64}$/.test(header) ? header : "unknown";
    const document: components["schemas"]["ThreadExport"] = {
      format: "another-agentic-system/thread-export",
      version: 1,
      exportedAt: new Date().toISOString(),
      thread: view,
      job: {
        number: (events.get(thread.id) ?? []).filter((e) => e.kind === "job_started").length + 1,
        attempt: view.job?.attempt ?? 1,
      },
      binding: { agentId: thread.target.agentId, contextId: thread.id },
      events: events.get(thread.id) ?? [],
      eventsTruncated: false,
      versions: {
        orchestrator: { version: "0.0.0-mock", revision: "unknown" },
        agents: [{ agent: thread.target.agentId, name: null, version: "unknown" }],
        web: { revision: web },
      },
    };
    res.writeHead(200, {
      "Content-Type": "application/json",
      "Content-Disposition": `attachment; filename="thread-${thread.id}.json"`,
      "Cache-Control": "no-store",
    });
    res.end(JSON.stringify(document, null, 2));
  }

  /**
   * `GET /api/threads` (`listThreads`): the caller's own threads and nobody else's, for every role.
   * An `owner` parameter, whatever its value, is a 400 (ADR 0039: it was the administrators' way to
   * list another person's threads, or everyone's).
   */
  function listThreads(req: http.IncomingMessage, res: http.ServerResponse, url: URL) {
    const me = meOf(req);
    if (!holds(me, "thread.read")) return forbid(res, "thread.read");
    if (url.searchParams.has("owner")) {
      return problem(res, 400, "Bad Request", "owner is not supported (ADR 0039)");
    }
    const limit = Math.min(100, Math.max(1, Number(url.searchParams.get("limit") ?? 50) || 50));
    const before = url.searchParams.get("before");
    let all = [...threads.values()].filter((t) => t.owner === me.user).reverse(); // newest first
    // an edit is a branch of a conversation the list already shows (`listBranches` finds it)
    if (url.searchParams.get("branches") !== "include") {
      all = all.filter((t) => links.get(t.id)?.kind !== "edit");
    }
    if (before) {
      const i = all.findIndex((t) => t.id === before);
      all = i >= 0 ? all.slice(i + 1) : [];
    }
    const cap = capOf(req);
    sendJson(
      res,
      200,
      all.slice(0, limit).map((t) => viewOf(t, { cap, link: false })),
    );
  }

  /**
   * `POST /api/threads/{id}/fork` (`forkThread`, ADR 0029): a new thread that begins as a copy of
   * this one. `{after}` copies to the end of the turn that holds that event (a finished job, `done`;
   * a turn that is still going on is a 409 `turn_open`), `{replace, text}` copies to just before a
   * message of the person and then holds the new message and the job it starts (an edit, a branch).
   * `target` says another agent; `id` makes a repeat of the request return the fork it made.
   */
  async function forkThread(req: http.IncomingMessage, res: http.ServerResponse, parent: Thread) {
    let body: unknown;
    try {
      body = await readJson(req);
    } catch {
      return problem(res, 400, "Bad Request", "the body is not JSON");
    }
    if (!isRecord(body)) return problem(res, 400, "Bad Request", "the body must be an object");
    const members = ["after", "replace", "text", "messageId", "target", "id"];
    const extra = Object.keys(body).find((k) => !members.includes(k));
    if (extra !== undefined) return problem(res, 400, "Bad Request", `unknown member \`${extra}\``);
    const { after, replace, text, messageId, target, id } = body;
    if ((after === undefined) === (replace === undefined)) {
      return problem(res, 400, "Bad Request", "exactly one of `after` and `replace`");
    }
    const point = after ?? replace;
    if (typeof point !== "number" || !Number.isInteger(point) || point < 1) {
      return problem(res, 400, "Bad Request", "`after` and `replace` are event numbers, from 1");
    }
    if (after !== undefined && (text !== undefined || messageId !== undefined)) {
      return problem(res, 400, "Bad Request", "`text` and `messageId` go with `replace`");
    }
    if (replace !== undefined) {
      if (typeof text !== "string" || text === "") {
        return problem(res, 400, "Bad Request", "`replace` needs the new `text`");
      }
      if (text.length > 100_000) {
        return problem(res, 400, "Bad Request", "text must be 1 to 100000 characters");
      }
      if (messageId !== undefined && (typeof messageId !== "string" || messageId.length > 256)) {
        return problem(
          res,
          400,
          "Bad Request",
          "`messageId` is a string of at most 256 characters",
        );
      }
    }
    if (id !== undefined && (typeof id !== "string" || !UUID.test(id))) {
      return problem(res, 400, "Bad Request", "`id` must be a UUID");
    }

    let to: Thread["target"] = parent.target;
    if (target !== undefined) {
      if (!isRecord(target) || typeof target.agentId !== "string") {
        return problem(res, 400, "Bad Request", "`target` is {agentId, release?}");
      }
      const agent = findAgent(req, target.agentId);
      if (!agent) {
        if (registryOf(sessionOf(req)).down) {
          res.setHeader("Retry-After", "5");
          return problem(res, 503, "Service Unavailable", "the agent registry is unreachable");
        }
        return problem(res, 400, "Unknown agent", `No agent "${target.agentId}"`);
      }
      const release = target.release;
      if (release !== undefined) {
        if (typeof release !== "string" || !agent.releases) {
          return problem(res, 400, "Bad Request", `${agent.id} does not offer releases`);
        }
        const known =
          release in agent.releases.channels || (agent.releases.revisions ?? []).includes(release);
        if (!known) return problem(res, 400, "Unknown release", `No release "${release}"`);
      }
      to = { agentId: agent.id, ...(typeof release === "string" ? { release } : {}) };
    }

    if (!covers(meOf(req).agents.invoke, to.agentId)) {
      return forbid(res, "agent.invoke", to.agentId);
    }

    const forkId = typeof id === "string" ? id : randomUUID();
    const made = threads.get(forkId);
    if (made) {
      if (links.get(forkId)?.parent !== parent.id) {
        return problem(res, 409, "Conflict", "`id` is the id of another thread");
      }
      res.setHeader("Location", `/api/threads/${forkId}`);
      return sendJson(res, 200, viewOf(made));
    }

    const log = events.get(parent.id) ?? [];
    let cut: number;
    if (after !== undefined) {
      if (point > log.length) return problem(res, 422, "Unprocessable", "no such event");
      const next = log.find(
        (e) => e.seq > point && (e.kind === "user_message" || e.kind === "ui_action"),
      );
      if (next) cut = next.seq - 1;
      else if (isActiveState(parent.state)) {
        return problem(
          res,
          409,
          "Conflict",
          "the turn is still going on; try again when it has ended",
          "turn_open",
        );
      } else cut = log.length;
    } else {
      const replaced = log[point - 1];
      if (!replaced) return problem(res, 422, "Unprocessable", "no such event");
      if (replaced.kind !== "user_message") {
        return problem(res, 422, "Unprocessable", "that event is not a message of a person");
      }
      cut = point - 1;
    }

    const kind = after !== undefined ? "fork" : "edit";
    const now = new Date().toISOString();
    const created: Thread = {
      id: forkId,
      owner: meOf(req).user,
      title: parent.title,
      ...(parent.description ? { description: parent.description } : {}),
      target: to,
      state: "done",
      createdAt: now,
      updatedAt: now,
      lastSeq: cut,
      forkedFrom: { threadId: parent.id, seq: cut, kind },
    };
    threads.set(forkId, created);
    events.set(
      forkId,
      log.slice(0, cut).map((e) => ({ ...e, threadId: forkId })),
    );
    links.set(forkId, { parent: parent.id, cut, kind });
    // the servers the copied log left attached stay (ADR 0024)
    const copied = [...attachedBy(events.get(forkId) ?? [])].sort();
    if (copied.length > 0) created.tools = copied;
    const gate = gates.get(parent.id);
    if (gate) gates.set(forkId, gate);
    const person: Actor = { type: "user", name: meOf(req).user };
    append(forkId, "thread_forked", person, {
      from: { threadId: parent.id, seq: cut },
      kind,
      title: parent.title,
      ...(parent.description ? { description: parent.description } : {}),
      target: to,
    });
    // ...minus those its agent may not use, which the fork's own log detaches, first
    const offered = new Set(offeredFor(req, to.agentId).map((t) => t.id));
    const kept = copied.filter((id) => offered.has(id));
    if (kept.length < copied.length) setTools(created, kept, person);
    if (kind === "edit" && typeof text === "string") {
      const message = append(forkId, "user_message", person, {
        text,
        messageId: typeof messageId === "string" ? messageId : `m-${randomUUID()}`,
      });
      const link = links.get(forkId);
      if (link) link.replacing = message.seq;
      const job = (events.get(forkId) ?? []).filter((e) => e.kind === "job_started").length + 2;
      append(forkId, "job_started", { type: "system", name: "orchestrator" }, { job });
      setState(created, "queued");
      const script = scriptFor(text);
      runs.set(forkId, { timer: undefined, pending: [], resume: script.resume });
      play(created, script.start);
    }
    res.setHeader("Location", `/api/threads/${forkId}`);
    return sendJson(res, 201, viewOf(created));
  }

  /**
   * `GET /api/threads/{id}/branches` (`listBranches`): the messages of this thread that have other
   * versions. The versions of a message are the thread that has the original and the threads made by
   * an edit of it, the edits of an edit included; everything of a thread up to its cut is its
   * parent's, and shows the parent's versions.
   */
  function listBranches(res: http.ServerResponse, thread: Thread) {
    /** The message of `t` at `s`, named by where it came from (the same name: the same message's versions). */
    const identity = (t: string, s: number): string => {
      const link = links.get(t);
      if (link?.kind === "edit") {
        if (s === link.replacing) return slot(t);
        if (s <= link.cut) return identity(link.parent, s);
      }
      return `${t}#${s}`;
    };
    /** The message an edit replaces, named like `identity`. */
    const slot = (t: string): string => {
      const link = links.get(t);
      return link ? identity(link.parent, link.cut + 1) : `${t}#0`;
    };
    /** The thread whose own message `s` of `t` is: `t`, or the ancestor it was copied from. */
    const owner = (t: string, s: number): string => {
      const link = links.get(t);
      return link?.kind === "edit" && s !== link.replacing && s <= link.cut
        ? owner(link.parent, s)
        : t;
    };
    const rootOf = (t: string): string => {
      const link = links.get(t);
      return link?.kind === "edit" ? rootOf(link.parent) : t;
    };
    const root = rootOf(thread.id);
    /** Every thread of the family (edits only), the root first, then in the order they were made. */
    const family = [...threads.keys()].filter((t) => rootOf(t) === root);
    /** The versions of the message `key` names: the original, then the edits in the order made. */
    const versions = (key: string) => {
      const out: { threadId: string; seq: number }[] = [];
      for (const t of family) {
        for (const e of events.get(t) ?? []) {
          if (e.kind === "user_message" && owner(t, e.seq) === t && identity(t, e.seq) === key) {
            out.push({ threadId: t, seq: e.seq });
          }
        }
      }
      return out;
    };
    const points = (events.get(thread.id) ?? []).flatMap((e) => {
      if (e.kind !== "user_message") return [];
      const siblings = versions(identity(thread.id, e.seq));
      if (siblings.length < 2) return [];
      const own = owner(thread.id, e.seq);
      return [
        {
          seq: e.seq,
          index: siblings.findIndex((v) => v.threadId === own),
          siblings: siblings.map((v) => ({
            threadId: v.threadId,
            seq: v.seq,
            title: threads.get(v.threadId)?.title ?? "",
          })),
        },
      ];
    });
    sendJson(res, 200, { root, points });
  }

  /** The agent `agentId`, or the answer for one that is not listed: 503 while the registry is down. */
  function agentOrRefuse(
    req: http.IncomingMessage,
    res: http.ServerResponse,
    agentId: string,
  ): Agent | undefined {
    const agent = findAgent(req, agentId);
    if (agent) return agent;
    if (registryOf(sessionOf(req)).down) {
      res.setHeader("Retry-After", "5");
      problem(res, 503, "Service Unavailable", "the agent registry is unreachable");
    } else {
      problem(res, 404, "Unknown agent", `No agent "${agentId}"`);
    }
    return undefined;
  }

  function capabilities(req: http.IncomingMessage, res: http.ServerResponse, agentId: string) {
    // the agents a person reads are the ones `agent.read` names: another is a 403, not a 404
    const me = meOf(req);
    if (!holds(me, "agent.read") || !covers(me.agents.read, agentId)) {
      return forbid(res, "agent.read", agentId);
    }
    const agent = agentOrRefuse(req, res, agentId);
    if (!agent) return;
    res.setHeader("Cache-Control", "no-store");
    sendJson(res, 200, {
      identity: {
        name: agent.name,
        ...(agent.description ? { description: agent.description } : {}),
      },
      transport: { streaming: true, resumable: true },
      humanInTheLoop: { supported: true, interrupts: true },
      multiAgent: {
        supported: true,
        delegation: true,
        subagents: [
          { name: agent.id, ...(agent.description ? { description: agent.description } : {}) },
        ],
      },
      // the extensions of the orchestrator's own that the live card lists, by exact URI: the key is the signal
      ...(agent.releases ||
      THREAD_TOOLS_AGENTS.has(agent.id) ||
      STEER_AGENTS.has(agent.id) ||
      MENTIONS_AGENTS.has(agent.id)
        ? {
            custom: {
              ...(agent.releases ? { [RELEASE_CHANNELS_URI]: agent.releases } : {}),
              ...(THREAD_TOOLS_AGENTS.has(agent.id) ? { [THREAD_TOOLS_URI]: {} } : {}),
              ...(STEER_AGENTS.has(agent.id) ? { [STEER_URI]: {} } : {}),
              ...(MENTIONS_AGENTS.has(agent.id) ? { [MENTIONS_URI]: {} } : {}),
            },
          }
        : {}),
    });
  }

  function connectThread(
    req: http.IncomingMessage,
    res: http.ServerResponse,
    url: URL,
    threadId: string,
  ) {
    const thread = accessible(req, res, threadId, false);
    if (!thread) return;
    const mode = url.searchParams.get("mode");
    if (mode !== null && mode !== "run") {
      return problem(res, 400, "Invalid request", "mode must be run");
    }
    const header = req.headers["last-event-id"];
    const cursor = typeof header === "string" ? header : "";
    if (cursor !== "" && !/^\d+$/.test(cursor)) {
      return problem(res, 400, "Invalid request", "Last-Event-ID must be a non-negative integer");
    }
    startViewer(res, thread, {
      fromSeq: Number(cursor || 0),
      end: mode === "run" ? "after-replay" : "never",
    });
  }

  // ---- sharing (ADR 0040) ---------------------------------------------------------------

  /** 32 random bytes as 43 characters of unpadded base64url: the shape of a real token (`nonce || MAC`). */
  const newToken = (): string => randomBytes(32).toString("base64url");

  /** `thread_shared`'s data: what the link is stored as, and the hash that tells one link from another. */
  const shareEventData = (threadId: string): Event["data"] => {
    const share = shares.get(threadId);
    return {
      visibility: share?.visibility ?? "internal",
      nonce_sha256: createHash("sha256")
        .update(share?.token ?? "")
        .digest("hex"),
    };
  };

  /**
   * A reader's stream ends when the link is taken down, replaced or narrowed below what it was
   * opened for (docs/api/agui.md "Reading a shared thread"): its reconnect then meets the 404.
   */
  function endStaleViewers(threadId: string) {
    for (const v of [...(viewers.get(threadId) ?? [])]) {
      if (v.reader && !v.reader.valid()) closeViewer(v);
    }
  }

  /** The thread of the caller's, or the 404 that is the answer for every other thread (ADR 0039). */
  function owned(req: http.IncomingMessage, res: http.ServerResponse, id: string) {
    const thread = threads.get(id);
    if (!thread || thread.owner !== meOf(req).user)
      return void problem(res, 404, "Thread not found");
    return thread;
  }

  /** What `shareThread` and `rotateThreadShare` need before they look at the thread: `thread.share`, and a cap above `disabled`. */
  function mayChangeShare(req: http.IncomingMessage, res: http.ServerResponse): boolean {
    if (!holds(meOf(req), "thread.share")) {
      forbid(res, "thread.share");
      return false;
    }
    if (capOf(req) === "disabled") {
      problem(
        res,
        403,
        "Forbidden",
        "sharing is turned off on this deployment",
        "sharing_disabled",
      );
      return false;
    }
    return true;
  }

  /** `PUT /api/threads/{id}/share` (`shareThread`): share, widen or narrow; the same level is the current link. */
  async function shareThread(req: http.IncomingMessage, res: http.ServerResponse, id: string) {
    if (!mayChangeShare(req, res)) return;
    const thread = owned(req, res, id);
    if (!thread) return;
    let body: unknown;
    try {
      body = await readJson(req);
    } catch {
      return problem(res, 400, "Bad Request", "the body is not JSON");
    }
    if (
      !isRecord(body) ||
      Object.keys(body).length !== 1 ||
      (body.visibility !== "internal" && body.visibility !== "public")
    ) {
      return problem(
        res,
        400,
        "Bad Request",
        "the body is `{visibility}`, internal or public (to stop sharing, DELETE)",
      );
    }
    const cap = capOf(req);
    const visibility = body.visibility;
    if (RANK[visibility] > RANK[cap]) {
      return problem(res, 409, "Conflict", `this deployment shares as ${cap} at most`, "over_cap");
    }
    const known = shares.get(thread.id);
    if (known?.visibility === visibility) return sendJson(res, 200, linkView(known, cap));
    const share: Share = {
      visibility,
      token: known?.token ?? newToken(),
      sharedAt: new Date().toISOString(),
    };
    shares.set(thread.id, share);
    append(
      thread.id,
      "thread_shared",
      { type: "user", name: thread.owner },
      shareEventData(thread.id),
    );
    endStaleViewers(thread.id);
    return sendJson(res, 200, linkView(share, cap));
  }

  /** `DELETE /api/threads/{id}/share` (`unshareThread`): needs only ownership; a private thread is left as it is. */
  function unshareThread(req: http.IncomingMessage, res: http.ServerResponse, id: string) {
    const thread = owned(req, res, id);
    if (!thread) return;
    if (shares.delete(thread.id)) {
      append(thread.id, "thread_unshared", { type: "user", name: thread.owner }, {});
      endStaleViewers(thread.id);
    }
    return void res.writeHead(204).end();
  }

  /** `POST /api/threads/{id}/share/rotate` (`rotateThreadShare`): a new link; the old one is a 404. */
  function rotateShare(req: http.IncomingMessage, res: http.ServerResponse, id: string) {
    if (!mayChangeShare(req, res)) return;
    const thread = owned(req, res, id);
    if (!thread) return;
    const known = shares.get(thread.id);
    if (!known) {
      return problem(
        res,
        409,
        "Conflict",
        "the thread is not shared: share it first",
        "not_shared",
      );
    }
    const share: Share = { ...known, token: newToken(), sharedAt: new Date().toISOString() };
    shares.set(thread.id, share);
    append(
      thread.id,
      "thread_shared",
      { type: "user", name: thread.owner },
      shareEventData(thread.id),
    );
    endStaleViewers(thread.id);
    return sendJson(res, 200, linkView(share, capOf(req)));
  }

  /**
   * Reading a link: `getSharedThread` and `getPublicSharedThread`, their files, and the connect
   * streams. **Every** way a link can fail is the one 404 with the one body: a token that cannot be
   * one, one nobody holds, a thread that is private again, a link that was replaced, a cap that was
   * lowered, an `internal` link asked for as anybody. The signed-in routes need `thread.read` (and
   * `artifact.read` for a file); the public ones need nothing and take no identity.
   */
  function readShared(
    req: http.IncomingMessage,
    res: http.ServerResponse,
    url: URL,
    ask: { reader: Reader; token: string; stream: boolean; sha256?: string },
  ) {
    const me = meOf(req);
    const signedIn = ask.reader === "internal";
    if (signedIn && me.permissions.length === 0) {
      return problem(
        res,
        403,
        "Forbidden",
        "your roles do not grant access to this API",
        "no_access",
      );
    }
    if (signedIn && !holds(me, "thread.read")) return forbid(res, "thread.read");
    if (signedIn && ask.sha256 !== undefined && !holds(me, "artifact.read")) {
      return forbid(res, "artifact.read");
    }
    const gone = () => problem(res, 404, "Not found");
    const found = SHARE_TOKEN.test(ask.token)
      ? [...shares.entries()].find(([, share]) => share.token === ask.token)
      : undefined;
    const thread = found ? threads.get(found[0]) : undefined;
    if (!found || !thread) return gone();
    const serves = (): boolean => {
      const share = shares.get(thread.id);
      if (!share || share.token !== ask.token) return false;
      const level = effectiveOf(share, capOf(req));
      return ask.reader === "public" ? level === "public" : level !== "private";
    };
    if (!serves()) return gone();
    const level = effectiveOf(found[1], capOf(req));
    res.setHeader("X-Robots-Tag", "noindex, nofollow");

    if (ask.stream) {
      const mode = url.searchParams.get("mode");
      if (mode !== null && mode !== "run") {
        return problem(res, 400, "Invalid request", "mode must be run");
      }
      const header = req.headers["last-event-id"];
      const cursor = typeof header === "string" ? header : "";
      if (cursor !== "" && !/^\d+$/.test(cursor)) {
        return problem(res, 400, "Invalid request", "Last-Event-ID must be a non-negative integer");
      }
      return startViewer(res, thread, {
        fromSeq: Number(cursor || 0),
        end: mode === "run" ? "after-replay" : "never",
        reader: { kind: ask.reader, valid: serves },
      });
    }

    if (ask.sha256 !== undefined) {
      const file = FILES.get(ask.sha256);
      // the file must be one the thread's own log names, and a public reader has files only when the deployment says so
      const named = (events.get(thread.id) ?? []).some(
        (e) =>
          e.kind === "artifact" &&
          typeof e.data.file === "object" &&
          e.data.file !== null &&
          (e.data.file as { sha256?: unknown }).sha256 === ask.sha256,
      );
      if (!file || !named || ask.reader === "public") return gone();
      const download = url.searchParams.get("download");
      if (download !== null && !["0", "1", "true", "false"].includes(download)) {
        return problem(res, 400, "Bad Request", "download must be 1");
      }
      res.writeHead(200, {
        ...fileHeaders(file, download === "1" || download === "true"),
        "Cache-Control": "no-store",
      });
      return void res.end(file.bytes);
    }

    const job = viewOf(thread).job;
    const view: SharedThread = {
      id: thread.id,
      title: thread.title,
      ...(thread.description ? { description: thread.description } : {}),
      target: thread.target,
      state: thread.state,
      ...(job ? { job } : {}),
      lastSeq: thread.lastSeq,
      visibility: level === "public" ? "public" : "internal",
      isOwner: signedIn && thread.owner === me.user,
      createdAt: thread.createdAt,
      updatedAt: thread.updatedAt,
    };
    return sendJson(res, 200, view);
  }

  const messageText = (m: Record<string, unknown>): string | undefined => {
    if (typeof m.content === "string") return m.content;
    if (!Array.isArray(m.content)) return undefined;
    const parts = m.content.flatMap((p: unknown) => {
      const part = p as { type?: unknown; text?: unknown };
      return part.type === "text" && typeof part.text === "string" ? [part.text] : [];
    });
    return parts.length ? parts.join("\n") : undefined;
  };

  async function runAgent(req: http.IncomingMessage, res: http.ServerResponse, agentId: string) {
    let body: Record<string, unknown> | null;
    try {
      body = (await readJson(req)) as Record<string, unknown> | null;
    } catch {
      return problem(res, 400, "Invalid request", "the body is not JSON");
    }
    if (!body || typeof body !== "object") {
      return problem(res, 400, "Invalid request", "the body is not a RunAgentInput");
    }
    const { threadId, runId } = body;
    if (
      typeof threadId !== "string" ||
      typeof runId !== "string" ||
      !Array.isArray(body.messages)
    ) {
      return problem(res, 400, "Invalid request", "threadId, runId and messages are required");
    }
    if (!UUID.test(threadId))
      return problem(res, 400, "Invalid request", "threadId must be a UUID");
    // a run is a write of a thread, with an agent the roles name: a 403 whatever else is true of it
    const me = meOf(req);
    if (!holds(me, "thread.write")) return forbid(res, "thread.write");
    if (!covers(me.agents.invoke, agentId)) return forbid(res, "agent.invoke", agentId);
    const agent = agentOrRefuse(req, res, agentId);
    if (!agent) return;
    let catalog: CatalogSent | undefined;
    try {
      catalog = await readCatalog(body.forwardedProps);
    } catch (e) {
      if (!(e instanceof BadCatalog)) throw e;
      return problem(
        res,
        e.status,
        e.status === 413 ? "Payload too large" : "Invalid request",
        e.message,
      );
    }

    // how a message sent while a run is open is delivered (ADR 0036): read on every run
    const sendRaw = isRecord(body.forwardedProps) ? body.forwardedProps["vymalo.send"] : undefined;
    if (
      sendRaw !== undefined &&
      sendRaw !== null &&
      sendRaw !== "steer" &&
      sendRaw !== "interrupt"
    ) {
      return problem(
        res,
        400,
        "Invalid request",
        `forwardedProps["vymalo.send"] must be "steer" or "interrupt", got ${JSON.stringify(sendRaw)}`,
      );
    }
    const send = sendRaw === "steer" || sendRaw === "interrupt" ? sendRaw : undefined;

    // `forwardedProps["vymalo.tools"]` is read on every run: one that is not an array of strings is a
    // 400 before the stream. It is applied only by the run that creates the thread.
    let tools: string[] = [];
    const sentTools = isRecord(body.forwardedProps) ? body.forwardedProps[TOOLS_PROP] : undefined;
    if (sentTools !== undefined && sentTools !== null) {
      if (!Array.isArray(sentTools) || !sentTools.every((id) => typeof id === "string")) {
        return problem(res, 400, "Invalid request", "vymalo.tools must be an array of server ids");
      }
      tools = sentTools as string[];
    }

    const messages = body.messages as Record<string, unknown>[];
    const resume = Array.isArray(body.resume)
      ? (body.resume as { interruptId?: string; status?: string; payload?: { text?: unknown } }[])
      : [];
    // an existing thread is the person's to read (else it is not there for them) and to change
    // (else it is theirs to read only); a new id is theirs to make
    const thread = threads.has(threadId) ? accessible(req, res, threadId, true) : undefined;
    if (threads.has(threadId) && !thread) return;
    const log = events.get(threadId) ?? [];
    const known = new Set(log.map((e) => e.data.messageId).filter((v) => typeof v === "string"));
    const fresh = messages.filter(
      (m) => m.role === "user" && typeof m.id === "string" && !known.has(m.id),
    );
    if (messages.some((m) => m.role !== "user" && typeof m.id === "string" && !known.has(m.id))) {
      return problem(res, 422, "Unprocessable", "a new message that is not from the user");
    }

    // `forwardedProps["vymalo.mentions"]` (ADR 0026) is read on every run (400 for a bad shape) and checked
    // against the message it rides with before anything is written; on a run with no message it is ignored
    const sentMentions = readMentions(
      isRecord(body.forwardedProps) ? body.forwardedProps[MENTIONS_PROP] : undefined,
    );
    if (!Array.isArray(sentMentions)) {
      return problem(res, sentMentions.status, sentMentions.title, sentMentions.detail);
    }
    let mentions: MentionRef[] = [];
    const carrier =
      fresh.length === 1 ? messageText(fresh[0] as Record<string, unknown>) : undefined;
    if (sentMentions.length > 0 && carrier !== undefined) {
      const registry = registryOf(sessionOf(req));
      const refused =
        checkAgainstText(carrier, sentMentions) ??
        checkAgents(sentMentions, {
          own: thread?.target.agentId ?? agent.id,
          mayInvoke: (id) => covers(me.agents.invoke, id),
          card: (id) => {
            const configured = AGENTS.find((a) => a.id === id);
            if (configured) return configured.cardUrl ?? null;
            if (registry.down) return "unavailable";
            const listed = registry.agents.find((a) => a.id === id);
            return listed ? (listed.cardUrl ?? null) : undefined;
          },
        });
      if (refused) return problem(res, refused.status, refused.title, refused.detail);
      mentions = sentMentions;
    }

    if (!thread && isRecord(body.forwardedProps) && "a2uiAction" in body.forwardedProps) {
      // the thread has no surface: an action on it reaches nothing
      return problem(res, 422, "Unprocessable", "the thread has no such surface");
    }
    if (!thread) {
      const first = fresh[0];
      const text = first ? messageText(first) : undefined;
      if (fresh.length !== 1 || !first || typeof text !== "string" || text === "") {
        return problem(res, 422, "Unprocessable", "a new thread takes exactly one user message");
      }
      if (text.length > 100_000) {
        return problem(res, 400, "Invalid request", "text must be 1 to 100000 characters");
      }
      const props = (body.forwardedProps ?? {}) as Record<string, { release?: unknown }>;
      const release = props[RELEASE_CHANNELS_URI]?.release;
      if (release !== undefined) {
        if (typeof release !== "string" || !agent.releases) {
          return problem(res, 400, "Invalid request", `${agent.id} does not offer releases`);
        }
        const ok =
          release in agent.releases.channels || (agent.releases.revisions ?? []).includes(release);
        if (!ok) return problem(res, 400, "Unknown release", `No release "${release}"`);
      }
      const refused = refusedServers(req, tools, agent.id, new Set());
      if (refused !== undefined) return problem(res, 422, "Unprocessable", refused);
      const now = new Date().toISOString();
      const created: Thread = {
        id: threadId,
        owner: me.user,
        title: text.slice(0, 60),
        target: { agentId: agent.id, ...(typeof release === "string" ? { release } : {}) },
        state: "queued",
        createdAt: now,
        updatedAt: now,
        lastSeq: 0,
      };
      threads.set(created.id, created);
      events.set(created.id, []);
      recordCatalog(created.id, catalog);
      const script = scriptFor(text);
      if (script.newerCatalog !== undefined) {
        // a newer version of the app opened this thread before: its catalog is the one that counts
        const catalogId = catalog?.catalogId ?? OWN_CATALOG_ID;
        recordCatalog(created.id, {
          catalogId,
          version: script.newerCatalog,
          digest: `sha256:${"9".repeat(64)}`,
          catalog: { catalogId, components: {} },
        });
      }
      append(
        created.id,
        "user_message",
        { type: "user", name: me.user },
        {
          text,
          messageId: first.id as string,
          runId,
          ...(mentions.length > 0 ? { mentions } : {}),
        },
      );
      // the creation commit holds the message, then the servers attached with it (ADR 0024)
      setTools(created, tools, { type: "user", name: me.user });
      if (script.gate) gates.set(created.id, script.gate);
      runs.set(created.id, {
        timer: undefined,
        pending: [],
        resume: script.resume,
        steerable: script.steerable,
      });
      play(created, script.start);
      return startViewer(res, created, {
        fromSeq: 0,
        audience: { skipUserMessageIds: new Set([first.id as string]) },
        end: "first-close",
      });
    }

    if (thread.target.agentId !== agent.id) {
      return problem(res, 409, "Conflict", `the thread targets ${thread.target.agentId}`);
    }
    if (isRecord(body.forwardedProps) && "a2uiAction" in body.forwardedProps) {
      return runAction(
        me.user,
        res,
        thread,
        runId,
        body,
        fresh.length + resume.length > 0,
        catalog,
      );
    }
    // A retry of a run the log holds attaches to it.
    const recorded = log.find((e) => e.kind === "user_message" && e.data.runId === runId);
    if (recorded && fresh.length === 0 && resume.length === 0) {
      return startViewer(res, thread, {
        fromSeq: recorded.seq - 1,
        audience: { skipUserMessageIds: new Set(messages.map((m) => String(m.id))) },
        end: "first-close",
      });
    }
    if (thread.state === "done" || thread.state === "failed" || thread.state === "cancelled") {
      // A thread is a conversation (ADR 0020): a message on a finished thread starts its next
      // job, as a run of its own. Only that: nothing else is for a finished thread to take.
      if (fresh.length !== 1) {
        return problem(res, 422, "Unprocessable", "nothing to run, or more than one new message");
      }
      const next = messageText(fresh[0] as Record<string, unknown>);
      const nextId = fresh[0]?.id as string;
      if (typeof next !== "string" || next === "") {
        return problem(res, 422, "Unprocessable", "a message without text");
      }
      const from = lastSeq(thread.id);
      recordCatalog(thread.id, catalog);
      append(
        thread.id,
        "user_message",
        { type: "user", name: me.user },
        { text: next, messageId: nextId, runId, ...(mentions.length > 0 ? { mentions } : {}) },
      );
      const job = log.filter((e) => e.kind === "job_started").length + 2;
      append(thread.id, "job_started", { type: "system", name: "orchestrator" }, { job });
      setState(thread, "queued");
      // the thread keeps its gate; the script of the new message plays as the new job
      const script = scriptFor(next);
      runs.set(thread.id, { timer: undefined, pending: [], resume: script.resume });
      play(thread, script.start);
      return startViewer(res, thread, {
        fromSeq: from,
        audience: { skipUserMessageIds: new Set([nextId]) },
        end: "first-close",
      });
    }
    if (thread.state === "queued" || thread.state === "working" || thread.state === "verifying") {
      // a message that says how it is delivered is served while a run is open (ADR 0036)
      if (!send || resume.length > 0) {
        return problem(
          res,
          409,
          "Conflict",
          'a run is already open on this thread; wait for it to finish, or send a message with forwardedProps["vymalo.send"] set to "steer" or "interrupt"',
        );
      }
      if (fresh.length !== 1) {
        return problem(res, 422, "Unprocessable", "nothing to run, or more than one new message");
      }
      if (log.some((e) => e.data.runId === runId)) {
        return problem(res, 422, "Unprocessable", "the run id was used before");
      }
      const sent = messageText(fresh[0] as Record<string, unknown>);
      const sentId = fresh[0]?.id as string;
      if (typeof sent !== "string" || sent === "") {
        return problem(res, 422, "Unprocessable", "a message without text");
      }
      return sendWhileRunning(me.user, res, thread, runId, sentId, sent, send, catalog, mentions);
    }
    const answer = resume.find((r) => r.status === "resolved");
    let text: string | undefined;
    let messageId: string | undefined;
    if (answer) {
      if (fresh.length > 0) {
        return problem(res, 422, "Unprocessable", "a resume answer together with a new message");
      }
      text = typeof answer.payload?.text === "string" ? answer.payload.text : undefined;
      if (text === undefined) {
        return problem(res, 422, "Unprocessable", "a resume payload with no text");
      }
    } else {
      if (fresh.length !== 1) {
        return problem(res, 422, "Unprocessable", "nothing to run, or more than one new message");
      }
      text = messageText(fresh[0] as Record<string, unknown>);
      messageId = fresh[0]?.id as string;
      if (typeof text !== "string" || text === "") {
        return problem(res, 422, "Unprocessable", "a message without text");
      }
    }
    const from = lastSeq(thread.id);
    recordCatalog(thread.id, catalog);
    append(
      thread.id,
      "user_message",
      { type: "user", name: me.user },
      { text, ...(messageId ? { messageId } : {}), runId },
    );
    setState(thread, "queued");
    const resumeScript = runs.get(thread.id)?.resume;
    if (resumeScript) play(thread, resumeScript(text));
    return startViewer(res, thread, {
      fromSeq: from,
      audience: { skipUserMessageIds: new Set(messageId ? [messageId] : []) },
      end: "first-close",
    });
  }

  /**
   * A message sent while the agent works (ADR 0036): logged at once, with the `delivery` the core
   * writes (`steer` or `interrupt`; none while the work is verified, when nothing runs). The
   * response is the run the message opens. `steer` is read by the running task of an agent that
   * lists `steer/v1` (the `steerable` script), and reaches any other agent after its turn: the next
   * job starts when this one is done. `interrupt` cancels the
   * running task and starts the next job with the text, with no `thread_state` for the abandoned one.
   */
  function sendWhileRunning(
    who: string,
    res: http.ServerResponse,
    thread: Thread,
    runId: string,
    messageId: string,
    text: string,
    how: "steer" | "interrupt",
    catalog: CatalogSent | undefined,
    mentions: MentionRef[],
  ) {
    const from = lastSeq(thread.id);
    recordCatalog(thread.id, catalog);
    const verifying = thread.state === "verifying";
    append(
      thread.id,
      "user_message",
      { type: "user", name: who },
      {
        text,
        messageId,
        runId,
        ...(verifying ? {} : { delivery: how }),
        ...(mentions.length > 0 ? { mentions } : {}),
      },
    );
    if (verifying) {
      // nothing runs: the message is a plain one, and the agent goes again (ADR 0036, row 4)
      setState(thread, "queued");
      const script = scriptFor(text);
      runs.set(thread.id, { timer: undefined, pending: [], resume: script.resume });
      play(thread, script.start);
    } else if (how === "steer") {
      const run = runs.get(thread.id);
      if (run?.steerable) {
        // the running task reads it at its next step and says what it read
        append(thread.id, "agent_message", agentActor(thread), {
          messageId: nextMessageId(),
          final: true,
          text: `steered: ${text}`,
        });
      } else if (run) run.held = [...(run.held ?? []), text];
    } else {
      const run = runs.get(thread.id);
      if (run) clearTimeout(run.timer);
      startNextJob(thread, text, [
        { kind: "agent_status", data: { status: "canceled", detail: "canceled" } },
      ]);
    }
    return startViewer(res, thread, {
      fromSeq: from,
      fromRun: runId,
      audience: { skipUserMessageIds: new Set([messageId]) },
      end: "first-close",
    });
  }

  /**
   * A user's action on an A2UI surface (`forwardedProps.a2uiAction.userAction`, docs/api/agui.md
   * "Actions"): a run with no message and no `resume`, accepted only while the thread waits for the
   * owner and only for a surface the thread has now. The scripted agent answers `ui-action <name>`.
   */
  function runAction(
    who: string,
    res: http.ServerResponse,
    thread: Thread,
    runId: string,
    body: Record<string, unknown>,
    withInput: boolean,
    catalog: CatalogSent | undefined,
  ) {
    const envelope = (body.forwardedProps as Record<string, unknown>).a2uiAction;
    const action = isRecord(envelope) ? envelope.userAction : undefined;
    if (!isRecord(action)) {
      return problem(res, 422, "Unprocessable", 'expected {"userAction": {...}}');
    }
    const strings: Record<string, string> = {};
    for (const key of ["surfaceId", "name", "sourceComponentId"]) {
      const value = action[key];
      if (typeof value !== "string") {
        return problem(res, 422, "Unprocessable", `${key} must be a string`);
      }
      if (Buffer.byteLength(value) > 256) {
        return problem(res, 413, "Payload too large", `${key} is over 256 bytes`);
      }
      strings[key] = value;
    }
    const context = action.context ?? {};
    if (!isRecord(context)) return problem(res, 422, "Unprocessable", "context must be an object");
    if (Buffer.byteLength(JSON.stringify(context)) > 16 * 1024) {
      return problem(res, 413, "Payload too large", "context is over 16 KiB");
    }
    if (withInput) {
      return problem(res, 422, "Unprocessable", "an action with a message, an answer or a cancel");
    }
    const log = events.get(thread.id) ?? [];
    const version = surfacesOf(log).get(strings.surfaceId as string);
    if (version === undefined) {
      return problem(res, 422, "Unprocessable", "the thread has no such surface");
    }
    if (log.some((e) => e.data.runId === runId)) {
      return problem(res, 422, "Unprocessable", "the run id was used before");
    }
    if (thread.state === "done" || thread.state === "failed" || thread.state === "cancelled") {
      return problem(
        res,
        409,
        "Conflict",
        `this card belongs to a finished request (${thread.state}); write a message to start the next one`,
      );
    }
    if (thread.state !== "blocked") {
      return problem(
        res,
        409,
        "Conflict",
        "a run is already open on this thread; wait for it to finish",
      );
    }
    const from = lastSeq(thread.id);
    recordCatalog(thread.id, catalog);
    append(
      thread.id,
      "ui_action",
      { type: "user", name: who },
      { ...strings, context, version: version as "v0.9" | "v0.9.1" | "v1.0", runId },
    );
    setState(thread, "queued");
    const resumeScript = runs.get(thread.id)?.resume;
    if (resumeScript) play(thread, resumeScript(`ui-action ${strings.name}${chosen(context)}`));
    return startViewer(res, thread, {
      fromSeq: from,
      end: "first-close",
    });
  }

  function cancel(res: http.ServerResponse, thread: Thread) {
    const active =
      thread.state === "queued" ||
      thread.state === "working" ||
      thread.state === "verifying" ||
      thread.state === "blocked";
    if (active) {
      const run = runs.get(thread.id);
      if (run) clearTimeout(run.timer);
      // no agent task is open while the work is verified: the thread is cancelled and nothing else
      play(thread, thread.state === "verifying" ? cancelSteps.slice(1) : cancelSteps);
    }
    res.writeHead(202).end();
  }

  server.on("close", () => {
    clearInterval(refresher);
    reset();
  });
  return server;
}

async function main() {
  const port = Number(process.env.MOCK_PORT ?? 4010);
  const stepMs = process.env.MOCK_STEP_MS ? Number(process.env.MOCK_STEP_MS) : undefined;
  const origins = (
    process.env.MOCK_PUBLIC_ORIGINS ??
    "http://127.0.0.1:3000,http://127.0.0.1:3001,http://127.0.0.1:3002"
  ).split(",");
  const server = createMockServer({
    ...(stepMs === undefined ? {} : { stepMs }),
    // `MOCK_BROWSER_AUTH=1`: the mock is the issuer too and wants DPoP (mock/issuer.ts)
    ...(process.env.MOCK_BROWSER_AUTH
      ? { browserAuth: { origin: `http://127.0.0.1:${port}`, publicOrigins: origins } }
      : {}),
  });
  server.listen(port, "127.0.0.1", () => {
    console.log(`mock orchestrator on http://127.0.0.1:${port}`);
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  void main();
}
