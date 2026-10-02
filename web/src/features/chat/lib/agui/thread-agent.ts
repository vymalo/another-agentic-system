import { AbstractAgent, type BaseEvent, EventType, type RunAgentInput } from "@ag-ui/client";
import { MessageNotSentError } from "@assistant-ui/react";
import createClient from "openapi-fetch";
import { Observable, ReplaySubject, type Subscription } from "rxjs";
import {
  OWN_CATALOG,
  type OwnCatalog,
  shouldSendCatalog,
  UI_CATALOG_PROP,
  type UiCatalogRef,
} from "@/features/chat/lib/a2ui/catalog";
import { problemMessage } from "@/lib/api/client";
import type { paths } from "@/lib/api/schema";
import { signInAgain } from "@/lib/api/session";
import type { ApiActor, ThreadState } from "@/lib/api/types";
import {
  applyLive,
  type Draft,
  isLiveNow,
  type LiveEvent,
  pending,
  resolveGroup,
} from "./live-drafts";
import { readSse } from "./sse";
import {
  A2UI_SURFACE,
  ACTIVITY,
  ACTOR_KEY,
  ACTOR_PART,
  type JobView,
  PURPOSE_KEY,
  PURPOSE_PART,
  parseJob,
  parsePurpose,
  parseToolIds,
  parseUiCatalog,
  RELEASE_CHANNELS_URI,
  TOOLS_PROP,
} from "./vymalo";

/**
 * The AG-UI side of one thread (docs/api/agui.md, ADR 0012).
 *
 * `@assistant-ui/react-ag-ui` drives an `AbstractAgent`: it calls `run(input)` for a run it starts
 * and applies the events that come back. This agent serves those calls from ONE long-lived
 * connect stream per open thread (`GET /agui/threads/{id}/connect`, our extension), so a run the
 * user starts, a run another tab started and a run that was already open when the page loaded all
 * reach the runtime the same way, and a dropped connection resumes with `Last-Event-ID`.
 *
 * - `run(input)` is `POST /agui/agents/{agentId}`, read until `RUN_STARTED` (the run was accepted),
 *   then released. The events of that run are the connect stream's, routed here by `runId`.
 * - The connect stream is read in groups. A frame with an `id:` closes a group (the orchestrator
 *   writes `id:` on the last frame of every log event, with no text message open); only whole
 *   groups are handed on, so a cut connection never leaves half of a message in the runtime, and
 *   the reconnect (`Last-Event-ID` = the last group) replays exactly what was not delivered.
 *   Groups at or below the last delivered `seq` are dropped, whatever the server sends.
 * - A run nobody here asked for (replay of an earlier run, another tab, a webhook) becomes an
 *   `ExternalRun`. `live-runs.ts` hands each one to the runtime, in order, through `adopt()`.
 * - `abortRun()` is a truncation, as in the protocol: it detaches this consumer and never cancels
 *   (the runtime calls it on unmount and on thread switches). Cancelling is `cancel()`.
 * - An A2UI surface (`a2ui-surface`) is handed to the runtime under our own activity type
 *   (`vymalo.a2ui-surface`), untouched. The runtime's own A2UI path converts a surface the moment
 *   it arrives, before any validator can run, drops `v0.9.1` operations (the version our
 *   orchestrator relays) and has no `openUrl`, so the surface stays an activity part that
 *   `lib/a2ui/prepare.ts` validates and the renderer draws (web/README.md, "A2UI surfaces").
 * - The UI catalog (ADR 0023): a run carries `forwardedProps["vymalo.uiCatalog"]` (this build's
 *   `{catalogId, version, digest, catalog}`) when the thread's last `STATE_SNAPSHOT` says it has
 *   none, or an older one, or another digest at the same version; the snapshot's
 *   `thread.uiCatalog` is kept in `ThreadSnapshot` for the renderer's "newer version" rule.
 * - Live text (ADR 0027, docs/api/agui.md "Live text"): a frame marked `metadata["vymalo.live"]`
 *   that is not the log's own is a piece of a reply still being written. It has no `id:` (never a
 *   resume point), so it is read when it arrives, not held in a group, and it never reaches the
 *   runtime, whose transcript is the log: it is a **draft** (`getDrafts()`, see
 *   `live-drafts.ts`), drawn after the turn's parts. The log's message for the same id comes in its
 *   own group as `CONTENT{final}` + `END{final}`, which become the one message the runtime reads,
 *   made of the draft and the rest. A group that cannot be made whole (it continues a draft this
 *   connection never held) is not delivered and the connection is reopened at the last resume
 *   point, which says the message plainly. A cut connection forgets its drafts.
 * - Where a message or a turn is in the log (ADR 0029: forking and editing name events by `seq`):
 *   `seqOfUser(messageId)` is the `seq` of the event a person's message came in, and
 *   `endOfRun(runId)` the `seq` of the last event delivered for a run, which is "an event of the
 *   turn" for `POST /api/threads/{id}/fork {after}`. They are read from the groups as they are
 *   delivered, so they hold for the replay, a live run and a reconnect alike.
 * - A user's action on a surface (`forwardedProps.a2uiAction`, from the runtime's
 *   `sendA2uiAction`, or staged by `stageA2uiAction` when an interrupt is open, which the runtime
 *   refuses to leave unanswered) goes out as a run with no message and no `resume`.
 */

