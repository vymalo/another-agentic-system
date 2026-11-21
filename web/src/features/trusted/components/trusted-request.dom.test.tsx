// @vitest-environment jsdom
import { act, cleanup, configure, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiMe, ApiUserToolServer } from "@/lib/api/types";

/*
 * `McpCredential`, the one trusted component this build draws (ADR 0083, ADR 0046), through the real runtime and the
 * real transcript: a `vymalo.trustedRequest` activity draws the client's own card, the person fills it, and the agent
 * gets a **receipt** with no value of the secret. A component this build does not draw, and a person whose roles do not
 * let them add a server, get **words and a link** — never a form (decision 6).
 */
const client = vi.hoisted(() => ({
  GET: vi.fn(),
  POST: vi.fn(),
  PATCH: vi.fn(),
  DELETE: vi.fn(),
}));
vi.mock("@/lib/api/client", async () => ({
  ...(await vi.importActual<typeof import("@/lib/api/client")>("@/lib/api/client")),
  api: client,
}));

const person = vi.hoisted(() => ({ current: { status: "ready", me: null } as unknown }));
vi.mock("@/features/me/hooks/use-me", () => ({ useMe: () => person.current }));

import type { SurfaceHost } from "@/features/chat/components/surface/surface-host";
import { mountSurfaces, resetSeq, stubLayout } from "@/features/chat/components/surface/testing";
import { THREAD_ID } from "@/features/chat/lib/agui/testing";

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(() => {
  resetSeq();
  client.GET.mockReset();
  client.POST.mockReset();
  client.PATCH.mockReset();
  person.current = { status: "ready", me: me() };
  client.GET.mockResolvedValue(listing());
});
afterEach(cleanup);

const SECRET = "tok-9f3a-this-is-a-secret-value";
const NEW_ID = "u-abcdefghij";

function me(permissions: string[] = ["thread.read", "thread.write", "tools.user.add"]): ApiMe {
  return {
    user: "tool-maker@example.com",
    roles: ["user", "tool-maker"],
    permissions: permissions.map((permission) => ({ permission })),
    agents: { read: ["plain"], invoke: ["plain"] },
  } as unknown as ApiMe;
}

const row = (over: Partial<ApiUserToolServer> = {}): ApiUserToolServer => ({
  id: NEW_ID,
  name: "My search",
  own: true,
  status: "ready",
  url: "https://mcp.example.com/mcp",
  auth: { kind: "bearer" },
  state: "connected",
  hasCredential: true,
  revision: "7",
  ...over,
});

const listing = (servers: ApiUserToolServer[] = []) => ({
  data: { servers, limit: 8, kinds: ["none", "bearer", "api_key", "oauth2"] },
  response: { status: 200, ok: true, headers: new Headers() },
});

const answer = (status: number, data: unknown) => ({
  data,
  response: { status, ok: true, headers: new Headers() },
});

const refused = (status: number, error: object) => ({
  error,
  response: { status, ok: false, headers: new Headers() },
});

const ACTOR = { "vymalo.actor": { type: "agent", name: "coder" } };

/** One run of a trusted request, and the thread left waiting for the person (so the form may act). */
function trustedRun(content: Record<string, unknown>, messageId = "trusted-3") {
  let seq = 0;
  const frame = (event: Record<string, unknown>, id?: number) => ({
    event,
    ...(id !== undefined ? { id } : {}),
  });
  return [
    frame({ type: "RUN_STARTED", threadId: THREAD_ID, runId: "run-1", protocolVersion: "1.0" }),
    frame(
      {
        type: "ACTIVITY_SNAPSHOT",
        messageId,
        activityType: "vymalo.trustedRequest",
        replace: false,
        content: { at: "2027-01-15T08:00:00Z", ...content },
        metadata: ACTOR,
      },
      ++seq,
    ),
    frame({
      type: "STATE_SNAPSHOT",
      snapshot: { thread: { state: "blocked", target: { agentId: "plain" }, title: "keys" } },
    }),
    frame(
      {
        type: "RUN_FINISHED",
        threadId: THREAD_ID,
        runId: "run-1",
        outcome: {
          type: "interrupt",
          interrupts: [{ id: "int-1", reason: "input_required", message: "Waiting" }],
        },
      },
      ++seq,
    ),
  ];
}

const CREDENTIAL = {
  component: "McpCredential",
  hints: { name: "My search", scopes: ["search.read", "search.write"], expiresInDays: 30 },
};

/** Mount the transcript, write one trusted request, and wait until the runtime has it. */
async function draw(
  content: Record<string, unknown> = CREDENTIAL,
  host: Partial<SurfaceHost> = {},
) {
  const send = vi.fn();
  const m = mountSurfaces({ send, ...host });
  const frames = trustedRun(content);
  await act(async () => {
    m.stream.frames(frames);
  });
  const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
  await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(last));
  return { ...m, send };
}

const card = () => screen.getByRole("region", { name: "Trusted request from coder" });

