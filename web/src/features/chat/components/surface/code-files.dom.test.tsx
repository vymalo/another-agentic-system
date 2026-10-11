// @vitest-environment jsdom
import {
  act,
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { OWN_CATALOG } from "@/features/chat/lib/a2ui/catalog";
import { surface } from "@/features/chat/lib/a2ui/testing";
import type { KeptFile } from "@/features/chat/lib/files";
import { FileCardView } from "./file-card";
import { mountSurfaces, resetSeq, stubLayout, surfaceRun } from "./testing";
import { ThreadFilesContext } from "./thread-files";

/**
 * Catalog version 6, group 1 (code and files), drawn by the real renderer through the runtime:
 * Code, Diff, Terminal and PullRequest in one surface; FileCard against the thread's kept files.
 */

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(resetSeq);
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

type Rec = Record<string, unknown>;
type Mounted = ReturnType<typeof mountSurfaces>;

async function feed(m: Mounted, frames: ReturnType<typeof surfaceRun>) {
  await act(async () => {
    m.stream.frames(frames);
  });
  const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
  await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(last));
}

const drawn = (components: Rec[]) =>
  surface(components, undefined, "v0.9.1", OWN_CATALOG.catalogId);

async function mountSurface(components: Rec[]) {
  const m = mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version });
  await feed(m, surfaceRun([drawn(components)]));
  return m;
}

const SOURCE = "const a = 1;\nconsole.log(a);";
const LOG = Array.from({ length: 20 }, (_, i) => `line ${i}`).join("\n");

describe("Code", () => {
  it("is the code as text, with its title and filename, and a copy button that copies it", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const m = await mountSurface([
      { id: "root", component: "Column", children: ["code"] },
      { id: "code", component: "Code", title: "Setup", filename: "src/a.ts", code: SOURCE },
    ]);
    expect(screen.getByText("Setup")).toBeTruthy();
    expect(screen.getByText("src/a.ts")).toBeTruthy();
    const block = screen.getByRole("region", { name: "Setup" });
    expect(block.textContent).toContain("console.log(a);");
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(SOURCE));
    expect(await screen.findByText("Copied")).toBeTruthy();
    m.agent.stop();
  });
});