/**
 * A user message the connect stream shows for an external run. `seq` is the log event it was
 * delivered with (the resume point of its group): what a fork or an edit of the message names.
 */
export type ExternalUserMessage = { id: string; text: string; actor?: ApiActor; seq?: number };

/** A run that started without this consumer's `run()`. */
export class ExternalRun {
  /** Every event of the run for the runtime, replayed to whoever subscribes. */
  readonly frames = new ReplaySubject<BaseEvent>();
  readonly userMessages: ExternalUserMessage[] = [];
  /** Resolves when the run's first material is here: a user message, agent output or the end. */
  readonly leadIn: Promise<void>;
  /** Whether the run has been queued for the runtime: only once it has something to show (`ThreadAgent.material`). */
  offered = false;
  private release: () => void = () => {};

  constructor(readonly runId: string) {
    this.leadIn = new Promise<void>((resolve) => {
      this.release = resolve;
    });
  }

  markLeadIn() {
    this.release();
  }
}

export type Connection = "idle" | "connecting" | "open" | "reconnecting";

export type ThreadSnapshot = {
  connection: Connection;
  /** The `seq` of the last delivered group: the resume point (0 = nothing yet). */
  lastSeq: number;
  /** `STATE_SNAPSHOT.thread` of the last group: what the server says the thread is doing. */
  state: ThreadState | undefined;
  title: string | undefined;
  /**
   * `STATE_SNAPSHOT.job` of the last group, when the thread runs under a verification gate
   * (attempt, attempts there are, sources, commit); null without a gate (ADR 0018).
   */
  job: JobView | null;
  /**
   * `STATE_SNAPSHOT.thread.uiCatalog`: the catalog the thread's agents were told about (ADR 0023),
   * which decides whether a run carries this build's own and which surfaces it can draw;
   * undefined for a thread that has none.
   */
  uiCatalog: UiCatalogRef | undefined;
  /**
   * `STATE_SNAPSHOT.thread.tools`: the ids of the MCP servers attached to the thread (ADR 0024),
   * sorted; undefined when there are none (the member is absent then).
   */
  tools: string[] | undefined;
  /**
   * The `RUN_ERROR` that ended the newest run (`code` such as `agent_failed`, `checks_failed`), so
   * the page can say why a thread failed; null while a run is open and after a run that did not fail.
   */
  failure: { code: string; message: string } | null;
  /** The `runId` of the run that is open according to the delivered frames, if any. */
  openRun: string | null;
  /**
   * The newest run ended in an interrupt: the agent asked something and waits for the answer. The
   * runtime's own interrupt list says the same until a send fails and takes it back; this does
   * not, so the header can say "Your turn" for as long as the thread waits for the person.
   */
  waiting: boolean;
  /** The connect stream answered 404: the thread does not exist for this user. */
  notFound: boolean;
  /** The last connect failure that is not a plain disconnect (401, 5xx). */
  error: string | null;
  /** How many sends the server refused, or could not be reached for (a counter, never reset). */
  sendFailures: number;
};

/**
 * A run the server refused or could not be reached for: nothing of it reached the log. It is
 * assistant-ui's `MessageNotSentError`, the signal a runtime gives for a send that never happened:
 * the composer takes its draft back, and the rejection is not reported as a crash.
 */
