// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";

/*
 * `GET /api/me` (ADR 0033): read once per page load for every component that asks, and an identity
 * that cannot be read is "unknown", which hides nothing.
 */

const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
let answer: { status: number; body: unknown } | "network-error" = { status: 200, body: {} };
let asked = 0;
let hook: typeof import("./use-me");

const ME = {
  user: "dev@example.com",
  roles: ["user"],
  permissions: [{ permission: "agent.read" }],
  agents: { read: ["*"], invoke: [] },
};

beforeAll(async () => {
  globalThis.Request = class extends RealRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(typeof input === "string" && input.startsWith("/") ? `http://x${input}` : input, init);
    }
  } as typeof Request;
  globalThis.fetch = (async () => {
    asked += 1;
    if (answer === "network-error") throw new TypeError("network down");
    return new Response(JSON.stringify(answer.body), {
      status: answer.status,
      headers: { "content-type": "application/json" },
    });
  }) as typeof fetch;
  hook = await import("./use-me");
});
afterAll(() => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
});
beforeEach(() => {
  asked = 0;
  hook.resetMe();
});
afterEach(cleanup);

function Probe() {
  const { status, me } = hook.useMe();
  return (
    <p>
      {status}:{me?.user ?? "-"}
    </p>
  );
}

describe("useMe", () => {
  it("is loading until the answer, then the person", async () => {
    answer = { status: 200, body: ME };
    render(<Probe />);
    expect(screen.getByText("loading:-")).toBeTruthy();
    await waitFor(() => expect(screen.getByText("ready:dev@example.com")).toBeTruthy());
  });

  it("is read once for every component that asks", async () => {
    answer = { status: 200, body: ME };
    render(
      <>
        <Probe />
        <Probe />
      </>,
    );
    await waitFor(() => expect(screen.getAllByText("ready:dev@example.com")).toHaveLength(2));
    expect(asked).toBe(1);
  });

  it("a person who is granted nothing is still a person: empty grants are the answer", async () => {
    answer = {
      status: 200,
      body: { ...ME, roles: [], permissions: [], agents: { read: [], invoke: [] } },
    };
    render(<Probe />);
    await waitFor(() => expect(screen.getByText("ready:dev@example.com")).toBeTruthy());
  });

  it("an answer that is not an identity, a 401, a 404 and a network error are all unknown", async () => {
    for (const next of [
      { status: 200, body: { hello: "world" } },
      { status: 401, body: { title: "Unauthorized", status: 401 } },
      { status: 404, body: { title: "Not found", status: 404 } },
      "network-error" as const,
    ]) {
      hook.resetMe();
      answer = next;
      const view = render(<Probe />);
      await waitFor(() => expect(screen.getByText("unknown:-")).toBeTruthy());
      view.unmount();
    }
  });

  it("a read that failed is tried again by the next component that asks", async () => {
    answer = "network-error";
    const first = render(<Probe />);
    await waitFor(() => expect(screen.getByText("unknown:-")).toBeTruthy());
    first.unmount();
    answer = { status: 200, body: ME };
    render(<Probe />);
    await waitFor(() => expect(screen.getByText("ready:dev@example.com")).toBeTruthy());
    expect(asked).toBe(2);
  });
});
