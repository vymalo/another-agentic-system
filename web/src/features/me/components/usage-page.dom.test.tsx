// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { MyUsageState } from "@/features/me/hooks/use-my-usage";
import { currentMonth, previousMonth } from "@/features/me/lib/month";
import type { ApiMyUsage } from "@/lib/api/types";

/*
 * The usage page (ADR 0065): the caller's month as the orchestrator priced it. The hook is replaced by a state the
 * test chooses; what is asserted is what a person reads: tokens and sandbox time always, money only with a currency,
 * "at least" and the unpriced lines when the total is not complete, and amounts exactly as the API wrote them.
 */
const hook = vi.hoisted(() => ({
  state: { status: "loading" } as MyUsageState,
  asked: [] as string[],
  reload: vi.fn(),
}));
vi.mock("@/features/me/hooks/use-my-usage", () => ({
  useMyUsage: (month: string) => {
    hook.asked.push(month);
    return { ...hook.state, reload: hook.reload };
  },
}));

const session = vi.hoisted(() => ({ openSignIn: vi.fn(() => "popup" as const) }));
vi.mock("@/lib/api/session", () => ({ openSignIn: session.openSignIn }));

import { UsagePage } from "./usage-page";

const MONTH = currentMonth();

const COMPLETE: ApiMyUsage = {
  month: MONTH,
  currency: "EUR",
  prices: "sha256:0123456789abcdef",
  tasks: 7,
  models: [
    {
      provider: "zai",
      model: "glm-5.3",
      inputTokens: 1_250_000,
      outputTokens: 310_500,
      totalTokens: 1_560_500,
      cachedInputTokens: 400_000,
      reasoningTokens: 90_000,
      price: "3.141593",
    },
    {
      model: "local-8b",
      inputTokens: 2_000,
      outputTokens: 500,
      totalTokens: 2_500,
      price: "0.000000",
    },
  ],
  sandbox: [
    {
      environment: "kubernetes",
      cpuMillis: 2000,
      memoryBytes: 4 * 2 ** 30,
      commands: 41,
      milliseconds: 4_320_000,
      price: "0.250000",
    },
  ],
  unmeasured: [],
  total: {
    amount: "123456789012345678.100000",
    currency: "EUR",
    complete: true,
    unpriced: { models: [], sandbox: 0, unmeasured: 0 },
  },
};

const INCOMPLETE: ApiMyUsage = {
  ...COMPLETE,
  models: [
    { ...COMPLETE.models[0], price: "3.000000" } as ApiMyUsage["models"][number],
    {
      provider: "zai",
      model: "glm-5.3-mini",
      inputTokens: 10,
      outputTokens: 20,
      totalTokens: 30,
      price: null,
    },
  ],
  sandbox: [
    { environment: "kubernetes", commands: 3, milliseconds: 90_000, price: null },
    ...COMPLETE.sandbox,
  ],
  unmeasured: [{ environment: "local", tasks: 2 }],
  total: {
    amount: "3.250000",
    currency: "EUR",
    complete: false,
    unpriced: { models: [{ provider: "zai", model: "glm-5.3-mini" }], sandbox: 1, unmeasured: 2 },
  },
};

const EMPTY: ApiMyUsage = {
  month: MONTH,
  currency: "EUR",
  prices: "sha256:0123456789abcdef",
  tasks: 0,
  models: [],
  sandbox: [],
  unmeasured: [],
  total: {
    amount: "0.000000",
    currency: "EUR",
    complete: true,
    unpriced: { models: [], sandbox: 0, unmeasured: 0 },
  },
};

const ready = (usage: ApiMyUsage) => {
  hook.state = { status: "ready", usage };
};
const total = () => screen.getByRole("region", { name: "Total" });

beforeEach(() => {
  hook.asked = [];
  hook.reload.mockClear();
  hook.state = { status: "loading" };
});
afterEach(cleanup);

