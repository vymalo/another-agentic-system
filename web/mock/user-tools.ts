/**
 * The mock's people's own MCP servers (ADR 0046): `listUserToolServers`, `createUserToolServer`, `updateUserToolServer`,
 * `deleteUserToolServer`, `authorizeUserToolServer`, `finishUserToolAuthorization`, the person's servers beside the
 * deployment's in `listToolServers`, and a stand-in for the authorization server's consent page. Each answer is the one
 * `docs/api/chat-api.yaml` documents, and `server.contract.test.ts` validates every one against it.
 *
 * It is a function of its state and the request, with no I/O of its own (`server.ts` reads the body and writes the
 * answer): a person is one session of the mock, as for every other route, so another session's servers are a 404.
 * What it checks is what the orchestrator checks, in the same order (`orchestrator/crates/api/src/user_tools.rs`,
 * `orchestrator/crates/app/src/user_tools.rs`): the permission, then the values, then the server, then the limit.
 * A secret is read to be checked and **never kept**: only whether one is (`hasCredential`) is, as no answer ever
 * holds one. The consent page is a stand-in for the authorization server: `GET /__mock/oauth/approve?state=` is
 * what the person's browser is sent to, and it sends the browser on to the callback as the real one would.
 */
import { randomBytes } from "node:crypto";
import type { components } from "../src/lib/api/schema";

export type UserToolServer = components["schemas"]["UserToolServer"];
type ToolServer = components["schemas"]["ToolServer"];
type Kind = "none" | "bearer" | "api_key" | "oauth2";
type State = UserToolServer["state"];
type Reason = NonNullable<UserToolServer["reason"]>;

/** The most servers one person holds (`UserToolServers.limit`). */
export const USER_TOOL_LIMIT = 8;

/** Where the browser comes back to (`finishUserToolAuthorization`). */
export const CALLBACK_PATH = "/api/user-tool-servers/oauth/callback";

/** How long an authorization lasts (ms): a callback after it finds nothing. */
const FLOW_MS = 10 * 60 * 1000;

/** What a test sets for its session (`POST /__mock/user-tools`). */
export type UserToolsConfig = {
  /** False: the deployment has the feature off, and every route of it is a 404. */
  enabled: boolean;
  /** False: the deployment keeps no credentials, so only `none` is a kind and the rest is a 422. */
  credentials: boolean;
  /** The authorization server a stranger's server chose: what `authorize` says as `issuer`. */
  issuer: string;
  /** Hosts today's egress lists refuse, on top of the private and reserved addresses. */
  denyHosts: string[];
  /** A refusal that `authorize` answers whatever the server, for the screen's wording of each. */
  authorize: null | "egress_refused" | "oauth_not_configured" | "authorization_unavailable";
};

type Stored = { server: UserToolServer };

/** One session's servers and flows. */
export type UserTools = {
  config: UserToolsConfig;
  servers: Stored[];
  /** `state` of an authorization in flight → the server it is for and when it ends. */
  flows: Map<string, { id: string; until: number }>;
  /** The times (ms) of the `authorize` calls of the last minute: ten are allowed. */
  calls: number[];
  counter: number;
};

export const newUserTools = (): UserTools => ({
  config: {
    enabled: true,
    credentials: true,
    issuer: "https://auth.mock-issuer.example",
    denyHosts: [],
    authorize: null,
  },
  servers: [],
  flows: new Map(),
  calls: [],
  counter: 0,
});

/** An answer: the status, the body (a problem, JSON or HTML) and the headers beyond the content type. */
export type Reply = {
  status: number;
  body?: unknown;
  type?: string;
  headers?: Record<string, string>;
};

const problem = (
  status: number,
  title: string,
  detail?: string,
  code?: string,
  headers?: Record<string, string>,
): Reply => ({
  status,
  type: "application/problem+json",
  body: { title, status, ...(detail ? { detail } : {}), ...(code ? { code } : {}) },
  ...(headers ? { headers } : {}),
});

const NOT_FOUND = problem(404, "Not found");
const OFF = problem(
  404,
  "Not found",
  "people's own MCP servers are not enabled in this deployment",
);
const FORBIDDEN = problem(403, "Forbidden", "your roles do not grant tools.user.add", "forbidden");