export class SendError extends MessageNotSentError {
  constructor(
    message: string,
    readonly status?: number,
    /** The refused run was an A2UI action: it carried no message, so there is nothing to take back. */
    readonly action = false,
  ) {
    super(message);
    this.name = "SendError";
  }
}

export type Target = {
  agentId: string | null;
  release: string | null;
  /**
   * The MCP servers a new chat attaches (ADR 0024): sent as `forwardedProps["vymalo.tools"]` on the
   * run that creates the thread, and only then. Absent for an open thread, whose set is changed
   * with `PUT /api/threads/{id}/tools` (`features/tools`).
   */
  tools?: readonly string[] | undefined;
};

export type ThreadAgentOptions = {
  threadId: string;
  fetch?: typeof fetch;
  /** Empty in the app (the page's own origin); tests use an absolute one. */
  baseUrl?: string;
  /** The agent and release a send goes to (read at send time). */
  target: () => Target;
  /** The UI catalog this build sends (ADR 0023); the build's own unless a test says otherwise. */
  catalog?: OwnCatalog;
  /** A send begins (the composer clears its last error). */
  onSending?: () => void;
  /** A run was accepted (the server answered `RUN_STARTED`). */
  onAccepted?: (info: { threadId: string; runId: string }) => void;
  /** Delay before reconnect attempt `attempt` (0-based), in ms. */
  backoff?: (attempt: number) => number;
  sleep?: (ms: number, signal: AbortSignal) => Promise<void>;
};

export const backoffMs = (attempt: number): number => Math.min(30_000, 1000 * 2 ** attempt);

