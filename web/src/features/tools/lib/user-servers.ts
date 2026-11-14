import type {
  ApiCreateUserToolServer,
  ApiToolServer,
  ApiUpdateUserToolServer,
  ApiUserToolAuth,
  ApiUserToolServer,
} from "@/lib/api/types";

/*
 * The person's own MCP servers (ADR 0046) as questions a screen asks, and the words it says them in. Nothing here
 * is a check: the orchestrator enforces every request, so this offers what it would accept and says what it refused
 * from the status and the `code` of its answer (`docs/api/chat-api.yaml`, the recipe of `listUserToolServers`), never
 * from a message it guessed. A secret is only ever an argument of the two builders below: it is in no state.
 */

export type Kind = ApiUserToolAuth["kind"];

/** The words for the kinds the API answers (`UserToolServers.kinds`); a kind it adds later is its own name. */
export const KIND_LABEL: Record<Kind, string> = {
  none: "No authentication",
  bearer: "Bearer token",
  api_key: "API key in a header",
  oauth2: "OAuth sign-in",
};
export const kindLabel = (kind: string): string => KIND_LABEL[kind as Kind] ?? kind;

/** What each kind asks of the person, in one line under the field that picks it. */
export const KIND_HELP: Record<Kind, string> = {
  none: "The server needs nothing to connect.",
  bearer: "You give a token. It is sent as an Authorization header.",
  api_key: "You give a key and the name of the header it goes in.",
  oauth2: "You sign in at the server's own authorization service once the server is added.",
};

/** A token or a key the person types: only these two kinds take one. */
export const takesSecret = (kind: Kind): boolean => kind === "bearer" || kind === "api_key";

/** The host of an address, and never its path: the path of a server's address can hold a secret. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

/** Scopes as typed: words separated by spaces, commas or lines, once each, in the order given. */
export const scopesOf = (text: string): string[] => [
  ...new Set(text.split(/[\s,]+/).filter((s) => s.length > 0)),
];

// ---- status, in words -------------------------------------------------------------------------

export type Standing = {
  /** The status, in one or two words. */
  label: string;
  /** What it means and what to do, in one sentence. */
  hint: string;
  /** `ok`: offered. `action`: the person has something to do. `blocked`: a cause outside them. */
  tone: "ok" | "action" | "blocked";
};

type Shown = {
  status?: string | undefined;
  reason?: string | undefined;
  /** Absent where the kind is not known (the picker's list does not carry it). */
  auth?: Pick<ApiUserToolAuth, "kind"> | undefined;
};

const needsLabel = (kind: string | undefined) =>
  kind === "oauth2"
    ? "Needs a sign-in"
    : kind === "bearer" || kind === "api_key"
      ? "Needs a key"
      : "Needs a key or a sign-in";

/**
 * What `status` and `reason` say, in words (`UserToolServerStatus`, `UserToolServerReason`). The lists are closed
 * but a client shows a generic line for a code it does not know, so a deployment ahead of this screen reads
 * something true and never nothing.
 */
export function standing({ status, reason, auth }: Shown): Standing {
  const oauth = auth?.kind === "oauth2";
  if (status === "ready" || status === undefined) {
    return { label: "Ready", hint: "Offered in the Tools menu of a chat.", tone: "ok" };
  }
  if (status === "needs_auth") {
    const what = oauth ? "sign-in" : auth ? "key" : "key or sign-in";
    const again = oauth ? "Connect it again." : "Give it a new key.";
    const hints: Record<string, string> = {
      not_connected: oauth
        ? "It is saved but not connected. Connect it to sign in."
        : "It is saved but has no key yet. Give it a key.",
      reauthorize: oauth
        ? `The sign-in ended or was refused. ${again}`
        : `The key was refused or ended. ${again}`,
      disconnected: oauth
        ? `It was disconnected. ${again}`
        : `It was disconnected and its key forgotten. ${again}`,
    };
    return {
      label: needsLabel(auth?.kind),
      hint: hints[reason ?? ""] ?? `It needs a ${what} before a chat can use it.`,
      tone: "action",
    };
  }
  if (status === "unavailable") {
    const hints: Record<string, string> = {
      not_permitted:
        "Your roles no longer let you use servers of your own. You can still delete it.",
      egress_refused:
        "This deployment no longer allows connections to its address. Change the address, or delete it.",
      broker_down:
        "This deployment keeps no credentials, so a server that needs one cannot be used here. Change it to no authentication, or delete it.",
    };
    return {
      label: "Not available",
      hint: hints[reason ?? ""] ?? "It cannot be used right now. Try again later.",
      tone: "blocked",
    };
  }
  return { label: "Not available", hint: "It cannot be used right now.", tone: "blocked" };
}