export const kindsOf = (config: UserToolsConfig): Kind[] =>
  config.credentials ? ["none", "bearer", "api_key", "oauth2"] : ["none"];

// ---- what the guard refuses ---------------------------------------------------------------

const PRIVATE_V4 = [
  /^0\./,
  /^10\./,
  /^127\./,
  /^169\.254\./,
  /^172\.(1[6-9]|2\d|3[01])\./,
  /^192\.168\./,
];

/** Whether the guard refuses `host`: a private, loopback, link-local or reserved address, or one today's lists name. */
function refused(config: UserToolsConfig, host: string): boolean {
  const h = host.toLowerCase();
  return (
    config.denyHosts.some((d) => d.toLowerCase() === h) ||
    h === "localhost" ||
    h.endsWith(".localhost") ||
    h.endsWith(".internal") ||
    h.endsWith(".local") ||
    h.startsWith("[") ||
    PRIVATE_V4.some((re) => re.test(h))
  );
}

const hostOf = (url: string): string => new URL(url).hostname;

// ---- the values -----------------------------------------------------------------------------

const FORBIDDEN_HEADERS = new Set(
  [
    "Authorization",
    "Host",
    "Accept",
    "Content-Type",
    "Last-Event-ID",
    "Content-Length",
    "Transfer-Encoding",
    "Connection",
    "Upgrade",
    "TE",
    "Expect",
    "Keep-Alive",
    "Trailer",
    "Cookie",
  ].map((h) => h.toLowerCase()),
);

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

type Auth = { kind: Kind; header?: string; scopes?: string[]; clientId?: string };

/**
 * `auth` as the API reads it, or the 400's words. The words never quote a value (the real answer says nothing of
 * what was typed, because a secret could be in a field that was typed in the wrong box).
 */
function readAuth(raw: unknown): Auth | string {
  if (!isRecord(raw)) return "`auth` is an object with a `kind`";
  for (const key of Object.keys(raw)) {
    if (!["kind", "header", "scopes", "clientId"].includes(key)) {
      return "`auth` has a member this route does not read";
    }
  }
  const kind = raw.kind;
  if (kind !== "none" && kind !== "bearer" && kind !== "api_key" && kind !== "oauth2") {
    return "`auth.kind` is none, bearer, api_key or oauth2";
  }
  const auth: Auth = { kind };
  if (kind === "api_key") {
    const header = raw.header;
    if (typeof header !== "string" || !/^[A-Za-z0-9-]{1,64}$/.test(header)) {
      return "an `api_key` names the header its key goes in: letters, digits and hyphens, at most 64";
    }
    const lower = header.toLowerCase();
    if (FORBIDDEN_HEADERS.has(lower) || lower.startsWith("mcp-") || lower.startsWith("proxy-")) {
      return "that header is one the orchestrator owns: choose another";
    }
    auth.header = header;
  } else if (raw.header !== undefined) {
    return "`auth.header` is for an `api_key`";
  }
  if (kind === "oauth2") {
    if (raw.scopes !== undefined) {
      const scopes = raw.scopes;
      if (
        !Array.isArray(scopes) ||
        scopes.length > 32 ||
        !scopes.every((s) => typeof s === "string" && s.length >= 1 && s.length <= 128)
      ) {
        return "`auth.scopes` is at most 32 scopes of 1 to 128 characters";
      }
      if (scopes.length > 0) auth.scopes = scopes as string[];
    }
    if (raw.clientId !== undefined) {
      if (
        typeof raw.clientId !== "string" ||
        raw.clientId.length < 1 ||
        raw.clientId.length > 256
      ) {
        return "`auth.clientId` is 1 to 256 characters";
      }
      auth.clientId = raw.clientId;
    }
  } else if (raw.scopes !== undefined || raw.clientId !== undefined) {
    return "`auth.scopes` and `auth.clientId` are for `oauth2`";
  }
  return auth;
}