const defaultSleep = (ms: number, signal: AbortSignal) =>
  new Promise<void>((resolve) => {
    if (signal.aborted) return resolve();
    const timer = setTimeout(resolve, ms);
    signal.addEventListener(
      "abort",
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
  });

/** A group of the connect stream could not be told whole: reconnect at the last resume point. */
class Resync extends Error {}

type Ev = BaseEvent & Record<string, unknown>;
type Group = { events: Ev[]; id: number | undefined };
type Route =
  | { kind: "claimed"; runId: string; sink: ReplaySubject<BaseEvent> }
  | { kind: "external"; runId: string; run: ExternalRun };

const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

const actorOf = (event: Ev): ApiActor | undefined => {
  const meta = event.metadata;
  const actor = isRecord(meta) ? meta[ACTOR_KEY] : undefined;
  return isRecord(actor) && str(actor.name) ? (actor as unknown as ApiActor) : undefined;
};

/** Text message frames of a user message: they belong to the transcript, not to a run's reply. */
const isUserText = (e: Ev, open: Set<string>): boolean => {
  const id = str(e.messageId);
  if (e.type === EventType.TEXT_MESSAGE_START) return e.role === "user";
  if (e.type === EventType.TEXT_MESSAGE_CONTENT || e.type === EventType.TEXT_MESSAGE_END) {
    return id !== undefined && open.has(id);
  }
  return false;
};

export class ThreadAgent extends AbstractAgent {
  private readonly client;
  private readonly options: ThreadAgentOptions;
  private snapshot: ThreadSnapshot = {
    connection: "idle",
    lastSeq: 0,
    state: undefined,
    title: undefined,
    job: null,
    uiCatalog: undefined,
    tools: undefined,
    failure: null,
    openRun: null,
    waiting: false,
    notFound: false,
    error: null,
    sendFailures: 0,
  };
  private readonly listeners = new Set<() => void>();
  // The replies that are still being written (live text). Apart from the snapshot, which moves
  // with every log event: a draft grows several times a second, and only the turn that draws it
  // should render for that.
  private drafts: readonly Draft[] = [];
  private readonly draftListeners = new Set<() => void>();
  private connectAbort: AbortController | undefined;
  private started = false;
  private pending: { events: Ev[] } = { events: [] };
  private route: Route | null = null;
  private readonly claims = new Map<string, ReplaySubject<BaseEvent>>();
  private readonly startedInvocations = new Set<string>();
  private readonly userTexts = new Map<string, ExternalUserMessage>();
  private readonly openUserText = new Set<string>();
  private readonly queue: ExternalRun[] = [];
  private waiter: ((run: ExternalRun | null) => void) | undefined;
  private adopted: ExternalRun | null = null;
  private posting: AbortController | undefined;
  private sendError: SendError | null = null;
  private stagedAction: Record<string, unknown> | undefined;
  /** The `seq` of the group being delivered, for `userSeqs` and `runEnds`. */
  private groupSeq = 0;
  private readonly userSeqs = new Map<string, number>();
  private readonly runEnds = new Map<string, number>();

  constructor(options: ThreadAgentOptions) {
    super({ threadId: options.threadId });
    this.options = options;
    const fetchImpl = options.fetch ?? ((...args) => globalThis.fetch(...args));
    this.client = createClient<paths>({
      baseUrl: options.baseUrl ?? "",
      fetch: (request) => fetchImpl(request),
    });
    // the connect stream's reconnect meets the expired session first: send the person to sign in
    this.client.use(signInAgain);
  }

  // ---- the observable state (useSyncExternalStore) ------------------------------------------

  getSnapshot = (): ThreadSnapshot => this.snapshot;

  onChange = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  /**
   * The replies that are still being written, oldest first, never in the runtime's transcript:
   * the turn draws them after its parts and the log's own message takes over (`live-drafts.ts`).
   */
  getDrafts = (): readonly Draft[] => this.drafts;

  /**
   * The `seq` of the log event a person's message arrived in (`TEXT_MESSAGE_START` of `messageId`),
   * once the group has been delivered; undefined before that, and for a message this page did not
   * see (a thread opened past it).
   */
  seqOfUser = (messageId: string): number | undefined => this.userSeqs.get(messageId);

  /**
   * The `seq` of the last event delivered for the run `runId`: an event of the turn it belongs to,
   * the one a fork "from here" names (the server finds the end of the turn from it).
   */
  endOfRun = (runId: string): number | undefined => this.runEnds.get(runId);

  onDraftsChange = (listener: () => void): (() => void) => {
    this.draftListeners.add(listener);
    return () => void this.draftListeners.delete(listener);
  };

  private setDrafts(next: readonly Draft[]) {
    if (next === this.drafts) return;
    this.drafts = next;
    for (const l of [...this.draftListeners]) l();
  }

  private patch(p: Partial<ThreadSnapshot>) {
    this.snapshot = { ...this.snapshot, ...p };
    for (const l of [...this.listeners]) l();
  }

  // ---- the connect stream --------------------------------------------------------------------

  /** Start following the thread, from the last delivered `seq` on. Idempotent. */
  start() {
    if (this.started) return;
    this.started = true;
    this.connectAbort = new AbortController();
    void this.connectLoop(this.connectAbort.signal);
  }

  /**
   * Stop following (unmount, or nothing left to follow). A later `start()` resumes from the cursor.
   *
   * A send in flight is left alone: it is the runtime's run, which only `abortRun()` truncates.
   * The connect stream can deliver a run whole, and the page pause on its `Done`, before the
   * POST's own `RUN_STARTED` arrives; aborting the POST then would drop the reply it already holds.
   */
  stop() {
    this.started = false;
    this.connectAbort?.abort();
  }

  private async connectLoop(signal: AbortSignal) {
    const backoff = this.options.backoff ?? backoffMs;
    const sleep = this.options.sleep ?? defaultSleep;
    let attempt = 0;
    let opened = false;
    while (!signal.aborted) {
      this.patch({ connection: opened ? "reconnecting" : "connecting" });
      try {
        const cursor = this.snapshot.lastSeq;
        const { data, response, error } = await this.client.GET(
          "/agui/threads/{threadId}/connect",
          {
            params: {
              path: { threadId: this.threadId },
              header: cursor > 0 ? { "Last-Event-ID": String(cursor) } : {},
            },
            parseAs: "stream",
            headers: { Accept: "text/event-stream" },
            signal,
          },
        );
        if (response.status === 404) {
          this.patch({ notFound: true, connection: "idle" });
          return;
        }
        if (!data) throw new Error(problemMessage(error));
        opened = true;
        attempt = 0;
        this.patch({ connection: "open", error: null });
        for await (const frame of readSse(data, signal)) this.onFrame(frame.data, frame.id);
      } catch (e) {
        // a group that could not be made whole is a reconnect, not a failure to report
        if (!signal.aborted && !(e instanceof Resync)) {
          this.patch({ error: e instanceof Error ? e.message : String(e) });
        }
      }
      // A cut connection: what was read after the last `id:` is discarded (the server sends it
      // again from the cursor), and so are the replies that were being written (the new
      // connection says them again from the start, or says the final message), then reconnect.
      this.pending = { events: [] };
      this.dropDrafts();
      if (signal.aborted) return;
      this.patch({ connection: "reconnecting" });
      await sleep(backoff(attempt++), signal);
    }
  }

  private onFrame(data: string, id: string | undefined) {
    let event: Ev;
    try {
      const parsed: unknown = JSON.parse(data);
      if (!isRecord(parsed) || typeof parsed.type !== "string") throw new Error("no type");
      event = parsed as Ev;
    } catch {
      console.warn("Dropping an unparseable AG-UI frame");
      return;
    }
    if (isLiveNow(event)) {
      // a piece of a reply still being written: no `id:`, so no group to wait for
      this.live(event);
      return;
    }
    this.pending.events.push(event);
    if (id === undefined) return;
    const events = this.pending.events;
    this.pending = { events: [] };
    const seq = Number(id);
    if (!Number.isSafeInteger(seq) || seq < 0) {
      console.warn(`Dropping a group with a bad id: ${id}`);
      return;
    }
    if (seq <= this.snapshot.lastSeq) return; // delivered before
    if (!this.deliver({ events, id: seq })) throw new Resync();
  }

  // ---- live text ------------------------------------------------------------------------------

  private live(event: Ev) {
    this.setDrafts(applyLive(this.drafts, event as LiveEvent));
  }

  private dropDrafts() {
    if (this.drafts.length > 0) this.setDrafts([]);
  }

  // ---- routing --------------------------------------------------------------------------------

  /** False when the group cannot be told whole (see `resolveGroup`): nothing of it was delivered. */
  private deliver(group: Group): boolean {
    // the replies the last group completed have reached the transcript by now
    const resolved = resolveGroup(pending(this.drafts), group.events as LiveEvent[]);
    if (!resolved) return false;
    this.setDrafts(resolved.drafts);
    this.groupSeq = group.id ?? this.groupSeq;
    for (const event of resolved.events) this.route1(event as Ev);
    if (group.id !== undefined) this.patch({ lastSeq: group.id });
    return true;
  }

  private route1(event: Ev) {
    switch (event.type) {
      case EventType.RUN_STARTED: {
        this.openUserText.clear();
        const runId = str(event.runId) ?? "";
        this.runEnds.set(runId, this.groupSeq);
        if (this.route?.runId === runId) return; // the preamble of a resumed run: already open
        if (this.route) this.finish(this.route, false);
        this.startedInvocations.clear();
        this.dropDrafts();
        const sink = this.claims.get(runId);
        if (sink) {
          // The POST answered RUN_STARTED to `run()`, which emitted it: not again.
          this.route = { kind: "claimed", runId, sink };
        } else {
          // queued for the runtime at its first material event, not here: a run that holds only
          // a snapshot (a rename of a finished thread) has nothing for the transcript
          const run = new ExternalRun(runId);
          this.route = { kind: "external", runId, run };
          run.frames.next(event);
        }
        this.patch({ openRun: runId, failure: null, waiting: false });
        return;
      }
      case EventType.STATE_SNAPSHOT: {
        const thread = isRecord(event.snapshot) ? event.snapshot.thread : undefined;
        if (isRecord(thread)) {
          const state = str(thread.state) as ThreadState | undefined;
          this.patch({
            state,
            title: str(thread.title) ?? this.snapshot.title,
            // a snapshot without a job is a thread without a gate
            job: parseJob(isRecord(event.snapshot) ? event.snapshot.job : undefined),
            // likewise: a snapshot without a catalog is a thread without one
            uiCatalog: parseUiCatalog(thread.uiCatalog) ?? undefined,
            // likewise: no `tools` is no server attached
            tools: parseToolIds(thread.tools),
          });
        }
        break;
      }
      case EventType.RUN_ERROR:
        this.patch({
          failure: { code: str(event.code) ?? "", message: str(event.message) ?? "" },
          waiting: false,
        });
        break;
      case EventType.RUN_FINISHED:
        this.patch({ waiting: isRecord(event.outcome) && event.outcome.type === "interrupt" });
        break;
      default:
    }
    if (!this.route) {
      console.warn(`Dropping ${event.type} outside a run`);
      return;
    }
    const route = this.route;
    this.runEnds.set(route.runId, this.groupSeq);
    if (isUserText(event, this.openUserText)) {
      this.userText(route, event);
      return;
    }
    if (event.type === EventType.SUBAGENT_STARTED) {
      const invocation = str(event.subagentRunId) ?? "";
      if (this.startedInvocations.has(invocation)) return; // the preamble of a resumed run
      this.startedInvocations.add(invocation);
    }
    const ends = event.type === EventType.RUN_FINISHED || event.type === EventType.RUN_ERROR;
    if (route.kind === "external" && !route.run.offered && ends) {
      // A run that ends with nothing for the transcript but its snapshots: the snapshots have
      // updated this snapshot (the title, the state), and the runtime never hears of the run. A
      // failure or a wait it says again is the page's already (`failure`, `waiting`).
      this.finish(route, true);
      return;
    }
    if (route.kind === "external" && event.type !== EventType.STATE_SNAPSHOT) {
      this.material(route.run);
    }
    for (const out of this.normalize(event, route.runId)) this.emit(route, out);
    if (event.type === EventType.RUN_FINISHED || event.type === EventType.RUN_ERROR) {
      this.finish(route, true);
    }
  }

  /** A user text message: dropped in a claimed run (the requester holds it), else a transcript entry. */
  private userText(route: Route, e: Ev) {
    const id = str(e.messageId) ?? "";
    if (e.type === EventType.TEXT_MESSAGE_START) {
      this.openUserText.add(id);
      const actor = actorOf(e);
      this.userSeqs.set(id, this.groupSeq);
      this.userTexts.set(id, {
        id,
        text: "",
        seq: this.groupSeq,
        ...(actor ? { actor } : {}),
      });
    } else if (e.type === EventType.TEXT_MESSAGE_CONTENT) {
      const m = this.userTexts.get(id);
      if (m) m.text += str(e.delta) ?? "";
    } else {
      this.openUserText.delete(id);
      const m = this.userTexts.get(id);
      this.userTexts.delete(id);
      if (m && route.kind === "external") {
        route.run.userMessages.push(m);
        this.material(route.run);
      }
    }
  }

  /**
   * The external run has something for the transcript: hand it to the runtime (`nextExternalRun`),
   * once, and let `leadIn` go.
   */
  private material(run: ExternalRun) {
    if (!run.offered) {
      run.offered = true;
      this.queue.push(run);
      this.waiter?.(this.queue.shift() ?? null);
    }
    run.markLeadIn();
  }

  private emit(route: Route, event: BaseEvent) {
    if (route.kind === "claimed") route.sink.next(event);
    else route.run.frames.next(event);
  }

  private finish(route: Route, terminal: boolean) {
    if (route.kind === "claimed") {
      route.sink.complete();
      this.claims.delete(route.runId);
    } else {
      route.run.markLeadIn();
      route.run.frames.complete();
    }
    if (this.route === route) this.route = null;
    // the run is over: a reply that was still being written is not (the overlay ended it first)
    if (terminal) {
      this.dropDrafts();
      this.patch({ openRun: null });
    }
  }

  /**
   * What the runtime gets. It drops an event's `metadata`, so the actor moves into the activity
   * content, and into a `CUSTOM` marker part in front of an invocation's output (with the `runId`
   * of the run, which is how a turn finds its end in the log, `endOfRun`). What an assistant's
   * words are for (`vymalo.purpose`, ADR 0031) moves into a marker part right before the text it
   * marks, for the same reason: the turn reads it to keep the answer in the chat and file the
   * working text with the steps.
   */
  private normalize(event: Ev, runId: string): BaseEvent[] {
    if (event.type === EventType.ACTIVITY_SNAPSHOT) {
      const type = str(event.activityType) ?? "";
      const actor = actorOf(event);
      if (type === A2UI_SURFACE && isRecord(event.content)) {
        // the label is the orchestrator's `vymalo.actor`; `surface` ties updates of one surface together
        return [
          {
            ...event,
            activityType: ACTIVITY.surface,
            content: {
              ...event.content,
              surface: str(event.messageId) ?? "",
              ...(actor ? { actor } : {}),
            },
          } as BaseEvent,
        ];
      }
      if (actor && type.startsWith("vymalo.") && isRecord(event.content)) {
        return [{ ...event, content: { ...event.content, actor } } as BaseEvent];
      }
    }
    if (event.type === EventType.TEXT_MESSAGE_START && event.role === "assistant") {
      const purpose = parsePurpose(
        isRecord(event.metadata) ? event.metadata[PURPOSE_KEY] : undefined,
      );
      if (purpose) {
        const messageId = str(event.messageId);
        return [
          {
            type: EventType.CUSTOM,
            name: PURPOSE_PART,
            value: { purpose, ...(messageId ? { messageId } : {}) },
          } as BaseEvent,
          event,
        ];
      }
    }
    if (event.type === EventType.SUBAGENT_STARTED) {
      const actor = actorOf(event);
      if (actor) {
        // `runId` ties the turn the runtime builds to the run of the log, for a fork from it
        return [
          event,
          { type: EventType.CUSTOM, name: ACTOR_PART, value: { ...actor, runId } } as BaseEvent,
        ];
      }
    }
    return [event];
  }

  // ---- external runs, for live-runs.ts --------------------------------------------------------

  /** The next run nobody here started, in log order; null when `signal` aborts first. */
  nextExternalRun(signal?: AbortSignal): Promise<ExternalRun | null> {
    const run = this.queue.shift();
    if (run) return Promise.resolve(run);
    if (signal?.aborted) return Promise.resolve(null);
    return new Promise((resolve) => {
      const done = (next: ExternalRun | null) => {
        signal?.removeEventListener("abort", onAbort);
        if (this.waiter === done) this.waiter = undefined;
        resolve(next);
      };
      const onAbort = () => done(null);
      signal?.addEventListener("abort", onAbort, { once: true });
      this.waiter = done;
    });
  }

  /** The next `run()` serves this run instead of sending a POST. */
  adopt(run: ExternalRun) {
    this.adopted = run;
  }

  // ---- runs -----------------------------------------------------------------------------------

  run(input: RunAgentInput): Observable<BaseEvent> {
    const adopted = this.adopted;
    this.adopted = null;
    if (adopted) return adopted.frames.asObservable();
    const action = this.takeAction(input);
    return new Observable<BaseEvent>((subscriber) => {
      const sink = new ReplaySubject<BaseEvent>();
      const abort = new AbortController();
      this.claims.set(input.runId, sink);
      this.posting = abort;
      this.options.onSending?.();
      this.sendError = null;
      let inner: Subscription | undefined;
      let accepted = false;
      this.post(input, action, abort.signal).then(
        (started) => {
          accepted = true;
          if (this.posting === abort) this.posting = undefined;
          // The run's events come by the connect stream, which a finished thread had paused
          // (nothing more to read): a thread never locks (ADR 0020), so a send on it starts a run
          // and the stream must follow. The run is open from here, unless the connect stream
          // already delivered it whole.
          if (this.claims.has(input.runId) && this.snapshot.openRun === null) {
            this.patch({ openRun: input.runId });
          }
          this.options.onAccepted?.({ threadId: this.threadId, runId: input.runId });
          subscriber.next(started);
          inner = sink.subscribe(subscriber);
        },
        (e: unknown) => {
          this.claims.delete(input.runId);
          if (abort.signal.aborted) return subscriber.complete();
          const error =
            e instanceof SendError ? e : new SendError(problemMessage(e), undefined, !!action);
          this.sendError = error;
          subscriber.error(error);
          // after the runtime has taken the failure (it removes the failed message in `onError`):
          // an observer that re-renders now must not see the transcript half way
          this.patch({ sendFailures: this.snapshot.sendFailures + 1 });
        },
      );
      return () => {
        inner?.unsubscribe();
        abort.abort();
        if (!accepted) this.claims.delete(input.runId);
      };
    });
  }

  /** The error of the last refused send, once: the runtime reports it through `onError`. */
  takeSendError(): SendError | null {
    const e = this.sendError;
    this.sendError = null;
    return e;
  }

  /**
   * The user's action on an A2UI surface that this run carries, once: the runtime's own
   * (`forwardedProps.a2uiAction.userAction`) or the one the app staged. It rides exactly one run.
   */
  private takeAction(input: RunAgentInput): Record<string, unknown> | undefined {
    const props = input.forwardedProps;
    const envelope = isRecord(props) ? props.a2uiAction : undefined;
    const fromRuntime =
      isRecord(envelope) && isRecord(envelope.userAction) ? envelope.userAction : undefined;
    const action = fromRuntime ?? this.stagedAction;
    this.stagedAction = undefined;
    return action;
  }

  /**
   * The next `run()` carries this action instead of a message or a `resume`. The runtime refuses
   * `sendA2uiAction` while an interrupt is open, so the app answers the interrupt through the
   * runtime (which starts the run) and stages the action here; the interrupt is answered by the
   * action on the server side (docs/api/agui.md, "Actions"). `clearStagedAction` is for when the
   * run never started.
   */
  stageA2uiAction(action: Record<string, unknown>) {
    this.stagedAction = action;
  }

  clearStagedAction() {
    this.stagedAction = undefined;
  }

  /**
   * `{"vymalo.uiCatalog": …}` when this run should tell the thread about this build's catalog:
   * the thread has none (a new thread, or one nobody told yet), or ours is newer (ADR 0023,
   * docs/api/agui.md "Inbound"). An older build sends nothing, so it cannot move a thread back.
   * The orchestrator records a digest once, so a run that sends it needlessly costs bytes only.
   */
  private catalogProps(): Record<string, unknown> {
    const own = this.options.catalog ?? OWN_CATALOG;
    return shouldSendCatalog(own, this.snapshot.uiCatalog) ? { [UI_CATALOG_PROP]: own } : {};
  }

  private async post(
    input: RunAgentInput,
    action: Record<string, unknown> | undefined,
    signal: AbortSignal,
  ): Promise<BaseEvent> {
    const { agentId, release, tools } = this.options.target();
    if (!agentId) throw new SendError("Choose an agent first.", undefined, !!action);
    const resume = input.resume?.length ? input.resume : undefined;
    // The orchestrator owns the history: it wants the one new user message, or the `resume`
    // answering an interrupt, and refuses both together (docs/api/agui.md, "Inbound"). An action
    // comes with neither.
    const last = input.messages.at(-1);
    const messages = !action && !resume && last?.role === "user" ? [last] : [];
    const { data, error, response } = await this.client.POST("/agui/agents/{agentId}", {
      params: { path: { agentId } },
      body: {
        threadId: this.threadId,
        runId: input.runId,
        messages,
        state: {},
        tools: [],
        context: [],
        forwardedProps: {
          ...(action
            ? { a2uiAction: { userAction: action } }
            : release
              ? { [RELEASE_CHANNELS_URI]: { release } }
              : {}),
          // the servers of a new chat ride the run that creates it; an action carries nothing else
          ...(tools?.length && !action ? { [TOOLS_PROP]: [...tools] } : {}),
          ...this.catalogProps(),
        },
        ...(resume && !action ? { resume } : {}),
      },
      parseAs: "stream",
      headers: { Accept: "text/event-stream" },
      signal,
    });
    if (!data) throw new SendError(problemMessage(error), response.status, !!action);
    for await (const frame of readSse(data, signal)) {
      let event: Ev;
      try {
        event = JSON.parse(frame.data) as Ev;
      } catch {
        continue;
      }
      if (event.type === EventType.RUN_STARTED) return event;
      if (event.type === EventType.RUN_ERROR) {
        throw new SendError(str(event.message) ?? "The run failed to start.", undefined, !!action);
      }
    }
    throw new SendError("The stream ended before the run started.", undefined, !!action);
  }

  /** Truncation, never cancellation: the run goes on (AG-UI lifecycle; see the class comment). */
  override abortRun() {
    this.posting?.abort();
  }

  /** `POST /api/threads/{id}/cancel`; the outcome arrives as `RUN_FINISHED{cancelled}`. */
  async cancel(): Promise<void> {
    const { error, response } = await this.client.POST("/api/threads/{threadId}/cancel", {
      params: { path: { threadId: this.threadId } },
    });
    if (!response.ok) throw new SendError(problemMessage(error), response.status);
  }
}
