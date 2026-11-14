// @vitest-environment jsdom
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ApiUserToolServer } from "@/lib/api/types";

/*
 * `/api/user-tool-servers` (ADR 0046) as the page uses it: the list is read when the page opens and again when the
 * window regains focus, every edit sends the revision the row was read at, a refusal is told by its code and a
 * write whose outcome is unknown is followed by a read. The secret is an argument of one call and is in no state.
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

import { useUserToolServers } from "./use-user-tool-servers";

const row = (over: Partial<ApiUserToolServer> = {}): ApiUserToolServer => ({
  id: "u-abcdefghij",
  name: "Search",
  own: true,
  status: "ready",
  url: "https://mcp.example.com/mcp",
  auth: { kind: "bearer" },
  state: "connected",
  hasCredential: true,
  revision: "1",
  ...over,
});

const listing = (servers: ApiUserToolServer[] = [row()], extra: object = {}) => ({
  data: { servers, limit: 8, kinds: ["none", "bearer", "api_key", "oauth2"], ...extra },
  response: { status: 200, ok: true, headers: new Headers() },
});

const refused = (status: number, error: object, headers: Record<string, string> = {}) => ({
  error,
  response: { status, ok: false, headers: new Headers(headers) },
});

const deferred = <T,>() => {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
};

beforeEach(() => {
  client.GET.mockReset().mockResolvedValue(listing());
  client.POST.mockReset();
  client.PATCH.mockReset();
  client.DELETE.mockReset();
});
afterEach(cleanup);

async function ready() {
  const hook = renderHook(() => useUserToolServers());
  await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
  return hook;
}

describe("reading the list", () => {
  it("is loading, then the servers with the limit and the kinds the answer gave", async () => {
    const hook = renderHook(() => useUserToolServers());
    expect(hook.result.current.state.status).toBe("loading");
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
    expect(hook.result.current.state).toMatchObject({
      servers: [{ id: "u-abcdefghij" }],
      limit: 8,
      kinds: ["none", "bearer", "api_key", "oauth2"],
      unreadable: false,
    });
    expect(client.GET).toHaveBeenCalledWith("/api/user-tool-servers");
  });

  it("says a 404 is a deployment that has the feature off, a 403 a person whose roles grant nothing, a 401 a signed-out person", async () => {
    for (const [status, expected] of [
      [404, "off"],
      [403, "no-access"],
      [401, "signed-out"],
    ] as const) {
      client.GET.mockResolvedValueOnce(refused(status, { title: "x", status }));
      const hook = renderHook(() => useUserToolServers());
      await waitFor(() => expect(hook.result.current.state.status).toBe(expected));
      cleanup();
    }
  });

  it("says a failed read in the problem's words and reads again on reload", async () => {
    client.GET.mockResolvedValueOnce(
      refused(503, { title: "x", status: 503, detail: "Storage is unavailable" }),
    );
    const hook = renderHook(() => useUserToolServers());
    await waitFor(() =>
      expect(hook.result.current.state).toMatchObject({
        status: "error",
        message: "Storage is unavailable",
      }),
    );
    act(() => hook.result.current.reload());
    await waitFor(() => expect(hook.result.current.state.status).toBe("ready"));
  });

  it("says a list that cannot be read as such: no servers, and not a failure", async () => {
    client.GET.mockResolvedValueOnce(listing([], { error: { code: "unreadable", detail: "x" } }));
    const hook = await ready();
    expect(hook.result.current.state).toMatchObject({ servers: [], unreadable: true });
  });

  it("reads again when the window regains focus, once for the focus and the visibility change that come together, and keeps what is on screen", async () => {
    const hook = await ready();
    const real = Date.now();
    // not within the quarter second after a read: that one is the same visit
    const clock = vi.spyOn(Date, "now").mockReturnValue(real + 5_000);
    client.GET.mockResolvedValueOnce(
      listing([row({ revision: "2", status: "needs_auth", reason: "reauthorize" })]),
    );
    act(() => {
      window.dispatchEvent(new Event("focus"));
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await waitFor(() =>
      expect(hook.result.current.state).toMatchObject({ servers: [{ revision: "2" }] }),
    );
    expect(client.GET).toHaveBeenCalledTimes(2);
    clock.mockRestore();
  });
});

describe("a list that is already on screen", () => {
  it("stays when a later read fails, and the failure is said beside it; the next good read clears it", async () => {
    const hook = await ready();
    client.GET.mockResolvedValueOnce(
      refused(503, { title: "x", status: 503, detail: "Storage is unavailable" }),
    );
    act(() => hook.result.current.reload());
    await waitFor(() =>
      expect(hook.result.current.state).toMatchObject({
        status: "ready",
        servers: [{ id: "u-abcdefghij" }],
        readError: "Storage is unavailable",
      }),
    );
    // a 404 or a 403 later is a failure beside the list too, not a list replaced by a page that says nothing of it
    client.GET.mockResolvedValueOnce(refused(404, { title: "x", status: 404 }));
    act(() => hook.result.current.reload());
    await waitFor(() => expect(hook.result.current.state).toMatchObject({ status: "ready" }));
    client.GET.mockResolvedValueOnce(listing([row({ revision: "9" })]));
    act(() => hook.result.current.reload());
    await waitFor(() =>
      expect(hook.result.current.state).toMatchObject({
        readError: null,
        servers: [{ revision: "9" }],
      }),
    );
  });

  it("is not overwritten by an older read that answers late", async () => {
    const hook = await ready();
    const slow = deferred<ReturnType<typeof listing>>();
    client.GET.mockReturnValueOnce(slow.promise);
    client.GET.mockResolvedValueOnce(listing([row({ name: "Newer", revision: "8" })]));
    act(() => hook.result.current.reload());
    act(() => hook.result.current.reload());
    await waitFor(() =>
      expect(hook.result.current.state).toMatchObject({ servers: [{ name: "Newer" }] }),
    );
    await act(async () => {
      slow.resolve(listing([row({ name: "Older", revision: "2" })]));
    });
    expect(hook.result.current.state).toMatchObject({
      servers: [{ name: "Newer", revision: "8" }],
    });
  });

  it("is not overwritten by a read that began before a write the person made and answers after it", async () => {
    const hook = await ready();
    const slow = deferred<ReturnType<typeof listing>>();
    client.GET.mockReturnValueOnce(slow.promise);
    act(() => hook.result.current.reload());
    client.PATCH.mockResolvedValueOnce({
      data: row({ name: "Mine", revision: "5" }),
      response: { status: 200, ok: true, headers: new Headers() },
    });
    await act(async () => {
      await hook.result.current.update("u-abcdefghij", { name: "Mine", revision: "1" });
    });
    await act(async () => {
      slow.resolve(listing([row({ name: "Before my change", revision: "1" })]));
    });
    expect(hook.result.current.state).toMatchObject({ servers: [{ name: "Mine", revision: "5" }] });
  });
});

describe("adding", () => {
  it("sends the body, puts the answer in the list sorted by name, and keeps the secret out of the state", async () => {
    const hook = await ready();
    client.POST.mockResolvedValueOnce({
      data: row({ id: "u-aaaaaaaaaa", name: "Alpha", revision: "9" }),
      response: { status: 201, ok: true, headers: new Headers() },
    });
    let result: Awaited<ReturnType<typeof hook.result.current.add>> | undefined;
    await act(async () => {
      result = await hook.result.current.add({
        name: "Alpha",
        url: "https://alpha.example.com/mcp",
        auth: { kind: "bearer" },
        secret: "tok-123456789",
      });
    });
    expect(result?.ok).toBe(true);
    expect(client.POST).toHaveBeenCalledWith("/api/user-tool-servers", {
      body: {
        name: "Alpha",
        url: "https://alpha.example.com/mcp",
        auth: { kind: "bearer" },
        secret: "tok-123456789",
      },
    });
    const state = hook.result.current.state;
    expect(state.status === "ready" && state.servers.map((s) => s.name)).toEqual([
      "Alpha",
      "Search",
    ]);
    expect(JSON.stringify(hook.result.current.state)).not.toContain("tok-123456789");
  });

  it("tells the ninth refusal by its code with the limit, and reads the list again", async () => {
    const hook = await ready();
    client.POST.mockResolvedValueOnce(
      refused(409, { title: "Conflict", status: 409, code: "too_many_servers" }),
    );
    let result: Awaited<ReturnType<typeof hook.result.current.add>> | undefined;
    await act(async () => {
      result = await hook.result.current.add({
        name: "x",
        url: "https://x.example.com/",
        auth: { kind: "none" },
      });
    });
    expect(result).toMatchObject({ ok: false, failure: { message: expect.stringContaining("8") } });
    await waitFor(() => expect(client.GET).toHaveBeenCalledTimes(2));
  });

  it("is a failure in words, not an exception, when the connection is lost, and reads the list again", async () => {
    const hook = await ready();
    client.POST.mockRejectedValueOnce(new TypeError("fetch failed"));
    let result: Awaited<ReturnType<typeof hook.result.current.add>> | undefined;
    await act(async () => {
      result = await hook.result.current.add({
        name: "x",
        url: "https://x.example.com/",
        auth: { kind: "none" },
      });
    });
    expect(result).toMatchObject({ ok: false, failure: { reread: true } });
    await waitFor(() => expect(client.GET).toHaveBeenCalledTimes(2));
  });
});

describe("changing", () => {
  it("sends the revision of the row with the change and takes the answer as the row", async () => {
    const hook = await ready();
    client.PATCH.mockResolvedValueOnce({
      data: row({ name: "Renamed", revision: "2" }),
      response: { status: 200, ok: true, headers: new Headers() },
    });
    await act(async () => {
      await hook.result.current.update("u-abcdefghij", { name: "Renamed", revision: "1" });
    });
    expect(client.PATCH).toHaveBeenCalledWith("/api/user-tool-servers/{serverId}", {
      params: { path: { serverId: "u-abcdefghij" } },
      body: { name: "Renamed", revision: "1" },
    });
    expect(hook.result.current.state).toMatchObject({
      servers: [{ name: "Renamed", revision: "2" }],
    });
  });

  it("tells a stale revision by its code and reads the list again to show where the server stands", async () => {
    const hook = await ready();
    client.PATCH.mockResolvedValueOnce(
      refused(409, { title: "Conflict", status: 409, code: "stale_revision" }),
    );
    client.GET.mockResolvedValueOnce(listing([row({ name: "Other tab", revision: "5" })]));
    let result: Awaited<ReturnType<typeof hook.result.current.update>> | undefined;
    await act(async () => {
      result = await hook.result.current.update("u-abcdefghij", { name: "Mine", revision: "1" });
    });
    expect(result).toMatchObject({
      ok: false,
      failure: { message: expect.stringContaining("changed since you opened") },
    });
    await waitFor(() =>
      expect(hook.result.current.state).toMatchObject({
        servers: [{ name: "Other tab", revision: "5" }],
      }),
    );
  });
});

describe("deleting and authorizing", () => {
  it("deletes by id with no revision, takes the row out, and treats a server that is already gone as deleted", async () => {
    const hook = await ready();
    client.DELETE.mockResolvedValueOnce({
      response: { status: 204, ok: true, headers: new Headers() },
    });
    await act(async () => {
      await hook.result.current.remove("u-abcdefghij");
    });
    expect(client.DELETE).toHaveBeenCalledWith("/api/user-tool-servers/{serverId}", {
      params: { path: { serverId: "u-abcdefghij" } },
    });
    expect(hook.result.current.state).toMatchObject({ servers: [] });
    client.DELETE.mockResolvedValueOnce(refused(404, { title: "Not found", status: 404 }));
    let again: Awaited<ReturnType<typeof hook.result.current.remove>> | undefined;
    await act(async () => {
      again = await hook.result.current.remove("u-abcdefghij");
    });
    expect(again?.ok).toBe(true);
  });

  it("asks where to send the person with no body and gives the answer back for the dialog, which keeps it", async () => {
    const hook = await ready();
    client.POST.mockResolvedValueOnce({
      data: {
        authorizeUrl: "https://auth.example.com/authorize?state=s",
        issuer: "https://auth.example.com",
        expiresAt: "2030-01-01T00:00:00Z",
      },
      response: { status: 200, ok: true, headers: new Headers() },
    });
    let result: Awaited<ReturnType<typeof hook.result.current.authorize>> | undefined;
    await act(async () => {
      result = await hook.result.current.authorize("u-abcdefghij");
    });
    expect(client.POST).toHaveBeenCalledWith("/api/user-tool-servers/{serverId}/authorize", {
      params: { path: { serverId: "u-abcdefghij" } },
    });
    expect(result).toMatchObject({ ok: true, value: { issuer: "https://auth.example.com" } });
    // the URL is a capability for the length of the flow: it is in the answer and in no state of the hook
    expect(JSON.stringify(hook.result.current.state)).not.toContain("authorize?state");
  });

  it("tells a rate limit with the wait the server asked for", async () => {
    const hook = await ready();
    client.POST.mockResolvedValueOnce(
      refused(429, { title: "x", status: 429, code: "rate_limited" }, { "Retry-After": "6" }),
    );
    let result: Awaited<ReturnType<typeof hook.result.current.authorize>> | undefined;
    await act(async () => {
      result = await hook.result.current.authorize("u-abcdefghij");
    });
    expect(result).toMatchObject({
      ok: false,
      failure: { message: expect.stringContaining("6 seconds") },
    });
  });
});
