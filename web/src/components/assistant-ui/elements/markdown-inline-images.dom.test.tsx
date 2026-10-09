// @vitest-environment jsdom
import {
  AssistantRuntimeProvider,
  type ThreadMessageLike,
  ThreadPrimitive,
  useExternalStoreRuntime,
} from "@assistant-ui/react";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ACTIVITY, ACTOR_PART, activityPartName } from "@/features/chat/lib/agui/vymalo";
import { setBrowserAuth } from "@/lib/auth/config";
import { AssistantMessage, UserMessage } from "./thread.aui";

// the browser mode fetches a file with the web's own tokens: a request that never answers keeps its line on screen
vi.mock("@/lib/api/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api/client")>()),
  apiFetch: () => new Promise<Response>(() => {}),
}));

afterEach(() => {
  cleanup();
  setBrowserAuth(null);
  vi.restoreAllMocks();
});

const THREAD = "0190a5a5-0000-7000-8000-000000000001";
const sha = (c: string) => c.repeat(64);
const href = (hash: string) => `/api/threads/${THREAD}/artifacts/${hash}`;

const data = (type: string, value: Record<string, unknown>) => ({
  type: "data" as const,
  name: activityPartName(type),
  data: value,
});

const shareStep = (id: string, path: string, name: string) =>
  data(ACTIVITY.step, {
    id,
    path: [],
    kind: "tool",
    label: "Share a file",
    state: "completed",
    input: { path, name, repo: "demo" },
  });

const sharedFile = (hash: string, name: string, over: Record<string, unknown> = {}) =>
  data(ACTIVITY.artifact, {
    kind: "file",
    name,
    mimeType: "image/png",
    href: href(hash),
    sha256: hash,
    size: 2048,
    filename: name,
    preview: "image",
    ...over,
  });

const actor = (runId: string) => ({
  type: "data" as const,
  name: ACTOR_PART,
  data: { type: "agent", name: "coder", runId },
});

const assistant = (id: string, parts: ThreadMessageLike["content"]): ThreadMessageLike => ({
  id,
  role: "assistant",
  content: [actor(id), ...(parts as never[])],
  status: { type: "complete", reason: "stop" },
});

function Harness({ messages }: { messages: ThreadMessageLike[] }) {
  const runtime = useExternalStoreRuntime<ThreadMessageLike>({
    messages,
    convertMessage: (m) => m,
    isRunning: false,
    onNew: async () => {},
  });
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <TooltipProvider>
        <ThreadPrimitive.Root>
          <div role="log" aria-label="Conversation">
            <ThreadPrimitive.Messages>
              {({ message }) => (message.role === "user" ? <UserMessage /> : <AssistantMessage />)}
            </ThreadPrimitive.Messages>
          </div>
        </ThreadPrimitive.Root>
      </TooltipProvider>
    </AssistantRuntimeProvider>
  );
}

async function show(messages: ThreadMessageLike[]): Promise<HTMLElement> {
  render(<Harness messages={messages} />);
  const log = await screen.findByRole("log", { name: "Conversation" });
  await waitFor(() => expect(log.querySelector(".aui-md")).not.toBeNull());
  return log;
}

/** The two screenshots and the answer of the owner's thread of 2026-10-09, in its shape. */
const screenshots = (answer: string) =>
  assistant("m1", [
    shareStep("s1", "shots/3-list.png", "3-list.png"),
    sharedFile(sha("a"), "3-list.png"),
    shareStep("s2", "shots/4-matches.png", "4-matches.png"),
    sharedFile(sha("b"), "4-matches.png"),
    { type: "text", text: answer },
  ]);