describe("the usage page", () => {
  it("says it is loading, and names the month it asks for: the current UTC one", () => {
    render(<UsagePage />);
    expect(screen.getByRole("status").textContent).toContain("Loading usage");
    expect(hook.asked.at(-1)).toBe(MONTH);
    expect(screen.getByRole("heading", { level: 1, name: "Usage" })).toBeTruthy();
  });

  it("lists the models and the sandbox with their prices, and a total that is complete", () => {
    ready(COMPLETE);
    render(<UsagePage />);
    expect(total().textContent).toContain("123456789012345678.100000 EUR");
    expect(total().textContent).not.toMatch(/at least/i);
    const models = screen.getByRole("table", { name: "Models" });
    const glm = within(models).getByRole("row", { name: /glm-5\.3/ });
    expect(glm.textContent).toContain("zai");
    expect(glm.textContent).toContain("1,250,000 in");
    expect(glm.textContent).toContain("400,000 cached");
    expect(glm.textContent).toContain("310,500 out");
    expect(glm.textContent).toContain("90,000 reasoning");
    expect(glm.textContent).toContain("3.141593 EUR");
    const sandbox = screen.getByRole("table", { name: "Sandbox" });
    const row = within(sandbox).getAllByRole("row")[1];
    expect(row?.textContent).toContain("kubernetes");
    expect(row?.textContent).toContain("2 cores");
    expect(row?.textContent).toContain("4 GiB");
    expect(row?.textContent).toContain("41 commands");
    expect(row?.textContent).toContain("1 h 12 min");
    expect(row?.textContent).toContain("0.250000 EUR");
  });

  it("keeps every amount as the API wrote it: nothing is rounded, trimmed or turned into a number", () => {
    ready({ ...COMPLETE, models: [{ ...COMPLETE.models[0], price: "0.100000" } as never] });
    render(<UsagePage />);
    expect(screen.getByRole("table", { name: "Models" }).textContent).toContain("0.100000 EUR");
    expect(total().textContent).toContain("123456789012345678.100000 EUR");
  });

  it("reads 'at least' when the total is not complete, and names what is not priced", () => {
    ready(INCOMPLETE);
    render(<UsagePage />);
    expect(total().textContent).toContain("At least 3.250000 EUR");
    expect(total().textContent).toContain("zai / glm-5.3-mini");
    expect(total().textContent).toMatch(/1 sandbox shape/);
    expect(total().textContent).toMatch(/2 tasks/);
    expect(total().textContent).toMatch(/not measured/);
    const models = screen.getByRole("table", { name: "Models" });
    expect(within(models).getByRole("row", { name: /glm-5\.3-mini/ }).textContent).toContain(
      "Not priced",
    );
  });

  it("shows tokens and sandbox time with no money at all when the deployment has no currency", () => {
    ready({
      ...COMPLETE,
      currency: null,
      prices: null,
      models: COMPLETE.models.map((m) => ({ ...m, price: null })),
      sandbox: COMPLETE.sandbox.map((s) => ({ ...s, price: null })),
      total: { ...COMPLETE.total, currency: null, complete: false },
    });
    const { container } = render(<UsagePage />);
    expect(container.textContent).toContain("1,250,000 in");
    expect(container.textContent).toContain("1 h 12 min");
    expect(container.textContent).not.toMatch(/EUR|USD|Not priced|3\.141593|0\.250000/);
    expect(container.textContent).toContain("No prices are configured");
    expect(screen.queryByRole("region", { name: "Total" })?.textContent).not.toMatch(/\d\.\d{6}/);
  });

  it("an empty month is complete at zero and says there is nothing to list", () => {
    ready(EMPTY);
    render(<UsagePage />);
    expect(total().textContent).toContain("0.000000 EUR");
    expect(total().textContent).not.toMatch(/at least/i);
    expect(screen.getByText("No model usage this month.")).toBeTruthy();
    expect(screen.getByText("No sandbox time this month.")).toBeTruthy();
    expect(screen.queryByRole("table", { name: "Models" })).toBeNull();
  });

  it("moves between months and never past the current one", () => {
    ready(EMPTY);
    render(<UsagePage />);
    const next = screen.getByRole("button", { name: "Next month" });
    expect(next.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(next);
    expect(hook.asked.at(-1)).toBe(MONTH);
    fireEvent.click(screen.getByRole("button", { name: "Previous month" }));
    expect(hook.asked.at(-1)).toBe(previousMonth(MONTH));
    expect(screen.getByRole("button", { name: "Next month" }).getAttribute("aria-disabled")).toBe(
      "false",
    );
    fireEvent.click(screen.getByRole("button", { name: "Next month" }));
    expect(hook.asked.at(-1)).toBe(MONTH);
  });

  it("keeps the focus on Next month when it stops at the current month (aria-disabled, not disabled)", () => {
    ready(EMPTY);
    render(<UsagePage />);
    fireEvent.click(screen.getByRole("button", { name: "Previous month" }));
    const next = screen.getByRole("button", { name: "Next month" }) as HTMLButtonElement;
    next.focus();
    expect(document.activeElement).toBe(next);
    fireEvent.click(next);
    // it is the current month now: the button said so, and still has the focus
    const after = screen.getByRole("button", { name: "Next month" }) as HTMLButtonElement;
    expect(after.getAttribute("aria-disabled")).toBe("true");
    expect(after.disabled).toBe(false);
    expect(document.activeElement).toBe(after);
    // and pressing it again reads nothing
    const asked = hook.asked.length;
    fireEvent.click(after);
    expect(hook.asked.slice(asked).every((m) => m === MONTH)).toBe(true);
  });

  it("never draws the figures of another month under the month it names (a stale answer is loading)", () => {
    ready({ ...COMPLETE, month: previousMonth(MONTH) ?? "" });
    render(<UsagePage />);
    expect(screen.getByRole("status").textContent).toContain("Loading usage");
    expect(screen.queryByRole("region", { name: "Total" })).toBeNull();
    expect(screen.queryByRole("table")).toBeNull();
    expect(document.querySelector("[data-slot=month]")?.textContent).toContain(
      String(new Date().getUTCFullYear()),
    );
  });

  it("says in one line that months are UTC ones", () => {
    ready(EMPTY);
    render(<UsagePage />);
    expect(screen.getByText(/months are UTC/i)).toBeTruthy();
  });

  it("line amounts beyond double precision are shown exactly (a line price is never a number)", () => {
    ready({
      ...COMPLETE,
      models: [{ ...COMPLETE.models[0], price: "1234567890123.456789" } as never],
      sandbox: [{ ...COMPLETE.sandbox[0], price: "9007199254740993.000001" } as never],
    });
    render(<UsagePage />);
    expect(screen.getByRole("table", { name: "Models" }).textContent).toContain(
      "1234567890123.456789 EUR",
    );
    expect(screen.getByRole("table", { name: "Sandbox" }).textContent).toContain(
      "9007199254740993.000001 EUR",
    );
  });

  it("an answer that lacks an array draws the rest and says it was incomplete, whichever array is absent", () => {
    for (const key of ["models", "sandbox", "unmeasured"] as const) {
      cleanup();
      const partial = { ...COMPLETE } as Record<string, unknown>;
      delete partial[key];
      ready(partial as unknown as ApiMyUsage);
      render(<UsagePage />);
      expect(total().textContent, key).toContain("123456789012345678.100000 EUR");
      expect(screen.getByText(/left out part of the month/i), key).toBeTruthy();
    }
    cleanup();
    ready({ ...COMPLETE, total: { ...COMPLETE.total, unpriced: undefined } } as never);
    render(<UsagePage />);
    expect(screen.getByText(/left out part of the month/i)).toBeTruthy();
  });

  it("a complete answer carries no such note", () => {
    ready(COMPLETE);
    render(<UsagePage />);
    expect(screen.queryByText(/left out part of the month/i)).toBeNull();
  });

  it("an answer with no total has no figure to show: the error state, never a throw", () => {
    const noTotal = { ...COMPLETE } as Record<string, unknown>;
    delete noTotal.total;
    ready(noTotal as unknown as ApiMyUsage);
    render(<UsagePage />);
    expect(screen.getByRole("alert").textContent).toContain("could not be read");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(hook.reload).toHaveBeenCalledTimes(1);
  });

  it("says a total that is not complete and names nothing in a sentence of its own, with no empty list", () => {
    ready({
      ...COMPLETE,
      total: { ...COMPLETE.total, complete: false },
    });
    render(<UsagePage />);
    expect(total().textContent).toContain("At least");
    expect(total().textContent).toMatch(/does not say which/i);
    expect(total().textContent).not.toContain("Left out:");
    expect(within(total()).queryByRole("list")).toBeNull();
  });

  it("with no currency, still says how many tasks' sandbox time was not measured, beside the sandbox figure", () => {
    ready({
      ...COMPLETE,
      currency: null,
      prices: null,
      models: COMPLETE.models.map((m) => ({ ...m, price: null })),
      sandbox: COMPLETE.sandbox.map((s) => ({ ...s, price: null })),
      unmeasured: [{ environment: "local", tasks: 2 }],
      total: {
        ...COMPLETE.total,
        currency: null,
        complete: false,
        unpriced: { models: [], sandbox: 0, unmeasured: 2 },
      },
    });
    render(<UsagePage />);
    const tile = screen.getByText("Sandbox time").parentElement as HTMLElement;
    expect(tile.textContent).toContain("1 h 12 min");
    expect(tile.textContent).toMatch(/2 tasks not measured/);
  });

  it("no such note on the sandbox figure when every task was measured", () => {
    ready(COMPLETE);
    render(<UsagePage />);
    const tile = screen.getByText("Sandbox time").parentElement as HTMLElement;
    expect(tile.textContent).not.toMatch(/not measured/);
  });

  it("says 1 core, not '1 cores'", () => {
    ready({ ...COMPLETE, sandbox: [{ ...COMPLETE.sandbox[0], cpuMillis: 1000 } as never] });
    render(<UsagePage />);
    const text = screen.getByRole("table", { name: "Sandbox" }).textContent ?? "";
    expect(text).toContain("1 core,");
    expect(text).not.toContain("1 cores");
  });

  it("two lines of one model (or one shape) do not collide on a React key", () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    ready({
      ...COMPLETE,
      models: [COMPLETE.models[0], COMPLETE.models[0]] as never,
      sandbox: [COMPLETE.sandbox[0], COMPLETE.sandbox[0]] as never,
    });
    render(<UsagePage />);
    expect(screen.getAllByRole("row", { name: /glm-5\.3/ })).toHaveLength(2);
    expect(errors.mock.calls.flat().join(" ")).not.toMatch(/same key|unique "key"/);
    errors.mockRestore();
  });

  it("a signed-out person is told so, with the app's own Sign in", () => {
    hook.state = { status: "signed-out" };
    render(<UsagePage />);
    expect(screen.getByRole("status").textContent).toMatch(/signed out/i);
    expect(screen.queryByText(/something went wrong/i)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(session.openSignIn).toHaveBeenCalledTimes(1);
  });

  it("says a failed read in words and offers to try again", () => {
    hook.state = { status: "error", message: "the ledger is down" };
    render(<UsagePage />);
    expect(screen.getByRole("alert").textContent).toContain("the ledger is down");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(hook.reload).toHaveBeenCalledTimes(1);
  });

  it("says plainly that an orchestrator without the route does not report usage", () => {
    hook.state = { status: "unavailable" };
    render(<UsagePage />);
    expect(screen.getByRole("status").textContent).toContain("does not report usage");
  });
});
