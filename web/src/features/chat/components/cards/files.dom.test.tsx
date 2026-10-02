// @vitest-environment jsdom
import { cleanup, configure, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { loadGolden, THREAD_ID } from "@/features/chat/lib/agui/testing";
import { TEXT_PREVIEW_BYTES } from "@/features/chat/lib/files";
import { resetSeq, stubLayout } from "../surface/testing";
import { artifactFrame, card, HREF, mount, play } from "./files-testing";

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(resetSeq);
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});
afterAll(() => vi.restoreAllMocks());

describe("the card of a file the agent made", () => {
  it("shows the name, the size, a download and the picture, from the golden's own href", async () => {
    const m = mount();
    await play(m, loadGolden("file"));
    await waitFor(() => expect(card()).toBeTruthy());
    expect(document.querySelectorAll('[data-slot="file-card"]')).toHaveLength(1);
    expect(
      within(card()).getByText("chart.png", { selector: '[data-slot="file-name"]' }),
    ).toBeTruthy();
    expect(within(card()).getByText("70 bytes · image/png")).toBeTruthy();

    // download: the same route with ?download=1, as a download
    const download = within(card()).getByRole("link", { name: "Download chart.png" });
    expect(download.getAttribute("href")).toBe(`${HREF}?download=1`);
    expect(download.hasAttribute("download")).toBe(true);

    // the picture: an <img src> of the file's own href, named by its file name
    const img = within(card()).getByRole("img", { name: "chart.png" }) as HTMLImageElement;
    expect(img.getAttribute("src")).toBe(HREF);
    expect(img.getAttribute("referrerpolicy")).toBe("no-referrer");
    // a picture is a picture: no inline markup, no object, no frame
    expect(card().querySelector("svg:not([aria-hidden]), object, embed, iframe")).toBeNull();
    m.agent.stop();
  });

  it("a broken picture says so, and the download stays", async () => {
    const m = mount();
    await play(m, loadGolden("file"));
    await waitFor(() => expect(card()).toBeTruthy());
    fireEvent.error(within(card()).getByRole("img", { name: "chart.png" }));
    expect(within(card()).getByText(/The image could not be shown/)).toBeTruthy();
    expect(within(card()).queryByRole("img")).toBeNull();
    expect(within(card()).getByRole("link", { name: /Download/ })).toBeTruthy();
    m.agent.stop();
  });

  it("an image without a file name is named by a generic sentence", async () => {
    const frames = loadGolden("file").map((f) => {
      const content = (f.event as { content?: Record<string, unknown> }).content;
      if (content?.filename) delete content.filename;
      return f;
    });
    const m = mount();
    await play(m, frames);
    await waitFor(() => expect(card()).toBeTruthy());
    expect(within(card()).getByRole("img", { name: "File from the agent" })).toBeTruthy();
    // the card is titled by the artifact's own name instead
    expect(within(card()).getByText("chart", { selector: '[data-slot="file-name"]' })).toBeTruthy();
    m.agent.stop();
  });

  it("a file the store did not keep is the old card (its words), and no download", async () => {
    const m = mount();
    await play(m, [
      ...loadGolden("file").slice(0, 8),
      artifactFrame({ kind: "file", name: "huge.bin", mimeType: "application/octet-stream" }, 40),
    ]);
    await waitFor(() => expect(screen.getByText("huge.bin")).toBeTruthy());
    expect(screen.queryByRole("link", { name: /Download/ })).toBeNull();
    m.agent.stop();
  });

  it("an attachment-only file is the card alone: no picture, no preview, a download", async () => {
    const sha = "5".repeat(64);
    const m = mount();
    await play(m, [
      ...loadGolden("file").filter((f) => f.id === undefined || f.id < 3),
      artifactFrame(
        {
          kind: "file",
          name: "export",
          mimeType: "application/zip",
          href: `/api/threads/${THREAD_ID}/artifacts/${sha}`,
          sha256: sha,
          size: 3 * 1024 * 1024,
          filename: "export.zip",
          preview: null,
        },
        3,
      ),
    ]);
    await waitFor(() => expect(card()).toBeTruthy());
    expect(card().getAttribute("data-preview")).toBe("none");
    expect(within(card()).getByText("3.0 MB · application/zip")).toBeTruthy();
    expect(card().querySelector("img, pre")).toBeNull();
    expect(
      within(card()).getByRole("link", { name: "Download export.zip" }).getAttribute("href"),
    ).toBe(`/api/threads/${THREAD_ID}/artifacts/${sha}?download=1`);
    m.agent.stop();
  });

  it("a file name from the agent is text: markup in it is shown as characters", async () => {
    const hostile = '<img src=x onerror="alert(1)">.png';
    const frames = loadGolden("file").map((f) => {
      const content = (f.event as { content?: Record<string, unknown> }).content;
      if (content?.filename) content.filename = hostile;
      return f;
    });
    const m = mount();
    await play(m, frames);
    await waitFor(() => expect(card()).toBeTruthy());
    expect(within(card()).getByText(hostile, { selector: '[data-slot="file-name"]' })).toBeTruthy();
    expect(card().querySelectorAll("img")).toHaveLength(1); // the file's own picture
    m.agent.stop();
  });
});