const readName = (raw: unknown): string | { error: string } => {
  if (typeof raw !== "string") return { error: "`name` is text" };
  const name = raw.trim();
  if (name.length < 1 || name.length > 80 || /[\r\n]/.test(name)) {
    return { error: "`name` is 1 to 80 characters on one line" };
  }
  return name;
};

/** The URL as the API keeps it, a 400 for what is not one, and `url_refused` for what the guard refuses. */
function readUrl(config: UserToolsConfig, raw: unknown): string | Reply {
  const shape = "`url` is `https`, a host, no user information, query or fragment, at most 2 KiB";
  if (typeof raw !== "string" || raw.length > 2048) return problem(400, "Bad Request", shape);
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return problem(400, "Bad Request", shape);
  }
  if (
    url.protocol !== "https:" ||
    url.hostname === "" ||
    url.username !== "" ||
    url.password !== "" ||
    url.search !== "" ||
    url.hash !== ""
  ) {
    return problem(400, "Bad Request", shape);
  }
  if (refused(config, url.hostname)) {
    return problem(
      400,
      "Bad Request",
      "this deployment does not allow connections to that address",
      "url_refused",
    );
  }
  return url.toString();
}

const readSecret = (raw: unknown): true | string => {
  if (typeof raw !== "string") return "`secret` is text";
  // 8 to 4096 printable ASCII characters, none at either end a space
  if (raw.length < 8 || raw.length > 4096 || !/^[\x20-\x7e]+$/.test(raw) || /^ | $/.test(raw)) {
    return "`secret` is 8 to 4096 printable characters with no space at either end";
  }
  return true;
};

const takesSecret = (kind: Kind) => kind === "bearer" || kind === "api_key";

// ---- what a server is to its owner -----------------------------------------------------------

/** The `status` and `reason` the listing shows: the first of the contract's rules that holds. */
function statusOf(
  tools: UserTools,
  can: boolean,
  s: UserToolServer,
): Pick<UserToolServer, "status" | "reason"> {
  const unavailable = (reason: Reason) => ({ status: "unavailable" as const, reason });
  if (!can) return unavailable("not_permitted");
  if (refused(tools.config, hostOf(s.url))) return unavailable("egress_refused");
  if (s.auth.kind !== "none" && !tools.config.credentials) return unavailable("broker_down");
  switch (s.state) {
    case "connected":
      return { status: "ready" };
    case "added":
      return { status: "needs_auth", reason: "not_connected" };
    case "needs_reauth":
      return { status: "needs_auth", reason: "reauthorize" };
    case "revoked":
      return { status: "needs_auth", reason: "disconnected" };
  }
}

/** The server as the listing shows it for this caller (a stored server holds no `reason`: it is derived). */
const shown = (tools: UserTools, can: boolean, s: UserToolServer): UserToolServer => ({
  ...s,
  ...statusOf(tools, can, s),
});

const byName = (a: UserToolServer, b: UserToolServer) =>
  a.name < b.name ? -1 : a.name > b.name ? 1 : a.id < b.id ? -1 : a.id > b.id ? 1 : 0;

/** The caller's servers, sorted by name and then id, with the status each has for them now. */
export function ownServers(tools: UserTools, can: boolean): UserToolServer[] {
  return tools.servers.map((s) => shown(tools, can, s.server)).sort(byName);
}

/** The person's own servers as `listToolServers` gives them, after the deployment's. */
export function ownToolServers(tools: UserTools, can: boolean): ToolServer[] {
  if (!tools.config.enabled) return [];
  return ownServers(tools, can).map((s) => ({
    id: s.id,
    name: s.name,
    own: true,
    status: s.status,
    ...(s.reason ? { reason: s.reason } : {}),
  }));
}

/**
 * Why `id` cannot be attached to a thread, as the 422 `putThreadTools` answers, or undefined: a server of the
 * person's that is not `ready`. An id that is not one of theirs is the caller's ("not offered").
 */
