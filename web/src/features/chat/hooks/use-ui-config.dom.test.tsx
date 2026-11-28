// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";

/*
 * `GET /api/config` (ADR 0034): the `ui` section, read once per page load, with the defaults a
 * missing key or an unreadable configuration leaves. Descriptions are not drawn until it is known.
 */

const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
let answer: { status: number; body: unknown } | "network-error" = { status: 200, body: {} };
let asked = 0;
let hook: typeof import("./use-ui-config");

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
  hook = await import("./use-ui-config");
});
afterAll(() => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
});
beforeEach(() => {
  asked = 0;
  hook.resetUiConfig();
});
afterEach(cleanup);

function Probe() {
  const shown = hook.useShowDescriptions();
  return <p>{shown ? "shown" : "hidden"}</p>;
}

function LanguagesProbe() {
  return <p>{hook.useLanguages().join("|")}</p>;
}

describe("languagesOf", () => {
  it("takes an array of interface tags, deduped case-insensitively, and nothing else", () => {
    expect(hook.languagesOf(["en-GB", "fr-FR"])).toEqual(["en-GB", "fr-FR"]);
    // the first spelling wins; a repeat in another case is the same language
    expect(hook.languagesOf(["fr-FR", "FR-fr", "en-GB"])).toEqual(["fr-FR", "en-GB"]);
    for (const bad of [
      undefined,
      null,
      "fr-FR",
      { 0: "fr-FR" },
      [],
      ["fr-FR", 3],
      ["fr-FR", ""],
      ["en_CM"],
      ["en-GB", null],
    ])
      expect(hook.languagesOf(bad), JSON.stringify(bad)).toBeUndefined();
  });
});

describe("useLanguages", () => {
  it("offers the four when the deployment names none (an older orchestrator)", async () => {
    answer = { status: 200, body: { ui: { showDescriptions: true } } };
    render(<LanguagesProbe />);
    await waitFor(() => expect(asked).toBe(1));
    await waitFor(() => expect(screen.getByText("en-GB|fr-FR|gsw-CH|fur-IT")).toBeTruthy());
  });

  it("offers the deployment's own list, in its order", async () => {
    answer = { status: 200, body: { ui: { languages: ["de-DE", "fr-FR"] } } };
    render(<LanguagesProbe />);
    await waitFor(() => expect(screen.getByText("de-DE|fr-FR")).toBeTruthy());
  });

  it("ignores a malformed list and offers the four", async () => {
    answer = { status: 200, body: { ui: { languages: ["fr-FR", 7] } } };
    render(<LanguagesProbe />);
    await waitFor(() => expect(asked).toBe(1));
    await waitFor(() => expect(screen.getByText("en-GB|fr-FR|gsw-CH|fur-IT")).toBeTruthy());
  });
});

describe("useShowDescriptions", () => {
  it("is off until the configuration is known, then follows ui.showDescriptions", async () => {
    answer = { status: 200, body: { ui: { showDescriptions: false } } };
    render(<Probe />);
    expect(screen.getByText("hidden")).toBeTruthy();
    await waitFor(() => expect(asked).toBe(1));
    await new Promise((r) => setTimeout(r, 20));
    expect(screen.getByText("hidden")).toBeTruthy();
  });

  it("shows descriptions when the configuration says so", async () => {
    answer = { status: 200, body: { ui: { showDescriptions: true } } };
    render(<Probe />);
    await waitFor(() => expect(screen.getByText("shown")).toBeTruthy());
  });

  it("a key that is missing is its default (shown), and an unknown key is ignored", async () => {
    answer = { status: 200, body: { ui: { somethingNew: 3 } } };
    render(<Probe />);
    await waitFor(() => expect(screen.getByText("shown")).toBeTruthy());
  });

  it("an unreadable configuration leaves the default, and the next ask tries again", async () => {
    answer = "network-error";
    const first = render(<Probe />);
    await waitFor(() => expect(screen.getByText("shown")).toBeTruthy());
    first.unmount();
    answer = { status: 200, body: { ui: { showDescriptions: false } } };
    render(<Probe />);
    await waitFor(() => expect(asked).toBe(2));
    await waitFor(() => expect(screen.getByText("hidden")).toBeTruthy());
  });

  it("is read once for every component that asks", async () => {
    answer = { status: 200, body: { ui: { showDescriptions: true } } };
    render(
      <>
        <Probe />
        <Probe />
      </>,
    );
    await waitFor(() => expect(screen.getAllByText("shown")).toHaveLength(2));
    expect(asked).toBe(1);
  });
});
