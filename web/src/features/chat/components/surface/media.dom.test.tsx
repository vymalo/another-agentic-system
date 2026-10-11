// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { KeptFile } from "@/features/chat/lib/files";
import { setBrowserAuth } from "@/lib/auth/config";

const fetched = vi.hoisted(() => ({ apiFetch: vi.fn() }));
vi.mock("@/lib/api/client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api/client")>()),
  apiFetch: fetched.apiFetch,
}));

import { GalleryView } from "./gallery";
import { LinkPreviewView } from "./link-preview";
import { AudioFile, VideoFile } from "./media";
import { ThreadFilesContext } from "./thread-files";

/**
 * Catalog version 6, group 4 (media), rendered against the thread's kept files (the list the validator checks against):
 * a Gallery of pictures that opens larger, a LinkPreview that fetches nothing, and a Video and an Audio that play a
 * file of the thread and say so when they cannot.
 */

afterEach(cleanup);

const file = (sha: string, over: Partial<KeptFile>): KeptFile => ({
  sha256: sha,
  href: `/api/threads/t/artifacts/${sha}`,
  size: 2048,
  name: "file",
  preview: null,
  ...over,
});
const PIC_A = "a1".repeat(32);
const PIC_B = "a2".repeat(32);
const WAV = "b1".repeat(32);
const MP4 = "b2".repeat(32);
const pictures = [
  file(PIC_A, { name: "chart", filename: "chart.png", mimeType: "image/png", preview: "image" }),
  file(PIC_B, { name: "list", filename: "list.png", mimeType: "image/png", preview: "image" }),
];

