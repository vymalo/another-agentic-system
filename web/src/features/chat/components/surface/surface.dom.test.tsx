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
import { type Prepared, prepareSurface } from "@/features/chat/lib/a2ui/prepare";
import {
  button,
  column,
  event,
  labelled,
  openUrl,
  SURFACE,
  surface,
  text,
} from "@/features/chat/lib/a2ui/testing";
import { loadGolden } from "@/features/chat/lib/agui/testing";
import { SurfaceView, surfaceLibrary } from "./surface-view";
import { mountSurfaces, resetSeq, stubLayout, surfaceRun } from "./testing";

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(resetSeq);
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

type Mounted = ReturnType<typeof mountSurfaces>;

/** Write frames, and wait until the agent has delivered the last group. */
async function feed(m: Mounted, frames: ReturnType<typeof surfaceRun>) {
  await act(async () => {
    m.stream.frames(frames);
  });
  const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
  await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(last));
}

const region = () => screen.getByRole("region", { name: /^Interface from / });
const regions = () => screen.queryAllByRole("region", { name: /^Interface from / });

/** A surface of one Card around one text. */
const cardSaying = (words: string) =>
  surface([{ id: "root", component: "Card", child: "t" }, text("t", words)]);

/** Makes the Card throw while it draws until `fix()`; `restore()` undoes the spies. */
function breakableCard() {
  const card = surfaceLibrary.Card as unknown as { render: (...args: unknown[]) => unknown };
  const real = card.render;
  let broken = true;
  const spy = vi.spyOn(card, "render").mockImplementation((...args: unknown[]) => {
    if (broken) throw new Error("boom");
    return real.apply(card, args);
  });
  const quiet = vi.spyOn(console, "warn").mockImplementation(() => {});
  const error = vi.spyOn(console, "error").mockImplementation(() => {});
  return {
    fix: () => {
      broken = false;
    },
    restore: () => {
      spy.mockRestore();
      quiet.mockRestore();
      error.mockRestore();
    },
  };
}

/** A title, a text field and two buttons: the Go button sends, the Open link is a link. */
const go = (name = "go", context?: Record<string, unknown>) => [
  column("root", ["title", "go", "go_label"]),
  text("title", "Pick one"),
  ...labelled("go", "Go", event(name, context)),
];

describe("the golden A2UI story, through the runtime and the renderer", () => {
  it("the surface is drawn, labelled by the orchestrator's actor, and a click sends the action once", async () => {
    const send = vi.fn();
    const m = mountSurfaces({ send });
    const frames = loadGolden("a2ui").slice(0, 26);
    await act(async () => {
      m.stream.frames(frames);
    });
    const ui = await screen.findByRole("region", { name: "Interface from plain" });
    expect(within(ui).getByText("Pick one")).toBeTruthy();
    const goButton = within(ui).getByRole("button", { name: "Go" });
    expect((goButton as HTMLButtonElement).disabled).toBe(false);
    // the surface is one part: its two snapshots replaced each other
    expect(regions()).toHaveLength(1);
    // nothing was sent by rendering it
    expect(send).not.toHaveBeenCalled();
    fireEvent.click(goButton);
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith({
      name: "go",
      surfaceId: "s1",
      sourceComponentId: "go",
      context: { choice: "a" },
    });
    m.agent.stop();
  });

  it("the action of the second run is a quiet line, and the surface stays where it was", async () => {
    const m = mountSurfaces({ canSend: false, state: "done" });
    await act(async () => {
      m.stream.frames(loadGolden("a2ui"));
    });
    await screen
      .findByText("answered: ui-action go", {}, { timeout: 10_000 })
      .catch(() => undefined);
    await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(11));
    await waitFor(() => expect(document.querySelector('[data-slot="action-line"]')).not.toBeNull());
    const line = document.querySelector('[data-slot="action-line"]') as HTMLElement;
    expect(line.textContent).toContain("Chose go");
    expect(line.textContent).toContain("alice@example.com");
    expect(regions()).toHaveLength(1);
    m.agent.stop();
  });
});