export function notReady(tools: UserTools, can: boolean, id: string): Reply | undefined {
  if (!tools.config.enabled) return undefined;
  const own = ownServers(tools, can).find((s) => s.id === id);
  if (!own || own.status === "ready") return undefined;
  const code = own.status === "needs_auth" ? "tool_server_needs_auth" : "tool_server_unavailable";
  const why: Record<Reason, string> = {
    not_connected: "it has no key or authorization yet",
    reauthorize: "its credential ended",
    disconnected: "it was disconnected",
    not_permitted: "your roles do not let you use servers of your own",
    egress_refused: "this deployment does not allow connections to it",
    broker_down: "this deployment keeps no credentials",
  };
  return problem(
    422,
    "Unprocessable",
    `the server \`${id}\` cannot be attached now: ${why[own.reason ?? "not_connected"]}`,
    code,
  );
}

// ---- the routes ---------------------------------------------------------------------------------

const newId = (tools: UserTools): string => {
  for (;;) {
    const id = `u-${randomBytes(8)
      .toString("hex")
      .slice(0, 10)
      .replace(/[^a-z0-9]/g, "a")}`;
    if (!tools.servers.some((s) => s.server.id === id)) return id;
  }
};

const nextRevision = (tools: UserTools): string => String(++tools.counter);

const KEYS_CREATE = ["name", "url", "auth", "secret"];
const KEYS_UPDATE = ["name", "url", "auth", "secret", "revision"];

/** What every route but the callback and the delete needs of the caller: the permission to add or change. */
const needsPermission = (can: boolean): Reply | undefined => (can ? undefined : FORBIDDEN);

export function listUserToolServers(tools: UserTools, can: boolean): Reply {
  if (!tools.config.enabled) return OFF;
  return {
    status: 200,
    body: {
      servers: ownServers(tools, can),
      limit: USER_TOOL_LIMIT,
      kinds: kindsOf(tools.config),
    },
  };
}

export function createUserToolServer(tools: UserTools, can: boolean, body: unknown): Reply {
  if (!tools.config.enabled) return OFF;
  const denied = needsPermission(can);
  if (denied) return denied;
  if (!isRecord(body) || Object.keys(body).some((k) => !KEYS_CREATE.includes(k))) {
    return problem(
      400,
      "Bad Request",
      "the body is an object of `name`, `url`, `auth` and `secret`",
    );
  }
  const name = readName(body.name);
  if (typeof name !== "string") return problem(400, "Bad Request", name.error);
  const url = readUrl(tools.config, body.url);
  if (typeof url !== "string") return url;
  const auth = readAuth(body.auth);
  if (typeof auth === "string") return problem(400, "Bad Request", auth);
  if (takesSecret(auth.kind)) {
    if (body.secret === undefined) {
      return problem(400, "Bad Request", "this kind of authentication needs a `secret`");
    }
    const ok = readSecret(body.secret);
    if (ok !== true) return problem(400, "Bad Request", ok);
  } else if (body.secret !== undefined) {
    return problem(400, "Bad Request", "`secret` is for `bearer` and `api_key` only");
  }
  if (!tools.config.credentials && auth.kind !== "none") {
    return problem(
      422,
      "Unprocessable",
      "this deployment keeps no credentials: only a server that wants no authentication can be added or changed to",
      "credentials_not_kept",
    );
  }
  if (tools.servers.length >= USER_TOOL_LIMIT) {
    return problem(
      409,
      "Conflict",
      `a person holds at most ${USER_TOOL_LIMIT} servers of their own; delete one first`,
      "too_many_servers",
    );
  }
  const connected = auth.kind === "none" || takesSecret(auth.kind);
  const server: UserToolServer = {
    id: newId(tools),
    name,
    own: true,
    status: "ready",
    url,
    auth,
    state: connected ? "connected" : "added",
    hasCredential: takesSecret(auth.kind),
    revision: nextRevision(tools),
  };
  tools.servers.push({ server });
  return { status: 201, body: shown(tools, can, server) };
}

const find = (tools: UserTools, id: string): Stored | undefined =>
  tools.servers.find((s) => s.server.id === id);

const sameScopes = (a: string[] = [], b: string[] = []) =>
  new Set(a).size === new Set(b).size && a.every((s) => b.includes(s));

