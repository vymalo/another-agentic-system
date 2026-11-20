import type { components } from "./schema";

export type ApiThread = components["schemas"]["Thread"];
export type ApiBranches = components["schemas"]["Branches"];
export type ApiBranchPoint = ApiBranches["points"][number];
export type ApiAgent = components["schemas"]["Agent"];
export type ApiMention = components["schemas"]["Mention"];
export type ApiMe = components["schemas"]["Me"];
export type ApiPermission = components["schemas"]["Permission"];
/** What the caller used in a UTC month, priced (`getMyUsage`, ADR 0065); every amount is a string, never a number. */
export type ApiMyUsage = components["schemas"]["MyUsage"];
export type ApiUsageModelLine = components["schemas"]["UsageModelLine"];
export type ApiUsageSandboxLine = components["schemas"]["UsageSandboxLine"];
/** How the person's own list shows a thread (`arrangeThread`, ADR 0042): pin, archive, eject or place it. */
export type ApiArrangement = components["schemas"]["ThreadArrangement"];
export type ApiToolServer = components["schemas"]["ToolServer"];
/** One of the person's own MCP servers, as `listUserToolServers` shows it (ADR 0046): no secret, only `hasCredential`. */
export type ApiUserToolServer = components["schemas"]["UserToolServer"];
export type ApiUserToolServers = components["schemas"]["UserToolServers"];
export type ApiUserToolAuth = components["schemas"]["UserToolAuth"];
export type ApiUserToolAuthorization = components["schemas"]["UserToolAuthorization"];
export type ApiCreateUserToolServer = components["schemas"]["CreateUserToolServerRequest"];
export type ApiUpdateUserToolServer = components["schemas"]["UpdateUserToolServerRequest"];
/** What a person's own server is to them now (`status`), and why it is not `ready` (`reason`). */
export type ApiUserToolStatus = components["schemas"]["UserToolServerStatus"];
export type ApiUserToolReason = components["schemas"]["UserToolServerReason"];
/** A person's prompt library (ADR 0070, docs/api/prompts-v1.md): a prompt, its summary and the list of summaries. */
export type ApiPrompt = components["schemas"]["Prompt"];
export type ApiPromptSummary = components["schemas"]["PromptSummary"];
export type ApiPromptList = components["schemas"]["PromptList"];
/** A question of the UI catalog's `Choices`, as a prompt's variable. */
export type ApiChoicesQuestion = components["schemas"]["ChoicesQuestion"];
/** The generated type of `variables` is `Record<string, never>[]`, which no question fits: the questions are written here. */
export type ApiCreatePrompt = Omit<components["schemas"]["CreatePromptRequest"], "variables"> & {
  variables?: ApiChoicesQuestion[];
};
export type ApiUpdatePrompt = Omit<components["schemas"]["UpdatePromptRequest"], "variables"> & {
  variables?: ApiChoicesQuestion[];
};
/** `searchThreads` (ADR 0068, docs/api/thread-search-v1.md): a person's own threads found by their words. */
export type ApiThreadSearchPage = components["schemas"]["ThreadSearchPage"];
export type ApiThreadHit = components["schemas"]["ThreadHit"];
export type ApiSearchHit = components["schemas"]["SearchHit"];
export type ApiSearchSnippet = components["schemas"]["SearchSnippet"];
export type ApiActor = components["schemas"]["Actor"];
/** The share a thread has (`share` of `getThread` and of the list's items): who, and what is served now. */
export type ApiShare = components["schemas"]["ThreadShare"];
/** What the owner is told after sharing, widening, narrowing or making a new link. */
export type ApiShareLink = components["schemas"]["ThreadShareLink"];
/** A shared thread as a reader is given it: no owner, no tools, no share (ADR 0040). */
export type ApiSharedThread = components["schemas"]["SharedThread"];
/** What a deployment lets a person share their threads as: the cap, or `disabled`. */
export type Sharing = NonNullable<ApiMe["sharing"]>;
export type ApiReleases = NonNullable<ApiAgent["releases"]>;
export type ThreadState = components["schemas"]["ThreadState"];

export const THREAD_STATES = [
  "queued",
  "working",
  "verifying",
  "blocked",
  "done",
  "failed",
  "cancelled",
] as const satisfies readonly ThreadState[];
type AllStatesListed =
  Exclude<ThreadState, (typeof THREAD_STATES)[number]> extends never ? true : never;
export const allStatesListed: AllStatesListed = true;

/** Mirrors A2A TaskState: the `status` of a `vymalo.status` activity (docs/api/agui.md). */
export const AGENT_STATUSES = [
  "submitted",
  "working",
  "input_required",
  "auth_required",
  "completed",
  "failed",
  "canceled",
] as const;
export type AgentStatus = (typeof AGENT_STATUSES)[number];

export const TERMINAL_STATES: readonly ThreadState[] = ["done", "failed", "cancelled"];
export const isTerminal = (s: ThreadState | undefined): boolean =>
  s !== undefined && TERMINAL_STATES.includes(s);
/** A run is open while the thread is queued, working or (under a gate) being verified. */
export const isActive = (s: ThreadState | undefined): boolean =>
  s === "queued" || s === "working" || s === "verifying";