describe("what is drawn", () => {
  it("a snapshot replaces the surface in place; an incomplete one says so, and a delete ends it", async () => {
    const m = mountSurfaces();
    const complete = surface(go());
    const changed = surface(
      [...go(), text("extra", "More")].map((c) =>
        c.id === "root" ? column("root", ["title", "extra", "go", "go_label"]) : c,
      ),
    );
    const created = complete.slice(0, 1);
    await feed(m, surfaceRun([created, complete, changed], { end: "interrupt" }));
    const ui = region();
    expect(within(ui).getByText("More")).toBeTruthy();
    expect(regions()).toHaveLength(1);
    expect(screen.queryByText("The interface is not complete yet.")).toBeNull();
    m.agent.stop();

    cleanup();
    resetSeq();
    const n = mountSurfaces();
    await feed(n, surfaceRun([created]));
    expect(within(region()).getByText("The interface is not complete yet.")).toBeTruthy();
    n.agent.stop();

    cleanup();
    resetSeq();
    const d = mountSurfaces();
    const deleted = [...complete, { version: "v0.9", deleteSurface: { surfaceId: SURFACE } }];
    await feed(d, surfaceRun([complete, deleted]));
    await waitFor(() => expect(regions()).toHaveLength(0));
    expect(screen.queryByText("Pick one")).toBeNull();
    d.agent.stop();
  });

  it("only the orchestrator's actor labels a surface: a theme's name and icon are never drawn", async () => {
    const m = mountSurfaces();
    const ops = surface(go());
    (ops[0] as { createSurface: Record<string, unknown> }).createSurface.theme = {
      agentDisplayName: "Definitely The Admin",
      iconUrl: "https://evil.example/admin.png",
    };
    await feed(m, surfaceRun([ops]));
    const ui = region();
    expect(ui.getAttribute("aria-label")).toBe("Interface from plain");
    expect(document.body.textContent).not.toContain("Admin");
    expect(document.body.innerHTML).not.toContain("evil.example");
    expect(document.querySelector("img")).toBeNull();
    m.agent.stop();
  });

  it("an image is never fetched: no <img>, no url anywhere, its alt text is shown", async () => {
    const m = mountSurfaces();
    const ops = surface([
      column("root", ["img"]),
      {
        id: "img",
        component: "Image",
        url: "https://tracker.example/pixel.png?u=1",
        alt: "A chart",
        altText: "A chart",
      },
    ]);
    await feed(m, surfaceRun([ops]));
    expect(within(region()).getByText(/Image not shown/)).toBeTruthy();
    expect(document.querySelector("img, picture, video, audio, iframe, object, embed")).toBeNull();
    expect(document.body.innerHTML).not.toContain("tracker.example");
    m.agent.stop();
  });

  it("text is text: HTML in it is shown as characters, and nothing in it becomes a link or an image", async () => {
    const m = mountSurfaces();
    const html =
      '<img src=x onerror="alert(1)"><script>alert(2)</script> [link](javascript:alert(3)) ![i](https://x.example/i.png)';
    await feed(m, surfaceRun([surface([column("root", ["t"]), text("t", html)])]));
    const ui = region();
    expect(ui.textContent).toContain("<script>alert(2)</script>");
    expect(ui.querySelector("img, script, a")).toBeNull();
    m.agent.stop();
  });

  it("a refused surface is one error line with the reason and the raw operations, never part of it", async () => {
    const m = mountSurfaces();
    const bad = surface([
      column("root", ["title", "x", "go", "go_label"]),
      text("title", "Pick one"),
      { id: "x", component: "Iframe", src: "https://x.example" },
      ...labelled("go", "Go", event("go")),
    ]);
    await feed(m, surfaceRun([bad]));
    expect(screen.getByText(/Interface not shown:/)).toBeTruthy();
    expect(screen.getByText(/component "Iframe" is not in the vocabulary/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Go" })).toBeNull();
    // the title of the surface is in the raw operations, as text, and not drawn as the surface
    expect(regions()).toHaveLength(0);
    const raw = document.querySelector("details pre") as HTMLElement;
    expect(raw.textContent).toContain('"Iframe"');
    // a scrollable region is reachable by keyboard
    expect(raw.getAttribute("tabindex")).toBe("0");
    expect(document.querySelector("iframe")).toBeNull();
    m.agent.stop();
  });

  it("a component that throws while it draws is caught: the refusal line, and the rest of the page stays", async () => {
    const card = surfaceLibrary.Card as unknown as { render: () => unknown };
    const spy = vi.spyOn(card, "render").mockImplementation(() => {
      throw new Error("boom");
    });
    const quiet = vi.spyOn(console, "warn").mockImplementation(() => {});
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const m = mountSurfaces();
    await feed(
      m,
      surfaceRun([surface([{ id: "root", component: "Card", child: "t" }, text("t", "inside")])]),
    );
    expect(screen.getByText(/the interface failed to draw/)).toBeTruthy();
    expect(screen.queryByText("inside")).toBeNull();
    expect(screen.getByRole("log", { name: "Conversation" })).toBeTruthy();
    spy.mockRestore();
    quiet.mockRestore();
    error.mockRestore();
    m.agent.stop();
  });

  it("a newer copy in the same run is drawn again after a copy that failed to draw", async () => {
    const card = breakableCard();
    const m = mountSurfaces();
    const frames = surfaceRun([cardSaying("first"), cardSaying("second")]);
    // up to and including the first snapshot, then the rest of the same run
    const cut = frames.findIndex((f) => f.event.type === "ACTIVITY_SNAPSHOT") + 1;
    await feed(m, frames.slice(0, cut));
    expect(screen.getByText(/the interface failed to draw/)).toBeTruthy();
    card.fix();
    await feed(m, frames.slice(cut));
    await waitFor(() => expect(within(region()).getByText("second")).toBeTruthy());
    expect(screen.queryByText(/the interface failed to draw/)).toBeNull();
    expect(regions()).toHaveLength(1);
    card.restore();
    m.agent.stop();
  });

  it("the render guard gives a replaced surface a fresh try, even when the view is kept", () => {
    const card = breakableCard();
    const drawn = (words: string) =>
      prepareSurface(cardSaying(words)) as Extract<Prepared, { kind: "surface" }>;
    const fallback = <p>the interface failed to draw</p>;
    const { rerender } = render(<SurfaceView prepared={drawn("first")} live fallback={fallback} />);
    expect(screen.getByText("the interface failed to draw")).toBeTruthy();
    card.fix();
    const second = drawn("second");
    // the same view instance, a new spec: the guard of the failed one does not stick
    rerender(<SurfaceView prepared={second} live fallback={fallback} />);
    expect(screen.getByText("second")).toBeTruthy();
    expect(screen.queryByText("the interface failed to draw")).toBeNull();
    // the same spec again is not a new try of anything: it stays drawn
    rerender(<SurfaceView prepared={second} live fallback={fallback} />);
    expect(screen.getByText("second")).toBeTruthy();
    card.restore();
  });

  it("a surface that is refused after it was drawn is replaced by the refusal", async () => {
    const m = mountSurfaces();
    const bad = surface([column("root", ["t"]), { id: "t", component: "Icon", name: "check" }]);
    await feed(m, surfaceRun([surface(go()), bad]));
    expect(screen.getByText(/Interface not shown:/)).toBeTruthy();
    expect(screen.queryByText("Pick one")).toBeNull();
    m.agent.stop();
  });

  it("an update in a later run leaves the earlier copy read-only, and a delete in a later run removes its note", async () => {
    const send = vi.fn();
    const m = mountSurfaces({ send });
    const first = surfaceRun([surface(go())], { end: "success" });
    const second = surfaceRun(
      [surface([...go("go2")].map((c) => (c.id === "title" ? text("title", "Second") : c)))],
      {
        runId: "run-2",
        end: "success",
        user: false,
      },
    );
    await feed(m, [...first, ...second]);
    await waitFor(() => expect(screen.getAllByText("Second")).toHaveLength(1));
    expect(screen.getByText("This interface was updated further down.")).toBeTruthy();
    const live = regions();
    expect(live).toHaveLength(2);
    // only the newest copy has a button
    expect(screen.getAllByRole("button", { name: "Go" })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Go" }));
    expect(send).toHaveBeenCalledWith(expect.objectContaining({ name: "go2" }));
    m.agent.stop();
  });
});

describe("actions: a control acts on a click of its own and on nothing else", () => {
  it("nothing is sent or filled by rendering, by updates, by time, or by typing", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const send = vi.fn();
    const fillComposer = vi.fn();
    const m = mountSurfaces({ send, fillComposer });
    const many = surface(
      [
        column("root", [
          "field",
          "box",
          "a",
          "a_label",
          "b",
          "b_label",
          "c",
          "c_label",
          "d",
          "d_label",
        ]),
        { id: "field", component: "TextField", label: "Name", value: { path: "/name" } },
        { id: "box", component: "CheckBox", label: "Agree", value: { path: "/agree" } },
        ...labelled("a", "A", event("a", { name: { path: "/name" } })),
        ...labelled("b", "B", { event: { name: "b", userMessage: "Please review" } }),
        ...labelled("c", "C", { functionCall: { call: "closeModal" } }),
        ...labelled("d", "D", openUrl("https://example.com/pr/1")),
      ],
      { name: "", agree: false },
    );
    await feed(m, surfaceRun([many]));
    // more snapshots, a data model update, and a minute of timers
    const updated = [
      ...many,
      {
        version: "v0.9",
        updateDataModel: { surfaceId: SURFACE, path: "/name", contents: "changed" },
      },
    ];
    await act(async () => {
      m.stream.frames(surfaceRun([updated], { runId: "run-2", end: "interrupt", user: false }));
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(60_000);
    });
    // typing, and Enter in the field, do not send
    const field = screen.getAllByRole("textbox", { name: "Name" })[0] as HTMLInputElement;
    fireEvent.change(field, { target: { value: "Ada" } });
    fireEvent.keyDown(field, { key: "Enter" });
    fireEvent.keyPress(field, { key: "Enter", charCode: 13 });
    fireEvent.keyUp(field, { key: "Enter" });
    fireEvent.click(screen.getAllByRole("checkbox", { name: "Agree" })[0] as HTMLElement);
    expect(send).not.toHaveBeenCalled();
    expect(fillComposer).not.toHaveBeenCalled();
    m.agent.stop();
  });

  it("a click sends the action with what the owner typed; a second click before the thread moves is the app's to stop", async () => {
    const send = vi.fn();
    const m = mountSurfaces({ send });
    const ops = surface(
      [
        column("root", ["field", "box", "go", "go_label"]),
        { id: "field", component: "TextField", label: "Email", value: { path: "/email" } },
        { id: "box", component: "CheckBox", label: "Agree", value: { path: "/agree" } },
        ...labelled(
          "go",
          "Send",
          event("submit", { email: { path: "/email" }, agree: { path: "/agree" }, fixed: 1 }),
        ),
      ],
      { email: "a@example.com", agree: false },
    );
    await feed(m, surfaceRun([ops]));
    const field = screen.getByRole("textbox", { name: "Email" }) as HTMLInputElement;
    expect(field.value).toBe("a@example.com");
    fireEvent.change(field, { target: { value: "ada@example.com" } });
    fireEvent.click(screen.getByRole("checkbox", { name: "Agree" }));
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith({
      name: "submit",
      surfaceId: SURFACE,
      sourceComponentId: "go",
      context: { email: "ada@example.com", agree: true, fixed: 1 },
    });
    m.agent.stop();
  });

  it("an action whose context is over 16 KiB is refused by the app, not sent", async () => {
    const send = vi.fn();
    const reject = vi.fn();
    const m = mountSurfaces({ send, reject });
    const ops = surface([
      column("root", ["go", "go_label"]),
      ...labelled("go", "Go", event("go", { big: "x".repeat(17 * 1024) })),
    ]);
    await feed(m, surfaceRun([ops]));
    fireEvent.click(screen.getByRole("button", { name: "Go" }));
    expect(send).not.toHaveBeenCalled();
    expect(reject).toHaveBeenCalledWith(expect.stringContaining("16 KiB"));
    m.agent.stop();
  });

  it("a control that cannot act is disabled, says why once, and a click on it does nothing", async () => {
    for (const [host, why] of [
      [{ canSend: false, state: "working" as const }, /work while the thread waits for you/],
      [{ canSend: false, canCompose: false, state: "done" as const }, /This thread is finished/],
    ] as const) {
      const send = vi.fn();
      const m = mountSurfaces({ send, ...host });
      await feed(m, surfaceRun([surface(go())], { end: "success" }));
      const goButton = screen.getByRole("button", { name: "Go" }) as HTMLButtonElement;
      expect(goButton.disabled).toBe(true);
      expect(screen.getAllByText(why)).toHaveLength(1);
      fireEvent.click(goButton);
      expect(send).not.toHaveBeenCalled();
      m.agent.stop();
      cleanup();
      resetSeq();
    }
  });

  it("a userMessage goes to the message box, unsent; it is never sent as an action", async () => {
    const send = vi.fn();
    const fillComposer = vi.fn();
    const m = mountSurfaces({ send, fillComposer });
    const ops = surface([
      column("root", ["ask", "ask_label"]),
      ...labelled("ask", "Ask me", { event: { name: "x", userMessage: "Please review the plan" } }),
    ]);
    await feed(m, surfaceRun([ops]));
    fireEvent.click(screen.getByRole("button", { name: "Ask me" }));
    expect(fillComposer).toHaveBeenCalledTimes(1);
    expect(fillComposer).toHaveBeenCalledWith("Please review the plan");
    expect(send).not.toHaveBeenCalled();
    m.agent.stop();
  });

  it("a userMessage needs the message box: on a finished thread it is off", async () => {
    const fillComposer = vi.fn();
    const m = mountSurfaces({ fillComposer, canCompose: false, canSend: false, state: "done" });
    const ops = surface([
      column("root", ["ask", "ask_label"]),
      ...labelled("ask", "Ask me", { event: { name: "x", userMessage: "Please review" } }),
    ]);
    await feed(m, surfaceRun([ops], { end: "success" }));
    expect((screen.getByRole("button", { name: "Ask me" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    m.agent.stop();
  });

  it("openUrl is a link: http(s) only, a new tab, noopener noreferrer, also on a finished thread", async () => {
    const send = vi.fn();
    const m = mountSurfaces({ send, canSend: false, canCompose: false, state: "done" });
    const ops = surface([
      column("root", ["pr", "pr_label"]),
      ...labelled("pr", "Open the pull request", openUrl("HTTPS://GitHub.com/acme/demo/pull/1")),
    ]);
    await feed(m, surfaceRun([ops], { end: "success" }));
    const link = screen.getByRole("link", { name: "Open the pull request" }) as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe("https://github.com/acme/demo/pull/1");
    expect(link.getAttribute("target")).toBe("_blank");
    expect(link.getAttribute("rel")).toBe("noopener noreferrer");
    fireEvent.click(link);
    expect(send).not.toHaveBeenCalled();
    m.agent.stop();
  });

  it("a surface with a link that is not http(s) is refused, and nothing of it is a link", async () => {
    for (const url of [
      "javascript:alert(1)",
      "JaVaScRiPt:alert(1)",
      "data:text/html,x",
      "//evil.example",
      " https://example.com",
    ]) {
      const m = mountSurfaces();
      const ops = surface([
        column("root", ["title", "pr", "pr_label"]),
        text("title", "Hello"),
        ...labelled("pr", "Open", openUrl(url)),
      ]);
      await feed(m, surfaceRun([ops]));
      expect(screen.getByText(/Interface not shown:/), url).toBeTruthy();
      expect(document.querySelector('a[href^="javascript" i], a[href^="data" i]')).toBeNull();
      expect(screen.queryByRole("link")).toBeNull();
      m.agent.stop();
      cleanup();
      resetSeq();
    }
  });

  it("a button with no known action does nothing", async () => {
    const send = vi.fn();
    const m = mountSurfaces({ send });
    const ops = surface([
      column("root", ["x", "x_label"]),
      ...labelled("x", "Close", { functionCall: { call: "closeModal", args: {} } }),
    ]);
    await feed(m, surfaceRun([ops]));
    const x = screen.getByRole("button", { name: "Close" }) as HTMLButtonElement;
    expect(x.disabled).toBe(true);
    fireEvent.click(x);
    expect(send).not.toHaveBeenCalled();
    m.agent.stop();
    void button;
  });
});