export function updateUserToolServer(
  tools: UserTools,
  can: boolean,
  id: string,
  body: unknown,
): Reply {
  if (!tools.config.enabled) return OFF;
  const denied = needsPermission(can);
  if (denied) return denied;
  if (!isRecord(body) || Object.keys(body).some((k) => !KEYS_UPDATE.includes(k))) {
    return problem(
      400,
      "Bad Request",
      "the body is an object of `name`, `url`, `auth`, `secret` and `revision`",
    );
  }
  if (!KEYS_CREATE.some((k) => body[k] !== undefined)) {
    return problem(400, "Bad Request", "name at least one of `name`, `url`, `auth` and `secret`");
  }
  const name = body.name === undefined ? undefined : readName(body.name);
  if (name !== undefined && typeof name !== "string")
    return problem(400, "Bad Request", name.error);
  const url = body.url === undefined ? undefined : readUrl(tools.config, body.url);
  if (url !== undefined && typeof url !== "string") return url;
  const auth = body.auth === undefined ? undefined : readAuth(body.auth);
  if (typeof auth === "string") return problem(400, "Bad Request", auth);
  if (body.secret !== undefined) {
    const ok = readSecret(body.secret);
    if (ok !== true) return problem(400, "Bad Request", ok);
  }
  if (
    body.revision !== undefined &&
    (typeof body.revision !== "string" || !/^[0-9]+$/.test(body.revision))
  ) {
    return problem(400, "Bad Request", "`revision` is the one the list gave");
  }
  const stored = find(tools, id);
  if (!stored) return NOT_FOUND;
  const was = stored.server;
  if (body.revision !== undefined && body.revision !== was.revision) {
    return problem(
      409,
      "Conflict",
      "the server changed since it was read; read it again",
      "stale_revision",
    );
  }
  const next = { url: url ?? was.url, auth: (auth ?? was.auth) as Auth };
  if (body.secret !== undefined && !takesSecret(next.auth.kind)) {
    return problem(400, "Bad Request", "`secret` is for `bearer` and `api_key` only");
  }
  const moved =
    next.url !== was.url ||
    next.auth.kind !== was.auth.kind ||
    (next.auth.kind === "oauth2" &&
      (!sameScopes(next.auth.scopes, (was.auth as Auth).scopes) ||
        next.auth.clientId !== (was.auth as Auth).clientId));
  const needsBroker =
    (moved && was.hasCredential) ||
    (next.auth.kind !== "none" && (moved || body.secret !== undefined));
  if (!tools.config.credentials && needsBroker) {
    return problem(
      422,
      "Unprocessable",
      "this deployment keeps no credentials: only a server that wants no authentication can be added or changed to",
      "credentials_not_kept",
    );
  }
  const renamed = name !== undefined && name !== was.name;
  const reheadered = JSON.stringify(next.auth) !== JSON.stringify(was.auth);
  if (!moved && !renamed && !reheadered && body.secret === undefined) {
    return { status: 200, body: shown(tools, can, was) };
  }
  let state: State = was.state;
  let hasCredential = was.hasCredential;
  if (moved) {
    state = next.auth.kind === "none" ? "connected" : "added";
    hasCredential = false;
  }
  if (body.secret !== undefined) {
    state = "connected";
    hasCredential = true;
  }
  const server: UserToolServer = {
    ...was,
    name: name ?? was.name,
    url: next.url,
    auth: next.auth,
    state,
    hasCredential,
    revision: nextRevision(tools),
  };
  stored.server = server;
  // a move ends a flow that was begun for the definition it was begun with
  if (moved) for (const [s, flow] of tools.flows) if (flow.id === id) tools.flows.delete(s);
  return { status: 200, body: shown(tools, can, server) };
}

/** Needs no permission: a person whose permission was taken away can still remove what they added. */
export function deleteUserToolServer(tools: UserTools, id: string): Reply {
  if (!tools.config.enabled) return OFF;
  const at = tools.servers.findIndex((s) => s.server.id === id);
  if (at < 0) return NOT_FOUND;
  tools.servers.splice(at, 1);
  for (const [s, flow] of tools.flows) if (flow.id === id) tools.flows.delete(s);
  return { status: 204 };
}

