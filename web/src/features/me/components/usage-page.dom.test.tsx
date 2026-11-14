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
    const next = screen.getByRole("button", { name: "Next month" }) as HTMLButtonElement;
    expect(next.disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Previous month" }));
    expect(hook.asked.at(-1)).toBe(previousMonth(MONTH));
    expect((screen.getByRole("button", { name: "Next month" }) as HTMLButtonElement).disabled).toBe(
      false,
    );
    fireEvent.click(screen.getByRole("button", { name: "Next month" }));
    expect(hook.asked.at(-1)).toBe(MONTH);
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
