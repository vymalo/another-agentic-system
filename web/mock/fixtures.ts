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