/**
 * The one thing that moves a server that needs the person: **Connect** for an OAuth server, a new key for one
 * that takes a key, nothing for anything else (a server that is not `needs_auth` needs no action of this kind).
 */
export function nextAction(server: ApiUserToolServer): "connect" | "key" | null {
  if (server.status !== "needs_auth") return null;
  if (server.auth.kind === "oauth2") return "connect";
  return takesSecret(server.auth.kind) ? "key" : null;
}

// ---- the picker ---------------------------------------------------------------------------------

/** One of the person's own servers, in the list of `GET /api/tool-servers` (`own: true`). */
export const isOwn = (server: ApiToolServer): boolean => server.own === true;

/** A deployment's server is offered; a person's own only while it is `ready` (a 422 would refuse it otherwise). */
export const attachable = (server: ApiToolServer): boolean =>
  !isOwn(server) || server.status === "ready";

// ---- the sign-in link --------------------------------------------------------------------------

export type OpenableLink =
  | { ok: false }
  | { ok: true; href: string; host: string; issuerHost: string; sameHost: boolean };

const LOOPBACK = new Set(["localhost", "127.0.0.1", "[::1]"]);

/**
 * Whether a `authorizeUrl` may be opened, and where it goes. The orchestrator takes the authorization endpoint from the
 * server's own RFC 8414 metadata and judges only its shape (`credential-broker/src/discovery.rs`, `found_in`): `https`, a
 * public host, no fragment. It does **not** require the endpoint to be on the issuer's origin, which the RFC allows. So the
 * rule is: parse it; `https:`, or `http:` on this machine (a dev authorization server); no user information; and the person
 * is shown the host of **the link itself**, beside the issuer's when they differ. Anything else (`javascript:`, `data:`, a
 * relative or malformed link) is never opened.
 */
export function openableLink(authorizeUrl: string, issuer: string): OpenableLink {
  let url: URL;
  try {
    url = new URL(authorizeUrl);
  } catch {
    return { ok: false };
  }
  const secure = url.protocol === "https:";
  const local = url.protocol === "http:" && LOOPBACK.has(url.hostname);
  if (!(secure || local) || url.username !== "" || url.password !== "" || url.hostname === "") {
    return { ok: false };
  }
  const issuerHost = hostOf(issuer);
  return {
    ok: true,
    href: url.href,
    host: url.host,
    issuerHost,
    sameHost: url.host === issuerHost,
  };
}

// ---- what is sent -------------------------------------------------------------------------------

/** What the form holds. The secret is the person's typing: the form clears it the moment it is sent. */
export type ServerFields = {
  name: string;
  url: string;
  kind: Kind;
  header: string;
  scopes: string;
  clientId: string;
  secret: string;
};

/** `auth` of a form: the whole object, with what its kind reads and nothing else. */
function authOf(f: ServerFields): ApiUserToolAuth {
  if (f.kind === "api_key") return { kind: "api_key", header: f.header.trim() };
  if (f.kind === "oauth2") {
    const scopes = scopesOf(f.scopes);
    const clientId = f.clientId.trim();
    return {
      kind: "oauth2",
      ...(scopes.length > 0 ? { scopes } : {}),
      ...(clientId ? { clientId } : {}),
    };
  }
  return { kind: f.kind };
}

/** `createUserToolServer`'s body. The secret goes only for a kind that takes one. */
export function buildCreate(f: ServerFields): ApiCreateUserToolServer {
  return {
    name: f.name.trim(),
    url: f.url.trim(),
    auth: authOf(f),
    ...(takesSecret(f.kind) ? { secret: f.secret } : {}),
  };
}

const sameAuth = (a: ApiUserToolAuth, b: ApiUserToolAuth): boolean =>
  a.kind === b.kind &&
  (a.header ?? "") === (b.header ?? "") &&
  (a.clientId ?? "") === (b.clientId ?? "") &&
  [...new Set(a.scopes ?? [])].sort().join(" ") === [...new Set(b.scopes ?? [])].sort().join(" ");

/**
 * `updateUserToolServer`'s body for an edit: only what differs, with the `revision` the person was looking at.
 * `null` when nothing differs (no request is better than one the server answers with the same server). A secret
 * typed counts as a change whatever else did.
 */