describe("Gallery", () => {
  it("is a grid of the pictures, and a picture opens larger, where the arrows move to the next one", () => {
    render(
      <ThreadFilesContext.Provider value={pictures}>
        <GalleryView
          title="Runs"
          images={[
            { artifact: PIC_A, alt: "A chart of the runs" },
            { artifact: PIC_B, alt: "The list" },
          ]}
        />
      </ThreadFilesContext.Provider>,
    );
    expect(screen.getByText("Runs")).toBeTruthy();
    const items = screen.getAllByRole("button", { name: /Open the picture/ });
    expect(items).toHaveLength(2);
    fireEvent.click(items[0] as HTMLElement);
    const dialog = screen.getByRole("dialog", { name: "A chart of the runs" });
    expect(dialog.getAttribute("aria-describedby")).toBeTruthy();
    expect(within(dialog).getByText("1 of 2")).toBeTruthy();
    fireEvent.keyDown(dialog, { key: "ArrowRight" });
    expect(within(dialog).getByText("2 of 2")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Previous picture" }));
    expect(within(dialog).getByText("1 of 2")).toBeTruthy();
    fireEvent.keyDown(dialog, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("a picture the thread does not hold is a line, never a broken picture", () => {
    render(
      <ThreadFilesContext.Provider value={[]}>
        <GalleryView images={[{ artifact: PIC_A, alt: "Gone" }]} />
      </ThreadFilesContext.Provider>,
    );
    expect(screen.getByText("Picture not shown: Gone.")).toBeTruthy();
    expect(screen.queryByRole("button")).toBeNull();
  });
});

describe("LinkPreview", () => {
  it("is a link that opens in a new tab, with the title, the description, the site and the host, and nothing fetched", () => {
    render(
      <LinkPreviewView
        url="https://www.postgresql.org/docs/current/"
        title="PostgreSQL documentation"
        description="The manual of the current release."
        site="postgresql.org"
      />,
    );
    const link = screen.getByRole("link", { name: /PostgreSQL documentation/ });
    expect(link.getAttribute("href")).toBe("https://www.postgresql.org/docs/current/");
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toBe("noopener noreferrer");
    expect(screen.getByText("The manual of the current release.")).toBeTruthy();
    expect(screen.getAllByText("postgresql.org")).toHaveLength(2);
  });

  it("a link that does not pass the rule is words, not a link", () => {
    render(<LinkPreviewView url="javascript:alert(1)" title="Not a link" />);
    expect(screen.getByText("Not a link")).toBeTruthy();
    expect(screen.queryByRole("link")).toBeNull();
  });
});

describe("Video and Audio", () => {
  it("a video of the thread plays from its own route, named by its title, with its download", () => {
    render(
      <ThreadFilesContext.Provider
        value={[file(MP4, { name: "clip", filename: "clip.mp4", mimeType: "video/mp4" })]}
      >
        <VideoFile artifact={MP4} title="The demo" caption="Recorded on Tuesday." />
      </ThreadFilesContext.Provider>,
    );
    const video = document.querySelector("video") as HTMLVideoElement;
    expect(video.getAttribute("src")).toBe(`/api/threads/t/artifacts/${MP4}`);
    expect(video.getAttribute("aria-label")).toBe("The demo");
    expect(video.hasAttribute("controls")).toBe(true);
    expect(screen.getByRole("link", { name: "Download The demo" })).toBeTruthy();
    expect(screen.getByText("Recorded on Tuesday.")).toBeTruthy();
  });

  it("a video that cannot be played says so, and keeps its download", () => {
    render(
      <ThreadFilesContext.Provider value={[file(MP4, { mimeType: "video/mp4" })]}>
        <VideoFile artifact={MP4} title="The demo" />
      </ThreadFilesContext.Provider>,
    );
    fireEvent.error(document.querySelector("video") as HTMLVideoElement);
    expect(screen.getByText(/could not be played/)).toBeTruthy();
    expect(screen.getByRole("link", { name: "Download The demo" })).toBeTruthy();
  });

  it("an audio plays a sound of the thread, and a file that is not a sound is a line", () => {
    render(
      <ThreadFilesContext.Provider
        value={[file(WAV, { mimeType: "audio/wav" }), file(MP4, { mimeType: "video/mp4" })]}
      >
        <AudioFile artifact={WAV} title="Tone" />
        <AudioFile artifact={MP4} title="Not a sound" />
      </ThreadFilesContext.Provider>,
    );
    const audio = document.querySelector("audio") as HTMLAudioElement;
    expect(audio.getAttribute("src")).toBe(`/api/threads/t/artifacts/${WAV}`);
    expect(screen.getByText("This sound is not one of this conversation's files.")).toBeTruthy();
  });
});

describe("a video in browser mode (ADR 0054): the file comes with the tokens, so it is fetched first", () => {
  let counter = 0;
  beforeEach(() => {
    counter = 0;
    URL.createObjectURL = () => `blob:http://app.test/${++counter}`;
    URL.revokeObjectURL = () => undefined;
    fetched.apiFetch.mockReset();
    fetched.apiFetch.mockImplementation(
      async () =>
        new Response(new Uint8Array([0, 0, 0, 24]), {
          status: 200,
          headers: { "Content-Type": "video/mp4" },
        }),
    );
    setBrowserAuth({ issuer: "https://id.example", clientId: "web", scope: "openid" });
  });
  afterEach(() => {
    setBrowserAuth(null);
    vi.restoreAllMocks();
  });
  const clip = (over: Partial<KeptFile> = {}) =>
    file(MP4, { name: "clip", filename: "clip.mp4", mimeType: "video/mp4", ...over });

  it("says it is loading and never asks for the route without the tokens; it plays from an object URL", async () => {
    render(
      <ThreadFilesContext.Provider value={[clip()]}>
        <VideoFile artifact={MP4} title="The demo" />
      </ThreadFilesContext.Provider>,
    );
    expect(screen.getByText("Loading the video…")).toBeTruthy();
    expect(document.querySelector("video")).toBeNull();
    await waitFor(() =>
      expect((document.querySelector("video") as HTMLVideoElement).getAttribute("src")).toMatch(
        /^blob:/,
      ),
    );
    expect(screen.queryByText(/could not be played/)).toBeNull();
    expect(fetched.apiFetch).toHaveBeenCalledTimes(1);
  });

  it("a file that cannot be fetched says so, and keeps its download; it does not fall back to the route", async () => {
    fetched.apiFetch.mockImplementation(async () => new Response("no", { status: 401 }));
    render(
      <ThreadFilesContext.Provider value={[clip()]}>
        <VideoFile artifact={MP4} title="The demo" />
      </ThreadFilesContext.Provider>,
    );
    await waitFor(() => expect(screen.getByText(/could not be played/)).toBeTruthy());
    expect(document.querySelector("video")).toBeNull();
    expect(screen.getByRole("button", { name: "Download The demo" })).toBeTruthy();
  });

  it("a failure is for the source it failed on: a new source plays, and the words go", async () => {
    const { rerender } = render(
      <ThreadFilesContext.Provider value={[clip()]}>
        <VideoFile artifact={MP4} title="The demo" />
      </ThreadFilesContext.Provider>,
    );
    await waitFor(() =>
      expect((document.querySelector("video") as HTMLVideoElement).getAttribute("src")).toMatch(
        /^blob:/,
      ),
    );
    fireEvent.error(document.querySelector("video") as HTMLVideoElement);
    expect(screen.getByText(/could not be played/)).toBeTruthy();

    const NEXT = "b3".repeat(32);
    rerender(
      <ThreadFilesContext.Provider value={[clip(), file(NEXT, { mimeType: "video/mp4" })]}>
        <VideoFile artifact={NEXT} title="The next demo" />
      </ThreadFilesContext.Provider>,
    );
    await waitFor(() =>
      expect((document.querySelector("video") as HTMLVideoElement).getAttribute("src")).toMatch(
        /^blob:/,
      ),
    );
    expect(screen.queryByText(/could not be played/)).toBeNull();
  });
});