describe("Code: what is read is what is there", () => {
  it("shows a character that hides text as its code point, and copies the text as it is read", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await mountSurface([
      { id: "root", component: "Column", children: ["code"] },
      { id: "code", component: "Code", title: "Check", code: "if (isAdmin) { // \u202E }" },
    ]);
    const block = screen.getByRole("region", { name: "Check" });
    expect(block.textContent).toContain("\u27e8U+202E\u27e9");
    expect(block.textContent).not.toContain("\u202E");
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    await waitFor(() =>
      expect(writeText).toHaveBeenCalledWith("if (isAdmin) { // \u27e8U+202E\u27e9 }"),
    );
  });

  it("says a copy the browser refuses has failed, whether there is no clipboard or it throws", async () => {
    Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
    await mountSurface([
      { id: "root", component: "Column", children: ["code"] },
      { id: "code", component: "Code", title: "Setup", code: SOURCE },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    expect(await screen.findByText("Copy failed")).toBeTruthy();
    cleanup();
    Object.defineProperty(navigator, "clipboard", {
      value: {
        writeText: () => {
          throw new Error("not allowed");
        },
      },
      configurable: true,
    });
    await mountSurface([
      { id: "root", component: "Column", children: ["code"] },
      { id: "code", component: "Code", title: "Setup", code: SOURCE },
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Copy code" }));
    expect(await screen.findByText("Copy failed")).toBeTruthy();
  });
});

describe("Diff", () => {
  it("marks each added and removed line in words as well as by its sign, and keeps the headers", async () => {
    const m = await mountSurface([
      { id: "root", component: "Column", children: ["diff"] },
      {
        id: "diff",
        component: "Diff",
        title: "Rename",
        diff: "--- a/x.ts\n+++ b/x.ts\n@@ -1 +1 @@\n-const old = 1;\n+const next = 1;",
      },
    ]);
    const added = document.querySelector('[data-kind="add"]') as HTMLElement;
    const removed = document.querySelector('[data-kind="remove"]') as HTMLElement;
    expect(added.textContent).toContain("Added: const next = 1;");
    expect(removed.textContent).toContain("Removed: const old = 1;");
    expect(document.querySelectorAll('[data-kind="header"]')).toHaveLength(2);
    expect(document.querySelector('[data-kind="hunk"]')?.textContent).toContain("@@ -1 +1 @@");
    m.agent.stop();
  });
});

describe("Terminal", () => {
  it("shows the command, its exit code in words, and folds a long output behind a button", async () => {
    const m = await mountSurface([
      { id: "root", component: "Column", children: ["term"] },
      {
        id: "term",
        component: "Terminal",
        title: "Tests",
        command: "npm test",
        exitCode: 2,
        output: LOG,
      },
    ]);
    expect(screen.getByText("npm test")).toBeTruthy();
    expect(screen.getByText(/Exit 2/)).toBeTruthy();
    expect(screen.getByText(/failed/)).toBeTruthy();
    const toggle = screen.getByRole("button", { name: "Show all 20 lines" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("line 19")).toBeNull();
    fireEvent.click(toggle);
    expect(screen.getByText(/line 19/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Show less" }).getAttribute("aria-expanded")).toBe(
      "true",
    );
    m.agent.stop();
  });
});

describe("PullRequest", () => {
  it("names the repository and number, says the state in words, counts the checks and links the title", async () => {
    const m = await mountSurface([
      { id: "root", component: "Column", children: ["pr"] },
      {
        id: "pr",
        component: "PullRequest",
        repo: "acme/demo",
        number: 7,
        title: "Add login",
        state: "merged",
        branch: "feature/login",
        checks: { passed: 3, failed: 1, pending: 2 },
        url: "https://github.com/acme/demo/pull/7",
      },
    ]);
    expect(screen.getByText("acme/demo #7")).toBeTruthy();
    expect(screen.getByText("Merged")).toBeTruthy();
    expect(screen.getByText("feature/login")).toBeTruthy();
    const link = screen.getByRole("link", { name: /Add login/ });
    expect(link.getAttribute("href")).toBe("https://github.com/acme/demo/pull/7");
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toBe("noopener noreferrer");
    const checks = screen.getByRole("list", { name: "Checks" });
    expect(within(checks).getByText("3 passed")).toBeTruthy();
    expect(within(checks).getByText("1 failed")).toBeTruthy();
    expect(within(checks).getByText("2 pending")).toBeTruthy();
    m.agent.stop();
  });
});

describe("FileCard", () => {
  const SHA = "d4".repeat(32);
  const notes: KeptFile = {
    sha256: SHA,
    href: `/api/threads/t/artifacts/${SHA}`,
    size: 12,
    name: "notes",
    filename: "notes.txt",
    mimeType: "text/plain",
    preview: "text",
  };

  it("is the file's name, its size and type, and a download, for a file the thread holds", () => {
    render(
      <ThreadFilesContext.Provider value={[notes]}>
        <FileCardView artifact={SHA} />
      </ThreadFilesContext.Provider>,
    );
    expect(screen.getByText("notes.txt")).toBeTruthy();
    expect(screen.getByText("12 bytes · text/plain")).toBeTruthy();
    const download = screen.getByRole("link", { name: "Download notes.txt" });
    expect(download.getAttribute("href")).toBe(`/api/threads/t/artifacts/${SHA}?download=1`);
  });

  it("says so, instead of drawing anything, for a file the thread does not hold", () => {
    render(
      <ThreadFilesContext.Provider value={[]}>
        <FileCardView artifact={"0".repeat(64)} />
      </ThreadFilesContext.Provider>,
    );
    expect(screen.getByText(/not one of this conversation's files/)).toBeTruthy();
    expect(screen.queryByRole("link")).toBeNull();
  });
});

describe("PullRequest: the link's host", () => {
  it("shows the host of its link in words, so the person sees where it goes", async () => {
    await mountSurface([
      { id: "root", component: "Column", children: ["pr"] },
      {
        id: "pr",
        component: "PullRequest",
        repo: "acme/demo",
        number: 7,
        title: "Fix the login",
        state: "open",
        url: "https://www.example.org/acme/demo/pull/7",
      },
    ]);
    expect(screen.getByText("example.org")).toBeTruthy();
  });
});