export function buildChange(
  was: ApiUserToolServer,
  f: ServerFields,
): ApiUpdateUserToolServer | null {
  const name = f.name.trim();
  const url = f.url.trim();
  const auth = authOf(f);
  const body: ApiUpdateUserToolServer = {};
  if (name !== was.name) body.name = name;
  if (url !== was.url) body.url = url;
  if (!sameAuth(auth, was.auth)) body.auth = auth;
  if (f.secret !== "" && takesSecret(f.kind)) body.secret = f.secret;
  if (Object.keys(body).length === 0) return null;
  return { ...body, revision: was.revision };
}

/**
 * What an edit would forget, said before it is sent: the credential was issued for the address and the kind
 * (and, for OAuth, the scopes and the client), so a change of them ends it and the person is asked again.
 */
export function moveWarning(was: ApiUserToolServer, f: ServerFields): string | null {
  if (!was.hasCredential) return null;
  const moved =
    f.url.trim() !== was.url ||
    f.kind !== was.auth.kind ||
    (f.kind === "oauth2" && !sameAuth(authOf(f), was.auth));
  if (!moved) return null;
  return "Changing the address or how it signs in makes the server forget its current key or sign-in. You will be asked for it again.";
}

// ---- what failed, in words ------------------------------------------------------------------------

export type Failure = {
  /** The field of the form the words are about, where the `code` names one; else none. */
  field?: "url" | "kind";
  message: string;
  /** The outcome is not what the person asked for or not known: the list must be read again. */
  reread: boolean;
};

type Problem = { status: number; code?: string | undefined; detail?: string | undefined };

/**
 * The words for a refused or failed write, from the status and the `code` of the problem. A 400's `detail` is
 * shown because the server never quotes a value in it; every other sentence is ours, from the `code`. A failure
 * whose outcome is unknown (a 5xx, a lost connection) says so and asks for the list to be read again: a write
 * runs to its end on the server whether or not the client waits.
 */
export function failureOf(
  problem: Problem,
  context: { retryAfter: string | null; limit: number },
): Failure {
  const { status, code, detail } = problem;
  const say = (message: string, reread = false, field?: Failure["field"]): Failure => ({
    message,
    reread,
    ...(field ? { field } : {}),
  });
  switch (code) {
    case "stale_revision":
      return say(
        "This server was changed since you opened it, in another tab or by a sign-in that ended. The list was read again: check it, then make your change again.",
        true,
      );
    case "too_many_servers":
      return say(
        `You already have ${context.limit} servers, the most one person can have. Delete one to add another.`,
        true,
      );
    case "url_refused":
      return say(
        `That address was refused. ${detail ?? "This deployment does not allow connections to it."}`,
        false,
        "url",
      );
    case "credentials_not_kept":
      return say(
        "This deployment keeps no credentials: only a server that needs no authentication can be added or changed to.",
        false,
        "kind",
      );
    case "egress_refused":
      return say(
        "This deployment does not allow connections to an address the sign-in needs, so this server cannot be connected.",
      );
    case "oauth_not_configured":
      return say(
        "This deployment cannot sign in to servers yet: it has no address for the sign-in to come back to. Ask whoever runs it.",
      );
    case "rate_limited": {
      const seconds = Number.parseInt(context.retryAfter ?? "", 10);
      return say(
        Number.isFinite(seconds) && seconds > 0
          ? `Too many sign-in attempts. Try again in ${seconds} seconds.`
          : "Too many sign-in attempts. Try again in a moment.",
      );
    }
    case "broker_down":
      return say(
        "The credential could not be kept or removed just now. Try again in a moment; the list was read again to show where the server stands.",
        true,
      );
    case "authorization_unavailable":
      return say(
        "The sign-in could not be started now: the server's authorization service did not answer. Try again in a moment.",
      );
    case "forbidden":
      return say("Your roles do not let you add or change servers of your own.");
    case "no_access":
      return say("Your roles grant no access to this app.");
    default:
  }
  if (status === 404) {
    return say("That server no longer exists, or this deployment turned the feature off.", true);
  }
  if (status === 400) {
    return say(detail || "The server could not accept that. Check what you typed.");
  }
  if (status === 503) {
    return say("The server is being changed by another request. Try again in a moment.", true);
  }
  return say(
    "Something went wrong, and the change may have been made. The list was read again: check where the server stands.",
    true,
  );
}