describe("a trusted request the client draws", () => {
  it("is the client's own card: the mark, the sentence, and the agent's hints prefilled", async () => {
    await draw();
    const region = card();
    expect(region.getAttribute("data-slot")).toBe("trusted-request");
    expect(region.textContent).toMatch(/The agent can’t see this/);
    // the hints, said as the agent's suggestions
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("My search");
    // the name and the scopes are the agent's suggestions, and the card says whose
    expect(screen.getAllByText(/Suggested by coder/)).toHaveLength(2);
    expect(screen.getByRole("list", { name: "Scopes" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remove search.read" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remove search.write" })).toBeTruthy();
    // the form is the client's, and nothing has been sent
    expect((screen.getByLabelText("Address") as HTMLInputElement).value).toBe("");
    expect(screen.getByLabelText("Token").getAttribute("type")).toBe("password");
    expect(client.POST).not.toHaveBeenCalled();
  });

  it("saves the key through the API and sends a receipt that holds no secret", async () => {
    client.POST.mockResolvedValue(answer(201, row()));
    const { send } = await draw();

    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "https://mcp.example.com/mcp" },
    });
    fireEvent.change(screen.getByLabelText("Token"), { target: { value: SECRET } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });

    await waitFor(() => expect(client.POST).toHaveBeenCalledTimes(1));
    const [path, options] = client.POST.mock.calls[0] ?? [];
    expect(path).toBe("/api/user-tool-servers");
    expect(options.body).toEqual({
      name: "My search",
      url: "https://mcp.example.com/mcp",
      auth: { kind: "bearer" },
      secret: SECRET,
    });

    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    const action = send.mock.calls[0]?.[0] as Record<string, unknown>;
    // the card the person answered, and the receipt: nothing else reaches the agent
    expect(action).toEqual({
      name: "submit",
      surfaceId: "trusted-3",
      sourceComponentId: "McpCredential",
      context: {
        status: "created",
        id: NEW_ID,
        name: "My search",
        scopes: ["search.read", "search.write"],
      },
    });
    expect(JSON.stringify(action)).not.toContain(SECRET);
    // write-only: gone from the card, and in no part of the page
    expect(screen.queryByLabelText("Token")).toBeNull();
    expect(document.body.textContent).not.toContain(SECRET);
    expect(card().textContent).toContain("Saved: My search");
  });

  it("shows a refusal in the problem's words and sends no receipt", async () => {
    client.POST.mockResolvedValue(
      refused(400, { title: "Bad Request", detail: "the address was refused" }),
    );
    const { send } = await draw();

    fireEvent.change(screen.getByLabelText("Address"), {
      target: { value: "https://127.0.0.1/mcp" },
    });
    fireEvent.change(screen.getByLabelText("Token"), { target: { value: SECRET } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save" }));
    });

    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("the address was refused"),
    );
    expect(send).not.toHaveBeenCalled();
    // the secret was cleared the moment it was sent: a refusal means typing it again
    expect((screen.getByLabelText("Token") as HTMLInputElement).value).toBe("");
    expect(document.body.textContent).not.toContain(SECRET);
  });

  it("re-keys one of the person's servers with the secret and its revision, and nothing else", async () => {
    client.GET.mockResolvedValue(
      listing([row({ id: "u-teamsearch", name: "Team search", revision: "4" })]),
    );
    client.PATCH.mockResolvedValue(
      answer(200, row({ id: "u-teamsearch", name: "Team search", revision: "5" })),
    );
    const { send } = await draw();

    await waitFor(() => expect(screen.getByLabelText("Server")).toBeTruthy());
    fireEvent.change(screen.getByLabelText("Server"), { target: { value: "u-teamsearch" } });
    // a re-key asks for the secret alone: no address, no name, no kind
    expect(screen.queryByLabelText("Address")).toBeNull();
    fireEvent.change(screen.getByLabelText("New token"), { target: { value: SECRET } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Save the key" }));
    });

    await waitFor(() => expect(client.PATCH).toHaveBeenCalledTimes(1));
    const [path, options] = client.PATCH.mock.calls[0] ?? [];
    expect(path).toBe("/api/user-tool-servers/{serverId}");
    expect(options.params).toEqual({ path: { serverId: "u-teamsearch" } });
    expect(options.body).toEqual({ secret: SECRET, revision: "4" });
    await waitFor(() => expect(send).toHaveBeenCalledTimes(1));
    const answered = send.mock.calls[0]?.[0] as { context: unknown } | undefined;
    expect(answered?.context).toEqual({
      status: "updated",
      id: "u-teamsearch",
      name: "Team search",
      scopes: ["search.read", "search.write"],
    });
  });
});

describe("a card this build cannot draw", () => {
  it("is words and a link, never a form", async () => {
    await draw({ component: "ApiKeyCreate", hints: {} });
    expect(card().textContent).toContain("ApiKeyCreate");
    expect(card().textContent).toMatch(/this version does not have/);
    const link = screen.getByRole("link", { name: "My tool servers" });
    expect(link.getAttribute("href")).toBe("/tools");
    expect(screen.queryByRole("form")).toBeNull();
    expect(document.querySelector('input[type="password"]')).toBeNull();
  });

  it("is words and a link for a person whose roles do not let them add a server", async () => {
    person.current = { status: "ready", me: me(["thread.read", "thread.write"]) };
    await draw();
    expect(card().textContent).toMatch(/Your roles do not let you add or change tool servers/);
    expect(screen.getByRole("link", { name: "My tool servers" }).getAttribute("href")).toBe(
      "/tools",
    );
    expect(screen.queryByRole("form")).toBeNull();
  });

  it("is words and a link where the deployment keeps no credentials", async () => {
    client.GET.mockResolvedValue({
      data: { servers: [], limit: 8, kinds: ["none"] },
      response: { status: 200, ok: true, headers: new Headers() },
    });
    await draw();
    await waitFor(() => expect(card().textContent).toMatch(/keeps no credentials/));
    expect(screen.queryByRole("form")).toBeNull();
  });
});
