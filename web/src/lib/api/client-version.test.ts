import { afterEach, describe, expect, it, vi } from "vitest";
import {
  CLIENT_VERSION_HEADER,
  isUpdateRequired,
  noteUpdateRequired,
  resetUpdateRequired,
  subscribeUpdateRequired,
  updateRequired,
  withClientVersion,
} from "./client-version";

const problem = (status: number, code?: string) =>
  new Response(JSON.stringify({ title: "x", status, ...(code ? { code } : {}) }), {
    status,
    headers: { "Content-Type": "application/problem+json" },
  });

afterEach(resetUpdateRequired);

describe("withClientVersion", () => {
  it("states the version on a request that does not", () => {
    const sent = withClientVersion(new Request("http://x/api/agents"));
    expect(sent.headers.get(CLIENT_VERSION_HEADER)).toMatch(/^\d+(\.\d+){0,3}$/);
  });
  it("leaves a stated version alone", () => {
    const given = new Request("http://x/api/agents", {
      headers: { [CLIENT_VERSION_HEADER]: "9.9" },
    });
    expect(withClientVersion(given).headers.get(CLIENT_VERSION_HEADER)).toBe("9.9");
  });
});

describe("the update-required problem", () => {
  it("is a 426 that says client_update_required", async () => {
    expect(await isUpdateRequired(problem(426, "client_update_required"))).toBe(true);
  });
  it("is not another 426, another code or another status", async () => {
    expect(await isUpdateRequired(problem(426))).toBe(false);
    expect(await isUpdateRequired(problem(426, "other"))).toBe(false);
    expect(await isUpdateRequired(problem(400, "client_update_required"))).toBe(false);
    expect(await isUpdateRequired(new Response("not json", { status: 426 }))).toBe(false);
  });
  it("tells the page once, and leaves the answer readable", async () => {
    const heard = vi.fn();
    subscribeUpdateRequired(heard);
    const res = problem(426, "client_update_required");
    await noteUpdateRequired(res);
    await noteUpdateRequired(problem(426, "client_update_required"));
    expect(updateRequired()).toBe(true);
    expect(heard).toHaveBeenCalledTimes(1);
    expect((await res.json()).code).toBe("client_update_required");
  });
});