/** `now` and `origin` are the caller's: the mock's own address is where the consent page is. */
export function authorizeUserToolServer(
  tools: UserTools,
  can: boolean,
  id: string,
  origin: string,
  now: number,
): Reply {
  if (!tools.config.enabled) return OFF;
  const denied = needsPermission(can);
  if (denied) return denied;
  // ten calls a minute: every call counts, a refused one too
  tools.calls = tools.calls.filter((t) => now - t < 60_000);
  tools.calls.push(now);
  if (tools.calls.length > 10) {
    return problem(
      429,
      "Too Many Requests",
      "too many authorization attempts; try again in a moment",
      "rate_limited",
      { "Retry-After": "6" },
    );
  }
  const stored = find(tools, id);
  if (stored?.server.auth.kind !== "oauth2") return NOT_FOUND;
  if (!tools.config.credentials) {
    return problem(
      422,
      "Unprocessable",
      "this deployment keeps no credentials, so a server cannot be authorized",
      "credentials_not_kept",
    );
  }
  const refusal = tools.config.authorize;
  if (refusal === "oauth_not_configured") {
    return problem(
      422,
      "Unprocessable",
      "this deployment has no address for the OAuth callback, so a server cannot be authorized",
      "oauth_not_configured",
    );
  }
  if (refusal === "egress_refused" || refused(tools.config, hostOf(stored.server.url))) {
    return problem(
      422,
      "Unprocessable",
      "this deployment does not allow connections to that address",
      "egress_refused",
    );
  }
  if (refusal === "authorization_unavailable") {
    return problem(
      503,
      "Service Unavailable",
      "the authorization could not be started now; try again",
      "authorization_unavailable",
      { "Retry-After": "5" },
    );
  }
  // a later call for the same server ends the earlier one
  for (const [s, flow] of tools.flows) if (flow.id === id) tools.flows.delete(s);
  const state = randomBytes(18).toString("base64url");
  const until = now + FLOW_MS;
  tools.flows.set(state, { id, until });
  return {
    status: 200,
    body: {
      authorizeUrl: `${origin}/__mock/oauth/approve?state=${state}`,
      issuer: tools.config.issuer,
      expiresAt: new Date(until).toISOString(),
    },
  };
}

// ---- the callback -----------------------------------------------------------------------------------

const CALLBACK_HEADERS: Record<string, string> = {
  "Content-Security-Policy":
    "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
  "Referrer-Policy": "no-referrer",
  "Cache-Control": "no-store",
  "X-Content-Type-Options": "nosniff",
  "X-Frame-Options": "DENY",
  "X-Robots-Tag": "noindex, nofollow",
};

/** A page of fixed words: nothing of the request is in it. */
const page = (
  status: number,
  title: string,
  text: string,
  extra?: Record<string, string>,
): Reply => ({
  status,
  type: "text/html; charset=utf-8",
  body: `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>${title}</title></head><body><h1>${title}</h1><p>${text}</p></body></html>`,
  headers: { ...CALLBACK_HEADERS, ...extra },
});

const INVALID = page(
  400,
  "This link is not valid",
  "It may have been used already or have ended. Go back to the app and connect the server again.",
);
const NOT_COMPLETED = page(
  400,
  "The authorization was not completed",
  "Go back to the app and connect the server again.",
);

/**
 * `finishUserToolAuthorization`: outside the identity layer, the person found by the `state` alone, taken once. `flows`
 * is every session's, since the browser that comes back carries no session. Returns the page, and what it did.
 */
