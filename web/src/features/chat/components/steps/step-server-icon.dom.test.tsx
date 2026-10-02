// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { TurnSteps } from "@/features/chat/lib/step-tree";
import { ToolServersProvider } from "@/features/tools/components/tool-servers-context";
import type { ApiToolServer } from "@/lib/api/types";
import { type ExpansionState, NO_EXPANSION } from "./expansion";
import { StepsPane } from "./steps-pane";
import { actorPart, assistant, RUNNING_VIEW, statusPart, stepPart, turnsOf } from "./testing";

/*
 * The icon slot of a step that calls an attached MCP server (docs/api/thread-tools-v1.md, "The step of a
 * call"): `icon: "mcp-server:<id>"` draws the image the deployment gave that server, a `data:` URI, and
 * nothing else: no icon in the list, an icon that is a URL, or a server the list does not have is the
 * glyph of a tool, and no `<img>` of any address is ever made.
 */

beforeEach(() => {
  Element.prototype.scrollIntoView = vi.fn();
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});
afterEach(cleanup);

const SVG = "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciLz4=";

function Harness({
  turns,
  servers,
  live = false,
}: {
  turns: readonly TurnSteps[];
  servers: readonly ApiToolServer[];
  live?: boolean;
}) {
  const [expanded, setExpanded] = useState<ExpansionState>(NO_EXPANSION);
  return (
    <ToolServersProvider servers={servers}>
      <StepsPane
        turns={turns}
        focus={null}
        live={live}
        expanded={expanded}
        onExpandedChange={setExpanded}
      />
    </ToolServersProvider>
  );
}

const call = (id: string, server: string, state = "completed", at = 2) =>
  stepPart(
    id,
    state,
    { label: `${server} · search`, icon: `mcp-server:${server}`, input: { query: "x" } },
    at,
  );
const turnOf = (...steps: ReturnType<typeof stepPart>[]): TurnSteps[] =>
  turnsOf([assistant([actorPart(), statusPart("working", undefined, 1), ...steps])]);
/** A turn that is still going on: its running step is really running. */
const runningTurnOf = (...steps: ReturnType<typeof stepPart>[]): TurnSteps[] =>
  turnsOf(
    [assistant([actorPart(), statusPart("working", undefined, 1), ...steps], { type: "running" })],
    RUNNING_VIEW,
  );

const rowOf = (id: string): HTMLElement =>
  document.querySelector(`[data-step="${id}"]`) as HTMLElement;
const imagesOf = (id: string) => [...rowOf(id).querySelectorAll("img")];

describe("a step that is a call of an attached MCP server", () => {
  it("draws the server's own icon, as an image of its data: URI", () => {
    render(
      <Harness
        turns={turnOf(call("T/a", "websearch"))}
        servers={[{ id: "websearch", name: "Web search", icon: SVG }]}
      />,
    );
    const images = imagesOf("T/a");
    expect(images).toHaveLength(1);
    expect(images[0]?.getAttribute("src")).toBe(SVG);
    expect(images[0]?.getAttribute("alt")).toBe("");
    expect(rowOf("T/a").getAttribute("data-server")).toBe("websearch");
  });

  it("draws the glyph of a tool for a server with no icon, one the list lacks, and while the list is not back", () => {
    render(
      <Harness
        turns={turnOf(call("T/a", "github"), call("T/b", "retired", "completed", 3))}
        servers={[{ id: "github", name: "GitHub" }]}
      />,
    );
    expect(imagesOf("T/a")).toHaveLength(0);
    expect(imagesOf("T/b")).toHaveLength(0);
    cleanup();
    render(<Harness turns={turnOf(call("T/a", "websearch"))} servers={[]} />);
    expect(imagesOf("T/a")).toHaveLength(0);
  });

  it("never makes an image of an icon that is not a data: URI of an svg, a png or a webp", () => {
    const hostile = [
      "https://tracker.example/pixel.png",
      "http://127.0.0.1:4010/probe.svg",
      "//tracker.example/x.svg",
      "data:text/html;base64,PGgxPmhpPC9oMT4=",
      "javascript:alert(1)",
    ];
    const servers = hostile.map((icon, i) => ({ id: `s${i}`, name: `S${i}`, icon }));
    render(
      <Harness
        turns={turnOf(...servers.map((s, i) => call(`T/${s.id}`, s.id, "completed", 2 + i)))}
        servers={servers}
      />,
    );
    expect(document.querySelectorAll("img")).toHaveLength(0);
    for (const s of servers) expect(rowOf(`T/${s.id}`)).not.toBeNull();
  });

  it("keeps the spinner while the call runs, and shows the icon once it has ended, failed or stopped", () => {
    const servers = [{ id: "websearch", name: "Web search", icon: SVG }];
    render(
      <Harness
        live
        turns={runningTurnOf(
          call("T/b", "websearch", "failed", 2),
          call("T/c", "websearch", "canceled", 3),
          call("T/a", "websearch", "running", 4),
        )}
        servers={servers}
      />,
    );
    // a call that is going on is a spinner, as every running step; one that ended is the server's
    expect(imagesOf("T/a")).toHaveLength(0);
    expect(imagesOf("T/b")).toHaveLength(1);
    expect(imagesOf("T/c")).toHaveLength(1);
  });
});
