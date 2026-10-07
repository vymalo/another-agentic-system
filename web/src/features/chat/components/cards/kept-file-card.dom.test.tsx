// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { KeptFile } from "@/features/chat/lib/files";
import { setBrowserAuth } from "@/lib/auth/config";

/*
 * A kept file where the web holds its own tokens (ADR 0054, decision 9): a link cannot carry a
 * header, so the file is fetched and shown from an object URL that is revoked with the component.
 * An edge deployment and a public share link keep their plain links.
 */
const fetched = vi.hoisted(() => ({ apiFetch: vi.fn() }));
vi.mock("@/lib/api/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api/client")>()),
  apiFetch: fetched.apiFetch,
}));

import { SourcesView } from "@/features/panel/components/sources-tab";
import { KeptFileCard } from "./kept-file-card";

const SHA = "a".repeat(64);
const ROUTE = `/api/threads/t-1/artifacts/${SHA}`;
const image: KeptFile = {
  sha256: SHA,
  href: ROUTE,
  size: 2048,
  filename: "chart.png",
  name: "chart",
  mimeType: "image/png",
  preview: "image",
};

const created: string[] = [];
const revoked: string[] = [];
let counter = 0;
beforeEach(() => {
  counter = 0;
  created.length = 0;
  revoked.length = 0;
  URL.createObjectURL = () => {
    const url = `blob:http://app.test/${++counter}`;
    created.push(url);
    return url;
  };
  URL.revokeObjectURL = (url: string) => void revoked.push(url);
  fetched.apiFetch.mockReset();
  fetched.apiFetch.mockImplementation(
    async () =>
      new Response(new Uint8Array([137, 80, 78, 71]), {
        status: 200,
        headers: { "Content-Type": "image/png" },
      }),
  );
  setBrowserAuth({ issuer: "https://id.example", clientId: "web", scope: "openid" });
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("an image in a deployment where the web holds its tokens", () => {
  it("is fetched with the session and shown from an object URL, never from the route", async () => {
    render(<KeptFileCard file={image} />);
    const img = await screen.findByRole("img", { name: "chart.png" });
    expect(img.getAttribute("src")).toBe("blob:http://app.test/1");
    expect(fetched.apiFetch).toHaveBeenCalledWith(
      ROUTE,
      expect.objectContaining({ credentials: "same-origin" }),
    );
    expect(document.querySelector(`[src="${ROUTE}"], [href="${ROUTE}"]`)).toBeNull();
  });

  it("revokes the object URL when the card goes", async () => {
    const { unmount } = render(<KeptFileCard file={image} />);
    await screen.findByRole("img", { name: "chart.png" });
    unmount();
    expect(revoked).toEqual(["blob:http://app.test/1"]);
  });

  it("says so when the file could not be fetched, and keeps the download", async () => {
    fetched.apiFetch.mockResolvedValue(new Response("no", { status: 404 }));
    render(<KeptFileCard file={image} />);
    await waitFor(() =>
      expect(document.querySelector('[data-slot="file-image-error"]')).not.toBeNull(),
    );
    expect(screen.getByRole("button", { name: "Download chart.png" })).toBeTruthy();
  });

  it("downloads by fetching the attachment and saving it: no link to the route, no navigation to a blob", async () => {
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(function (
      this: HTMLAnchorElement,
    ) {
      expect(this.getAttribute("download")).toBe("chart.png");
      expect(this.getAttribute("href")).toMatch(/^blob:/);
    });
    render(<KeptFileCard file={image} />);
    await screen.findByRole("img", { name: "chart.png" });
    fetched.apiFetch.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Download chart.png" }));
    await waitFor(() => expect(click).toHaveBeenCalledTimes(1));
    expect(fetched.apiFetch.mock.calls[0]?.[0]).toBe(`${ROUTE}?download=1`);
  });

  it("reads a text file's preview through the session", async () => {
    fetched.apiFetch.mockResolvedValue(new Response("hello <b>text</b>", { status: 200 }));
    render(
      <KeptFileCard
        file={{ ...image, preview: "text", mimeType: "text/plain", filename: "n.txt" }}
      />,
    );
    expect(await screen.findByText("hello <b>text</b>")).toBeTruthy();
    expect(fetched.apiFetch.mock.calls[0]?.[0]).toBe(ROUTE);
  });
});

describe("a file of a public share link", () => {
  it("stays a plain link: nothing is fetched, no token", () => {
    const href = `/api/public/shared/${"t".repeat(43)}/artifacts/${SHA}`;
    render(<KeptFileCard file={{ ...image, href }} />);
    expect(screen.getByRole("img", { name: "chart.png" }).getAttribute("src")).toBe(href);
    expect(screen.getByRole("link", { name: "Download chart.png" }).getAttribute("href")).toBe(
      `${href}?download=1`,
    );
    expect(fetched.apiFetch).not.toHaveBeenCalled();
  });
});

describe("a deployment with the edge's cookie", () => {
  it("is unchanged: an <img src> of the route and a download link", () => {
    setBrowserAuth(null);
    render(<KeptFileCard file={image} />);
    expect(screen.getByRole("img", { name: "chart.png" }).getAttribute("src")).toBe(ROUTE);
    expect(screen.getByRole("link", { name: "Download chart.png" }).getAttribute("href")).toBe(
      `${ROUTE}?download=1`,
    );
    expect(fetched.apiFetch).not.toHaveBeenCalled();
  });
});

describe("the Sources tab where the web holds its tokens", () => {
  const source = (over: object) => ({
    key: `file:${SHA}`,
    kind: "file" as const,
    title: "chart.png",
    detail: "2.0 KB · image/png",
    href: ROUTE,
    downloadHref: `${ROUTE}?download=1`,
    preview: "image" as const,
    turns: [{ id: "turn-1", number: 1 }],
    ...over,
  });
  const view = (...items: ReturnType<typeof source>[]) =>
    render(<SourcesView groups={[{ id: "files", label: "Files", items }]} onShowTurn={() => {}} />);

  it("opens an image inside the app, in a dialog, from an object URL, and has no link to the route", async () => {
    view(source({}));
    expect(document.querySelector(`a[href="${ROUTE}"]`)).toBeNull();
    fireEvent.click(document.querySelector('[data-slot="source-open"]') as HTMLElement);
    const img = await screen.findByAltText("chart.png");
    expect(img.getAttribute("src")).toMatch(/^blob:/);
    expect(screen.getByRole("dialog")).toBeTruthy();
    fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
    await waitFor(() => expect(revoked.length).toBeGreaterThan(0));
  });

  it("downloads anything else instead of opening it", async () => {
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    view(source({ title: "export.zip", preview: undefined }));
    fireEvent.click(document.querySelector('[data-slot="source-open"]') as HTMLElement);
    await waitFor(() => expect(click).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(fetched.apiFetch.mock.calls[0]?.[0]).toBe(`${ROUTE}?download=1`);
  });

  it("keeps plain links for a public reader's file", () => {
    const href = `/api/public/shared/${"t".repeat(43)}/artifacts/${SHA}`;
    view(source({ href, downloadHref: `${href}?download=1` }));
    expect(document.querySelector(`a[href="${href}"]`)).not.toBeNull();
    expect(document.querySelector('[data-slot="source-open"]')).toBeNull();
  });
});