export function finishAuthorization(
  find: (state: string) => { tools: UserTools; flow: { id: string; until: number } } | undefined,
  method: string,
  params: URLSearchParams,
  now: number,
): Reply {
  if (method !== "GET") {
    return { status: 405, type: "text/plain", body: "", headers: { Allow: "GET" } };
  }
  for (const key of ["code", "state", "iss", "error"]) {
    if (params.getAll(key).length > 1) return INVALID;
  }
  const state = params.get("state");
  if (!state) return INVALID;
  const found = find(state);
  if (!found?.tools.config.enabled) return INVALID;
  // the state is spent from the first callback that finds it, whatever the outcome
  found.tools.flows.delete(state);
  if (now >= found.flow.until) return INVALID;
  const stored = found.tools.servers.find((s) => s.server.id === found.flow.id);
  if (stored?.server.auth.kind !== "oauth2") return INVALID;
  if (params.has("error") || !params.get("code")) return NOT_COMPLETED;
  stored.server = {
    ...stored.server,
    state: "connected",
    hasCredential: true,
    revision: nextRevision(found.tools),
  };
  return page(200, "The server is connected", "You can close this window and return to the app.");
}

// ---- what a test sets --------------------------------------------------------------------------------

const AUTHORIZE_REFUSALS = ["egress_refused", "oauth_not_configured", "authorization_unavailable"];

/**
 * `POST /__mock/user-tools`: any of `enabled`, `credentials`, `issuer`, `denyHosts`, `authorize` (a refusal, or `null`)
 * and `mark` (`{name, state}`: a server whose credential ended or was revoked, which only the broker can say). The 400's
 * words, or undefined when the body was taken.
 */
export function configure(tools: UserTools, body: unknown): string | undefined {
  if (!isRecord(body)) return "the body is an object";
  const known = ["enabled", "credentials", "issuer", "denyHosts", "authorize", "mark"];
  if (Object.keys(body).some((k) => !known.includes(k)))
    return `the members are ${known.join(", ")}`;
  if (body.enabled !== undefined && typeof body.enabled !== "boolean")
    return "enabled is true or false";
  if (body.credentials !== undefined && typeof body.credentials !== "boolean") {
    return "credentials is true or false";
  }
  if (
    body.issuer !== undefined &&
    (typeof body.issuer !== "string" || !/^https:\/\/[^/?#]+$/.test(body.issuer))
  ) {
    return "issuer is an https origin";
  }
  if (
    body.denyHosts !== undefined &&
    (!Array.isArray(body.denyHosts) || !body.denyHosts.every((h) => typeof h === "string"))
  ) {
    return "denyHosts is a list of host names";
  }
  if (
    body.authorize !== undefined &&
    body.authorize !== null &&
    !AUTHORIZE_REFUSALS.includes(body.authorize as string)
  ) {
    return `authorize is null or one of ${AUTHORIZE_REFUSALS.join(", ")}`;
  }
  let mark: { name: string; state: State } | undefined;
  if (body.mark !== undefined) {
    const m = body.mark;
    if (
      !isRecord(m) ||
      typeof m.name !== "string" ||
      !["connected", "needs_reauth", "revoked", "added"].includes(m.state as string)
    ) {
      return "mark is {name, state: connected, needs_reauth, revoked or added}";
    }
    mark = { name: m.name, state: m.state as State };
    if (!tools.servers.some((s) => s.server.name === mark?.name)) return "mark names no server";
  }
  const c = tools.config;
  if (body.enabled !== undefined) c.enabled = body.enabled as boolean;
  if (body.credentials !== undefined) c.credentials = body.credentials as boolean;
  if (body.issuer !== undefined) c.issuer = body.issuer as string;
  if (body.denyHosts !== undefined) c.denyHosts = body.denyHosts as string[];
  if (body.authorize !== undefined) c.authorize = body.authorize as UserToolsConfig["authorize"];
  if (mark) {
    for (const s of tools.servers) {
      if (s.server.name === mark.name) {
        s.server = {
          ...s.server,
          state: mark.state,
          // a grant that ended is still kept (until the person deletes or re-keys it); one that was not given or was revoked is not
          hasCredential: mark.state === "connected" || mark.state === "needs_reauth",
          revision: nextRevision(tools),
        };
      }
    }
  }
  return undefined;
}

/** Where the consent page sends the browser: the callback, with what the authorization server would add. */
export function approvalRedirect(state: string, issuer: string, decline: boolean): string {
  const query = decline
    ? new URLSearchParams({ error: "access_denied", state })
    : new URLSearchParams({ code: "mock-code", state, iss: issuer });
  return `${CALLBACK_PATH}?${query}`;
}