describe("an image of an answer that means a file the agent shared", () => {
  it("is drawn in the words from the file's own route, with the agent's alt text", async () => {
    const log = await show([
      screenshots("Here it is:\n\n![Matches list with percentages](shots/4-matches.png)"),
    ]);
    const img = within(log).getByRole("img", { name: "Matches list with percentages" });
    expect(img.getAttribute("src")).toBe(href(sha("b")));
    expect(img.closest("[data-slot='agent-message']")).not.toBeNull();
    expect(log.querySelector("[data-slot='md-image-text']")).toBeNull();
  });

  it("is found by its base name when the path is not the one that was shared", async () => {
    const log = await show([screenshots("![List](/work/demo/shots/3-list.png)")]);
    expect(within(log).getByRole("img", { name: "List" }).getAttribute("src")).toBe(href(sha("a")));
  });

  it("does not repeat in the list of files a file that the words show; a file nothing shows stays", async () => {
    const log = await show([screenshots("![Matches](shots/4-matches.png)")]);
    const cards = [...log.querySelectorAll("[data-slot='file-card'] [data-slot='file-name']")];
    expect(cards.map((c) => c.textContent)).toEqual(["3-list.png"]);
  });

  it("leaves every file in the list when no image names one", async () => {
    const log = await show([screenshots("Both are shared, see the cards.")]);
    const cards = [...log.querySelectorAll("[data-slot='file-card'] [data-slot='file-name']")];
    expect(cards.map((c) => c.textContent)).toEqual(["3-list.png", "4-matches.png"]);
  });

  it("an image written in code is not a reference, so its file stays in the list", async () => {
    const log = await show([screenshots("Write `![x](shots/4-matches.png)` to place it.")]);
    expect(log.querySelectorAll("[data-slot='file-card']")).toHaveLength(2);
    expect(log.querySelector("img[alt='x']")).toBeNull();
  });

  it("a path nothing shared is a small placeholder, an icon and the alt text, never an <img>", async () => {
    const log = await show([screenshots("![The login page](shots/9-login.png)")]);
    expect(log.querySelector("[data-slot='md-image-text']")?.textContent).toContain(
      "The login page",
    );
    expect(log.querySelector("[data-slot='md-image-text'] svg")).not.toBeNull();
    // the shared files are all still listed
    expect(log.querySelectorAll("[data-slot='file-card']")).toHaveLength(2);
    // no image anywhere has the agent's path as its source
    for (const img of log.querySelectorAll("img")) {
      expect(img.getAttribute("src")).toMatch(/^\/api\/threads\/.+\/artifacts\/[0-9a-f]{64}$/);
    }
    expect(log.querySelector("img[src*='9-login']")).toBeNull();
  });

  it("a file that is not an image is never drawn as one", async () => {
    const log = await show([
      assistant("m1", [
        shareStep("s1", "out/notes.txt", "notes.txt"),
        sharedFile(sha("c"), "notes.txt", { mimeType: "text/plain", preview: "text" }),
        { type: "text", text: "![Notes](out/notes.txt)" },
      ]),
    ]);
    expect(log.querySelector("[data-slot='md-image-text']")?.textContent).toContain("Notes");
    expect(log.querySelector("img")).toBeNull();
    // the card stays: nothing shows it
    expect(log.querySelectorAll("[data-slot='file-card']")).toHaveLength(1);
  });

  it("remote and scheme-relative images keep the policy: a link for http(s), text for the rest, nothing fetched", async () => {
    const log = await show([
      screenshots(
        [
          "![remote](https://evil.example/4-matches.png)",
          "![network path](//evil.example/4-matches.png)",
          "![script](javascript:alert(1))",
        ].join("\n\n"),
      ),
    ]);
    expect(log.querySelector("img[src*='evil']")).toBeNull();
    const link = log.querySelector("a[data-slot='md-image-link']");
    expect(link?.getAttribute("href")).toBe("https://evil.example/4-matches.png");
    const texts = [...log.querySelectorAll("[data-slot='md-image-text']")].map(
      (e) => e.textContent,
    );
    expect(texts.join(" ")).toContain("network path");
    expect(texts.join(" ")).toContain("script");
    // nothing resolved, so nothing was hidden from the list
    expect(log.querySelectorAll("[data-slot='file-card']")).toHaveLength(2);
  });

  it("prefers the file of its own run over the same name of another", async () => {
    const log = await show([
      assistant("m1", [
        shareStep("s1", "shots/list.png", "list.png"),
        sharedFile(sha("a"), "list.png"),
        { type: "text", text: "![first](shots/list.png)" },
      ]),
      assistant("m2", [
        shareStep("s2", "shots/list.png", "list.png"),
        sharedFile(sha("b"), "list.png"),
        { type: "text", text: "![second](shots/list.png)" },
      ]),
    ]);
    expect(within(log).getByRole("img", { name: "first" }).getAttribute("src")).toBe(
      href(sha("a")),
    );
    expect(within(log).getByRole("img", { name: "second" }).getAttribute("src")).toBe(
      href(sha("b")),
    );
  });

  it("a later run can show a file an earlier run shared", async () => {
    const log = await show([
      assistant("m1", [
        shareStep("s1", "shots/list.png", "list.png"),
        sharedFile(sha("a"), "list.png"),
        { type: "text", text: "Done." },
      ]),
      assistant("m2", [{ type: "text", text: "As before: ![list](shots/list.png)" }]),
    ]);
    expect(within(log).getByRole("img", { name: "list" }).getAttribute("src")).toBe(href(sha("a")));
  });

  it("an earlier run cannot show a file shared after it", async () => {
    const log = await show([
      assistant("m1", [{ type: "text", text: "Look: ![list](shots/list.png)" }]),
      assistant("m2", [
        shareStep("s1", "shots/list.png", "list.png"),
        sharedFile(sha("a"), "list.png"),
        { type: "text", text: "Shared." },
      ]),
    ]);
    // the words of the first turn show no picture; the card of the second turn is that file's own
    expect(log.querySelector("[data-slot='md-image-file']")).toBeNull();
    expect(log.querySelector("[data-slot='md-image-text']")?.textContent).toContain("list");
  });

  it("the person's own markdown is not resolved: only an agent's words mean a shared file", async () => {
    const user: ThreadMessageLike = {
      id: "u1",
      role: "user",
      content: [{ type: "text", text: "![mine](shots/4-matches.png)" }],
    };
    const log = await show([screenshots("Shared."), user]);
    expect(log.querySelector("[data-slot='user-message'] img")).toBeNull();
  });

  it("a thread with no files at all (a public reader's) gets the placeholder", async () => {
    const log = await show([
      assistant("m1", [{ type: "text", text: "![Matches](shots/4-matches.png)" }]),
    ]);
    expect(log.querySelector("img")).toBeNull();
    expect(log.querySelector("[data-slot='md-image-text']")?.textContent).toContain("Matches");
  });

  it("goes through the renderer's own spelling of a path: escaped spaces, non-ASCII, ./, a query and a fragment", async () => {
    const log = await show([
      assistant("m1", [
        shareStep("s1", "./shots/my shot.png", "my shot.png"),
        sharedFile(sha("a"), "my shot.png"),
        shareStep("s2", "shots/é.png", "é.png"),
        sharedFile(sha("b"), "é.png"),
        shareStep("s3", "a/chart.png", "chart.png"),
        sharedFile(sha("c"), "chart.png"),
        shareStep("s4", "b/chart.png", "chart.png"),
        sharedFile(sha("d"), "chart.png"),
        shareStep("s5", "c/chart.png", "chart.png"),
        sharedFile(sha("e"), "chart.png"),
        {
          type: "text",
          text: [
            "![spaced](<shots/my shot.png>)",
            "![accent](./shots/é.png?raw=1#top)",
            // two files of one name: the path says which, whatever its spelling, and the last share is not it
            "![second](./b/chart.png?x=1#y)",
          ].join("\n\n"),
        },
      ]),
    ]);
    expect(within(log).getByRole("img", { name: "spaced" }).getAttribute("src")).toBe(
      href(sha("a")),
    );
    expect(within(log).getByRole("img", { name: "accent" }).getAttribute("src")).toBe(
      href(sha("b")),
    );
    expect(within(log).getByRole("img", { name: "second" }).getAttribute("src")).toBe(
      href(sha("d")),
    );
    // the three files the words show are not cards; the other two are
    const cards = [...log.querySelectorAll("[data-slot='file-card'] [data-slot='file-name']")];
    expect(cards.map((c) => c.textContent)).toEqual(["chart.png", "chart.png"]);
  });

  it("a picture that cannot be decoded is a line inside the paragraph, not a paragraph in a paragraph", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const log = await show([screenshots("![Matches](shots/4-matches.png)")]);
    fireEvent.error(within(log).getByRole("img", { name: "Matches" }));
    const line = await waitFor(() => {
      const el = log.querySelector("[data-slot='file-image-error']");
      expect(el).not.toBeNull();
      return el as HTMLElement;
    });
    expect(line.tagName).toBe("SPAN");
    expect(line.closest("p")).not.toBeNull();
    expect(line.textContent).toBe("The image could not be shown.");
    expect(errors.mock.calls.flat().join("\n")).not.toMatch(
      /cannot be a descendant|validateDOMNesting/,
    );
  });

  it("the line of a picture being fetched, where the web holds its own tokens, is a span too", async () => {
    setBrowserAuth({ issuer: "https://id.example", clientId: "web", scope: "openid" });
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const log = await show([screenshots("![Matches](shots/4-matches.png)")]);
    const line = await waitFor(() => {
      const el = log.querySelector("[data-slot='file-image-loading']");
      expect(el).not.toBeNull();
      return el as HTMLElement;
    });
    expect(line.tagName).toBe("SPAN");
    expect(line.closest("p")).not.toBeNull();
    expect(errors.mock.calls.flat().join("\n")).not.toMatch(
      /cannot be a descendant|validateDOMNesting/,
    );
  });
});
