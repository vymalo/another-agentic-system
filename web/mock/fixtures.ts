import type { components } from "../src/lib/api/schema";

type Agent = components["schemas"]["Agent"];

export const AGENTS: readonly Agent[] = [
  {
    id: "coder",
    source: "static",
    name: "Coder",
    description: "Implements a change and opens a pull request.",
    cardUrl: "http://coder.agents.svc/.well-known/agent-card.json",
    releases: {
      defaultChannel: "production",
      channels: { production: "coder-r47", staging: "coder-r51" },
      revisions: ["coder-r53", "coder-r51", "coder-r47"],
    },
  },
  {
    id: "reviewer",
    source: "static",
    name: "Reviewer",
    description: "Reviews a pull request and reports findings.",
    cardUrl: "http://reviewer.agents.svc/.well-known/agent-card.json",
  },
  {
    // the verifier of the `verify-reviewed*` scenarios (ADR 0018): a configured agent like any other
    id: "verifier",
    source: "static",
    name: "Verifier",
    description: "Checks the commit an agent pushed and answers with a verdict.",
    cardUrl: "http://verifier.agents.svc/.well-known/agent-card.json",
  },
];

export const DEV_USER = "dev@example.com";

/**
 * What the platform's registry says when the registry is read (ADR 0022): the detail of a source
 * that could not be.
 */
export const REGISTRY_UNREACHABLE = "the registry could not be reached";

type Me = components["schemas"]["Me"];

export type ProfileName = "user" | "admin" | "read-only" | "limited" | "no-access";

/**
 * Who a session is (`GET /api/me`, ADR 0033), switched by `POST /__mock/config?me=<profile>`. The
 * default is `user`, which is what every session was before roles: all permissions, over the
 * person's own threads, so nothing that was written for the mock without roles changes.
 *
 * - `user`: `dev@example.com`, the built-in `user` role.
 * - `admin`: reads every thread (`thread.read` of scope `any`) and changes only their own; lists
 *   everyone's with `?owner=*`.
 * - `read-only`: a role that reads its own threads and does not write, nor start an agent.
 * - `limited`: the `user` role, but of the agents it may invoke only the reviewer (it reads all).
 * - `no-access`: a valid identity whose roles grant nothing; every route but `/api/me` is a 403.
 */
export const PROFILES: Record<ProfileName, Me> = {
  user: {
    user: DEV_USER,
    email: DEV_USER,
    name: "Dev Example",
    roles: ["user"],
    permissions: [
      { permission: "agent.read" },
      { permission: "agent.invoke" },
      { permission: "thread.read", scope: "own" },
      { permission: "thread.write", scope: "own" },
      { permission: "artifact.read", scope: "own" },
    ],
    agents: { read: ["*"], invoke: ["*"] },
  },
  admin: {
    user: "admin@example.com",
    email: "admin@example.com",
    name: "Ada Admin",
    roles: ["admin"],
    permissions: [
      { permission: "agent.read" },
      { permission: "agent.invoke" },
      { permission: "thread.read", scope: "any" },
      { permission: "thread.write", scope: "own" },
      { permission: "artifact.read", scope: "any" },
      { permission: "admin" },
    ],
    agents: { read: ["*"], invoke: ["*"] },
  },
  "read-only": {
    user: "viewer@example.com",
    email: "viewer@example.com",
    name: "Vera Viewer",
    roles: ["viewer"],
    permissions: [
      { permission: "agent.read" },
      { permission: "thread.read", scope: "own" },
      { permission: "artifact.read", scope: "own" },
    ],
    agents: { read: ["*"], invoke: [] },
  },
  limited: {
    user: "limited@example.com",
    email: "limited@example.com",
    name: "Lena Limited",
    roles: ["reviewer-user"],
    permissions: [
      { permission: "agent.read" },
      { permission: "agent.invoke" },
      { permission: "thread.read", scope: "own" },
      { permission: "thread.write", scope: "own" },
      { permission: "artifact.read", scope: "own" },
    ],
    agents: { read: ["*"], invoke: ["reviewer"] },
  },
  "no-access": {
    user: "nobody@example.com",
    email: "nobody@example.com",
    name: "Nina Nobody",
    roles: [],
    permissions: [],
    agents: { read: [], invoke: [] },
  },
};
export const isProfileName = (v: string | null): v is ProfileName =>
  v !== null && Object.hasOwn(PROFILES, v);

type ToolServer = components["schemas"]["ToolServer"];

/** What a server's icon is made of: a magnifier, as the SVG a deployment would put in its configuration. */
const SEARCH_ICON_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle cx="7" cy="7" r="4.5" fill="none" stroke="#2f6f4f" stroke-width="2"/><path d="M10.5 10.5 14 14" stroke="#2f6f4f" stroke-width="2" stroke-linecap="round"/></svg>';

/** A 16 by 16 PNG (a teal page with three lines), as the second kind of icon a configuration may carry. */
const DOCS_ICON_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAYAAAAf8/9hAAAAKklEQVR42mNgAAL50OD/5GAGmGb9hlqyMNiQYWYAsWDUC6NeoL4BlGZnANSjQtmyrOTxAAAAAElFTkSuQmCC";

/**
 * The MCP servers the mock deployment offers for attaching (`GET /api/tool-servers`, ADR 0024), in
 * its order: one with an SVG icon for every agent, one with no icon (the generic one is drawn) for
 * the coder only, and one with a PNG for the coder and the reviewer. A test swaps the list for its
 * own session with `POST /__mock/tool-servers`.
 */
export const TOOL_SERVERS: readonly ToolServer[] = [
  {
    id: "websearch",
    name: "Web search",
    description: "Search the web and read what comes back.",
    icon: `data:image/svg+xml;base64,${Buffer.from(SEARCH_ICON_SVG).toString("base64")}`,
  },
  {
    id: "github",
    name: "GitHub",
    description: "Read repositories, issues and pull requests.",
    agents: ["coder"],
  },
  {
    id: "docs",
    name: "Team docs",
    description: "Look things up in the team's documentation.",
    icon: `data:image/png;base64,${DOCS_ICON_PNG}`,
    agents: ["coder", "reviewer"],
  },
];

/**
 * The agents whose card lists `thread-tools/v1` (the capabilities document says it in `custom`, so a
 * client can flag an agent before it sends): the others are sent no tools and the web says so.
 */
export const THREAD_TOOLS_AGENTS: ReadonlySet<string> = new Set(["coder"]);
export const THREAD_TOOLS_URI = "https://agents.vymalo.com/a2a/extensions/thread-tools/v1";

/**
 * The agents whose card lists `steer/v1` (ADR 0036), so the capabilities document says it in `custom`
 * and the web words its Send menu "reads it at its next step"; the others say "after this turn". The
 * mock does not play the extension: a steered message reaches the agent after its turn for every
 * agent (`Run.held`), which is also what the orchestrator does until the dispatcher steers.
 */
export const STEER_AGENTS: ReadonlySet<string> = new Set(["coder"]);
export const STEER_URI = "https://agents.vymalo.com/a2a/extensions/steer/v1";
