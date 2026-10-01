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
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { mountSurfaces, STEPS_PANE, stubLayout } from "@/features/chat/components/surface/testing";
import { type GoldenFrame, loadGolden, THREAD_ID } from "@/features/chat/lib/agui/testing";
import type { CiContent } from "@/features/chat/lib/agui/vymalo";
import { CI_CONCLUSIONS, conclusionLabel } from "@/features/chat/lib/ci";
import { CiStep, NAME_PREVIEW, SUMMARY_PREVIEW } from "./ci-step";

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
afterEach(cleanup);

const SHA = "0000000000000000000000000000000000000001";

const ci = (over: Partial<CiContent> = {}): CiContent => ({
  name: "ci/build",
  conclusion: "failure",
  passed: false,
  sha: SHA,
  shortSha: "0000000",
  provider: "generic",
  repository: "github.com/acme/demo",
  ...over,
});

// a report is a step of the turn: a list item named by the check and its conclusion
const card = () => screen.getByRole("listitem", { name: /^CI: / });
const iconOf = (el: HTMLElement) =>
  [...(el.querySelector("[data-slot='badge'] svg")?.classList ?? [])].find((c) =>
    c.startsWith("lucide-"),
  );

describe("the CI card", () => {
  it("says the conclusion in words, names the check, and gives the commit, its provider and repository", () => {
    render(
      <CiStep
        data={ci({
          branch: "agent/fix",
          url: "https://ci.example.com/runs/1",
          summary: "1 test failed: tests::login",
          actor: { type: "system", name: "orchestrator" },
        })}
      />,
    );
    expect(card().getAttribute("aria-label")).toBe("CI: ci/build, failure");
    const view = within(card());
    expect(within(card()).getByText("Failure").closest("[data-slot='badge']")).not.toBeNull();
    expect(view.getByText("ci/build")).toBeTruthy();
    expect(view.getByText("0000000").getAttribute("title")).toBe(SHA);
    expect(view.getByText("0000000").tagName).toBe("CODE");
    expect(view.getByText("agent/fix")).toBeTruthy();
    expect(view.getByText("Branch", { exact: false })).toBeTruthy();
    expect(view.getByText("Generic webhook · github.com/acme/demo")).toBeTruthy();
    expect(view.getByText("1 test failed: tests::login")).toBeTruthy();
    // the orchestrator is not an author worth a label on its own reports
    expect(view.queryByText("orchestrator")).toBeNull();
    expect(card().getAttribute("data-conclusion")).toBe("failure");
    expect(card().getAttribute("data-passed")).toBe("false");
  });

  it("an agent that reported is named", () => {
    render(<CiStep data={ci({ actor: { type: "agent", name: "verifier", revision: "v-r3" } })} />);
    expect(within(card()).getByText("verifier · v-r3")).toBeTruthy();
  });

  it("names GitHub as GitHub", () => {
    render(<CiStep data={ci({ provider: "github", conclusion: "success", passed: true })} />);
    expect(within(card()).getByText("GitHub · github.com/acme/demo")).toBeTruthy();
  });

  it("every conclusion has its words, its own icon and a tone; the words are in the accessible name", () => {
    const icons = new Set<string>();
    const tones: Record<string, string> = {};
    for (const conclusion of CI_CONCLUSIONS) {
      const passed = ["success", "neutral", "skipped"].includes(conclusion);
      const { unmount } = render(<CiStep data={ci({ conclusion, passed })} />);
      const label = conclusionLabel(conclusion);
      expect(card().getAttribute("aria-label")).toBe(`CI: ci/build, ${label.toLowerCase()}`);
      const badge = within(card()).getByText(label).closest("[data-slot='badge']") as HTMLElement;
      expect(badge, conclusion).not.toBeNull();
      // the words are the badge's text; the icon is decoration
      expect(badge.textContent).toBe(label);
      expect(badge.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
      icons.add(iconOf(card()) ?? conclusion);
      tones[conclusion] =
        [...badge.classList].find((c) =>
          /^text-(success|destructive|warning|muted-foreground)$/.test(c),
        ) ?? "";
      expect(card().getAttribute("data-passed")).toBe(passed ? "true" : "false");
      unmount();
    }
    // nine conclusions, nine icons: colour is never the only difference
    expect(icons.size).toBe(CI_CONCLUSIONS.length);
    expect(tones).toMatchObject({
      success: "text-success",
      failure: "text-destructive",
      timed_out: "text-destructive",
      startup_failure: "text-destructive",
      action_required: "text-warning",
      cancelled: "text-muted-foreground",
      stale: "text-muted-foreground",
      neutral: "text-muted-foreground",
      skipped: "text-muted-foreground",
    });
  });

  it("a conclusion it does not know is shown by its name; `passed` picks the colour", () => {
    const { rerender } = render(
      <CiStep data={ci({ conclusion: "partial_success", passed: true })} />,
    );
    expect(within(card()).getByText("Partial success")).toBeTruthy();
    expect(card().getAttribute("aria-label")).toBe("CI: ci/build, partial success");
    expect(within(card()).getByText("Partial success").className).toContain("text-success");
    rerender(<CiStep data={ci({ conclusion: "melted", passed: false })} />);
    expect(within(card()).getByText("Melted").className).toContain("text-destructive");
  });

  it("branch, summary and link are left out when the report has none", () => {
    const { container } = render(<CiStep data={ci()} />);
    expect(container.querySelector("[data-slot='ci-branch']")).toBeNull();
    expect(container.querySelector("[data-slot='ci-summary']")).toBeNull();
    expect(within(card()).queryByRole("link")).toBeNull();
  });

  describe("View run", () => {
    it("is a link to the run when the url is http(s): a new tab, without opener or referrer", () => {
      render(<CiStep data={ci({ url: "https://ci.example.com/runs/1" })} />);
      const link = within(card()).getByRole("link", { name: /^View run/ });
      expect(link.getAttribute("href")).toBe("https://ci.example.com/runs/1");
      expect(link.getAttribute("target")).toBe("_blank");
      expect(link.getAttribute("rel")).toBe("noopener noreferrer");
      expect(link.textContent).toContain("View run");
      cleanup();
      render(<CiStep data={ci({ url: "http://ci.example.com/runs/1" })} />);
      expect(
        within(card())
          .getByRole("link", { name: /^View run/ })
          .getAttribute("href"),
      ).toBe("http://ci.example.com/runs/1");
    });

    it("is not drawn without a url", () => {
      render(<CiStep data={ci()} />);
      expect(screen.queryByRole("link")).toBeNull();
      expect(screen.queryByText(/View run/)).toBeNull();
    });

    it("is never drawn for another scheme, even when the card is handed one directly", () => {
      for (const url of [
        "javascript:window.pwned=1",
        "JAVASCRIPT:window.pwned=1",
        " javascript:window.pwned=1",
        "java\tscript:window.pwned=1",
        "data:text/html,<script>window.pwned=1</script>",
        "vbscript:x",
        "file:///etc/passwd",
        "blob:https://ci.example.com/x",
        "ftp://ci.example.com/x",
        "mailto:a@example.com",
        "//ci.example.com/runs/1",
        "/runs/1",
        "https://user:pass@ci.example.com/runs/1",
        "https://ci.example.com\\@evil.example/",
      ]) {
        const { container, unmount } = render(<CiStep data={ci({ url })} />);
        expect(container.querySelector("a"), url).toBeNull();
        expect(container.querySelector("[href], [src], [action]"), url).toBeNull();
        expect(container.textContent).not.toContain("View run");
        unmount();
      }
      expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
    });
  });

  it("name, branch and summary are text: markup and markdown are the characters they are", () => {
    const name = '<img src=x onerror="window.pwned=1"> **bold** [x](javascript:window.pwned=2)';
    const branch = "<script>window.pwned = 3</script>";
    const summary =
      "# heading\n- item\n\n> quote `code` <a href='https://evil.example'>go</a> https://auto.example/link";
    const { container } = render(
      <CiStep data={ci({ name, branch, summary, provider: "<b>x</b>", repository: "<i>y</i>" })} />,
    );
    const view = within(card());
    expect(view.getByText(name)).toBeTruthy();
    expect(view.getByText(branch)).toBeTruthy();
    expect(container.querySelector("[data-slot='ci-summary']")?.textContent).toBe(summary);
    expect(view.getByText("<b>x</b> · <i>y</i>")).toBeTruthy();
    // none of it became an element: the only `code` is the commit, and there is no link
    // (the step itself is the one `li`; nothing inside it is markup)
    expect(
      card().querySelector(
        "script, img, a, b, i, strong, em, h1, ul, li, blockquote, iframe, svg[onload]",
      ),
    ).toBeNull();
    expect(container.querySelectorAll("code")).toHaveLength(1);
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
  });

  it("a long summary is cut, with a control that shows all of it and takes it back", () => {
    const long = `${"x".repeat(SUMMARY_PREVIEW)}TAIL-OF-THE-SUMMARY`;
    const { container } = render(<CiStep data={ci({ summary: long })} />);
    const summary = container.querySelector("[data-slot='ci-summary']") as HTMLElement;
    expect(summary.textContent).not.toContain("TAIL-OF-THE-SUMMARY");
    expect(summary.textContent).toContain("…");
    const more = within(summary).getByRole("button", { name: "Show more" });
    expect(more.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(more);
    expect(summary.textContent).toContain("TAIL-OF-THE-SUMMARY");
    const less = within(summary).getByRole("button", { name: "Show less" });
    expect(less.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(less);
    expect(summary.textContent).not.toContain("TAIL-OF-THE-SUMMARY");
  });

  it("a short summary has no control", () => {
    const { container } = render(<CiStep data={ci({ summary: "3 tests passed" })} />);
    expect(
      within(container.querySelector("[data-slot='ci-summary']") as HTMLElement).queryByRole(
        "button",
      ),
    ).toBeNull();
  });

  it("a very long name and branch are cut, the whole name stays in the title", () => {
    const name = `${"n".repeat(NAME_PREVIEW)}END-OF-THE-NAME`;
    const { container } = render(<CiStep data={ci({ name, branch: `${"b".repeat(300)}END` })} />);
    const shown = container.querySelector("[data-slot='ci-name']") as HTMLElement;
    expect(shown.textContent).not.toContain("END-OF-THE-NAME");
    expect(shown.textContent).toContain("…");
    expect(shown.getAttribute("title")).toBe(name);
    expect(card().getAttribute("aria-label")?.length).toBeLessThan(NAME_PREVIEW + 40);
    expect(container.querySelector("[data-slot='ci-branch']")?.textContent).not.toContain("END");
  });

  it("a commit that is not a hash is cut, not trusted", () => {
    render(
      <CiStep
        data={ci({ shortSha: "<script>alert(1)</script>", sha: "<script>alert(1)</script>" })}
      />,
    );
    expect(within(card()).getByText("<script>aler…")).toBeTruthy();
  });
});

// ---- through the runtime: the card as the transcript draws it -------------------------------

type Mounted = ReturnType<typeof mountSurfaces>;

async function feed(m: Mounted, frames: GoldenFrame[]) {
  await act(async () => {
    m.stream.frames(frames);
  });
  const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
  await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(last));
}

const actor = { "vymalo.actor": { type: "system", name: "orchestrator" } };
const content = (over: Record<string, unknown> = {}) => ({
  name: "ci/build",
  conclusion: "failure",
  passed: false,
  sha: SHA,
  shortSha: "0000000",
  provider: "generic",
  repository: "github.com/acme/demo",
  ...over,
});

/** A run of the given activities (each `[messageId, activityType, content]`), closed by success. */
function activities(list: [string, string, unknown][], first = 1): GoldenFrame[] {
  let seq = first;
  return [
    { event: { type: "RUN_STARTED", threadId: THREAD_ID, runId: "run-1", protocolVersion: "1.0" } },
    ...list.map(([messageId, activityType, body]) => ({
      id: seq++,
      event: {
        type: "ACTIVITY_SNAPSHOT",
        messageId,
        activityType,
        content: body,
        replace: true,
        metadata: actor,
      },
    })),
    {
      id: seq,
      event: {
        type: "RUN_FINISHED",
        threadId: THREAD_ID,
        runId: "run-1",
        outcome: { type: "success" },
      },
    },
  ];
}

const cards = () => screen.queryAllByRole("listitem", { name: /^CI: / });
const checks = () => screen.queryAllByRole("listitem", { name: /^Check: / });

describe("the CI card in the transcript", () => {
  it("the ci golden: a card for each report, next to the check it decided, in the order of the log", async () => {
    const m = mountSurfaces({}, {}, STEPS_PANE);
    await feed(m, loadGolden("ci"));
    await waitFor(() => expect(cards()).toHaveLength(2));
    expect(cards().map((c) => c.getAttribute("aria-label"))).toEqual([
      "CI: ci/build, failure",
      "CI: ci/build, success",
    ]);
    const [red, green] = cards() as [HTMLElement, HTMLElement];
    expect(within(red).getByText("1 test failed: tests::login")).toBeTruthy();
    expect(within(red).getByText("0000000").getAttribute("title")).toMatch(/0001$/);
    expect(within(red).getByText("agent/fix")).toBeTruthy();
    expect(
      within(red)
        .getByRole("link", { name: /^View run/ })
        .getAttribute("href"),
    ).toBe("https://ci.example.com/runs/1");
    expect(within(green).getByText("3 tests passed")).toBeTruthy();
    expect(within(green).getByText("0000000").getAttribute("title")).toMatch(/0002$/);
    expect(
      within(green)
        .getByRole("link", { name: /^View run/ })
        .getAttribute("href"),
    ).toBe("https://ci.example.com/runs/2");
    expect(within(red).queryByText("orchestrator")).toBeNull();

    // the gate's own cards are still there: pending was replaced by the answer, one per attempt
    expect(checks().map((c) => c.getAttribute("aria-label"))).toEqual([
      "Check: CI, attempt 1, failed",
      "Check: CI, attempt 2, passed",
    ]);
    // document order: check, CI report, rework, check, CI report
    // the steps are the side panel's: the tree is where the cards are
    const log = document.querySelector('[data-slot="steps-pane"]') as HTMLElement;
    const order = [
      ...log.querySelectorAll(
        "[data-slot='check-card'], [data-slot='ci-card'], [data-slot='rework-step']",
      ),
    ].map(
      (n) =>
        `${n.getAttribute("data-slot")}:${n.getAttribute("data-conclusion") ?? n.getAttribute("data-status") ?? ""}`,
    );
    expect(order).toEqual([
      "check-card:failed",
      "ci-card:failure",
      "rework-step:",
      "check-card:passed",
      "ci-card:success",
    ]);
    m.agent.stop();
  });

  it("the connect stream of the real orchestrator draws the same two cards", async () => {
    const m = mountSurfaces({}, {}, STEPS_PANE);
    await feed(m, loadGolden("connect-ci"));
    await waitFor(() => expect(cards()).toHaveLength(2));
    expect(cards().map((c) => c.getAttribute("data-conclusion"))).toEqual(["failure", "success"]);
    m.agent.stop();
  });

  it("a card is replaced in place by whatever id the wire gives it, and is not keyed by sha and name", async () => {
    const m = mountSurfaces({}, {}, STEPS_PANE);
    await feed(m, activities([["any-id-1", "vymalo.ci", content()]]).slice(0, 2));
    await waitFor(() => expect(cards()).toHaveLength(1));
    expect(cards()[0]?.getAttribute("data-conclusion")).toBe("failure");
    // the same id again: the card changes where it was
    await feed(m, [
      {
        id: 2,
        event: {
          type: "ACTIVITY_SNAPSHOT",
          messageId: "any-id-1",
          activityType: "vymalo.ci",
          replace: true,
          content: content({ conclusion: "success", passed: true, summary: "now green" }),
          metadata: actor,
        },
      },
    ]);
    await waitFor(() => expect(cards()[0]?.getAttribute("data-conclusion")).toBe("success"));
    expect(cards()).toHaveLength(1);
    expect(within(cards()[0] as HTMLElement).getByText("now green")).toBeTruthy();
    // another id for the same commit and check: a card of its own
    await feed(m, [
      {
        id: 3,
        event: {
          type: "ACTIVITY_SNAPSHOT",
          messageId: "any-id-2",
          activityType: "vymalo.ci",
          replace: true,
          content: content({ conclusion: "cancelled", passed: false }),
          metadata: actor,
        },
      },
    ]);
    await waitFor(() => expect(cards()).toHaveLength(2));
    expect(cards().map((c) => c.getAttribute("data-conclusion"))).toEqual(["success", "cancelled"]);
    m.agent.stop();
  });

  it("a malformed payload draws nothing, unknown fields are ignored, and the rest of the run renders", async () => {
    const m = mountSurfaces({}, {}, STEPS_PANE);
    await feed(
      m,
      activities([
        ["e1", "vymalo.ci", {}],
        ["e2", "vymalo.ci", content({ name: undefined })],
        ["e3", "vymalo.ci", content({ passed: "yes" })],
        ["e4", "vymalo.ci", content({ conclusion: 7 })],
        ["e5", "vymalo.ci", { name: "ci/build", conclusion: "failure" }],
        [
          "e8",
          "vymalo.ci",
          content({ conclusion: "success", passed: true, futureField: { x: [1] } }),
        ],
        ["e9", "vymalo.future", { anything: true }],
      ]),
    );
    await waitFor(() => expect(cards()).toHaveLength(1));
    expect(cards()[0]?.getAttribute("data-conclusion")).toBe("success");
    m.agent.stop();
  });

  it("a javascript: url from the wire is no link", async () => {
    const m = mountSurfaces({}, {}, STEPS_PANE);
    await feed(
      m,
      activities([
        ["e1", "vymalo.ci", content({ url: "javascript:window.pwned=1", summary: "<b>hi</b>" })],
      ]),
    );
    await waitFor(() => expect(cards()).toHaveLength(1));
    // the steps are the side panel's: the tree is where the cards are
    const log = document.querySelector('[data-slot="steps-pane"]') as HTMLElement;
    expect(within(log).getByText("<b>hi</b>")).toBeTruthy();
    expect(log.querySelector("a, b")).toBeNull();
    expect(screen.queryByText(/View run/)).toBeNull();
    m.agent.stop();
  });
});