describe("the preview of a text file", () => {
  const SHA_TEXT = "7".repeat(64);
  const textFile = (over: Record<string, unknown> = {}) =>
    artifactFrame(
      {
        kind: "file",
        name: "notes",
        mimeType: "text/plain",
        href: `/api/threads/${THREAD_ID}/artifacts/${SHA_TEXT}`,
        sha256: SHA_TEXT,
        size: 12,
        filename: "notes.txt",
        preview: "text",
        ...over,
      },
      3,
    );
  const run = () => [
    ...loadGolden("file").filter((f) => f.id === undefined || f.id < 3),
    textFile(),
  ];

  it("is fetched from the file's href, and drawn as text: markup is shown as characters", async () => {
    const body = '<script>alert(1)</script>\n<img src=x onerror="alert(2)">';
    const fetcher = vi.fn(async (_input: RequestInfo | URL) => new Response(body));
    vi.stubGlobal("fetch", fetcher);
    const m = mount();
    await play(m, run());
    await waitFor(() => expect(document.querySelector('[data-slot="file-text"]')).toBeTruthy());
    const pre = document.querySelector('[data-slot="file-text"]') as HTMLElement;
    expect(pre.textContent).toBe(body);
    expect(pre.querySelector("script, img")).toBeNull();
    expect(
      fetcher.mock.calls.filter(([u]) => String(u).includes(SHA_TEXT)).map(([u]) => String(u)),
    ).toEqual([`/api/threads/${THREAD_ID}/artifacts/${SHA_TEXT}`]);
    expect(within(card()).queryByRole("img")).toBeNull();
    expect(within(card()).getByRole("link", { name: "Download notes.txt" })).toBeTruthy();
    m.agent.stop();
  });

  it("is cut at 64 KiB, and says so", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("x".repeat(TEXT_PREVIEW_BYTES + 1000))),
    );
    const m = mount();
    await play(m, [...run().slice(0, -1), textFile({ size: TEXT_PREVIEW_BYTES + 1000 })]);
    await waitFor(() => expect(document.querySelector('[data-slot="file-text"]')).toBeTruthy());
    expect(
      (document.querySelector('[data-slot="file-text"]') as HTMLElement).textContent,
    ).toHaveLength(TEXT_PREVIEW_BYTES);
    expect(screen.getByText(/Showing the first 64 KiB of 65\.0 KB/)).toBeTruthy();
    m.agent.stop();
  });

  it("a file that cannot be read says so, and keeps its download", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("no", { status: 404 })),
    );
    const m = mount();
    await play(m, run());
    await waitFor(() =>
      expect(document.querySelector('[data-slot="file-text-error"]')).toBeTruthy(),
    );
    expect(within(card()).getByRole("link", { name: /Download notes\.txt/ })).toBeTruthy();
    m.agent.stop();
  });
});
