// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { LONG_FAILURE } from "../../../../../mock/scripts";
import { ErrorCallout, FailedCallout } from "./error-callout";

afterEach(cleanup);

describe("the failure callouts", () => {
  it("say a one-line reason as it is, with nothing behind a disclosure", () => {
    const { container } = render(
      <FailedCallout
        data={{
          status: "failed",
          actor: { type: "agent", name: "adam" },
          detail: "scripted failure",
        }}
      />,
    );
    expect(container.querySelector('[data-slot="failure-message"]')?.textContent).toBe(
      "scripted failure",
    );
    expect(container.querySelector("details")).toBeNull();
  });

  it("show the first line of a long reason and keep the rest in a preformatted block behind Show details", () => {
    const { container } = render(
      <FailedCallout
        data={{ status: "failed", actor: { type: "agent", name: "adam" }, detail: LONG_FAILURE }}
      />,
    );
    const message = container.querySelector('[data-slot="failure-message"]');
    expect(message?.textContent).toBe("yarn check failed: 3 type errors in 2 files");
    const details = container.querySelector("details");
    expect(details).not.toBeNull();
    expect(details?.hasAttribute("open")).toBe(false);
    expect(details?.querySelector("summary")?.textContent).toContain("Show details");
    const pre = details?.querySelector("pre");
    const scroller = details?.querySelector('[data-slot="failure-scroll"]');
    expect(scroller?.contains(pre ?? null)).toBe(true);
    expect(scroller?.getAttribute("tabindex")).toBe("0");
    expect(scroller?.getAttribute("aria-label")).toBe("Failure details");
    // as written: the frames keep their indentation, and the message line is not repeated
    expect(pre?.textContent).toContain("12   const count: number = ");
    expect(pre?.textContent).toContain(
      "    at checkFile (node_modules/typescript/lib/tsc.js:1000:13)",
    );
    expect(pre?.textContent?.startsWith("src/app/page.tsx:12:7")).toBe(true);
    expect(pre?.textContent).not.toContain("yarn check failed");
  });

  it("treat markup in a reason as text, in the message and in the details", () => {
    const hostile = '<img src=x onerror="alert(1)">\n<script>alert(2)</script>';
    const { container } = render(<ErrorCallout data={{ message: hostile, retryable: false }} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector('[data-slot="failure-message"]')?.textContent).toBe(
      '<img src=x onerror="alert(1)">',
    );
    expect(container.querySelector("pre")?.textContent).toBe("<script>alert(2)</script>");
  });

  it("do the same for an error of the orchestrator (a gate that ran out of attempts)", () => {
    const message = [
      "the work did not pass verification after 3 attempts; agent checks: lint failed",
      "  --> src/a.ts:3:1",
      "   |",
    ].join("\n");
    const { container } = render(<ErrorCallout data={{ message, retryable: false }} />);
    expect(container.querySelector('[data-slot="failure-message"]')?.textContent).toBe(
      "the work did not pass verification after 3 attempts; agent checks: lint failed",
    );
    expect(container.querySelector("pre")?.textContent).toBe("  --> src/a.ts:3:1\n   |");
  });
});
