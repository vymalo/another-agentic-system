// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { ApiAgent } from "@/lib/api/types";
import { AgentNamesProvider, useAgentNames } from "./agent-names-context";

afterEach(cleanup);

function Names({ ids }: { ids: string[] }) {
  const names = useAgentNames();
  return <p data-testid="names">{ids.map((id) => names.get(id) ?? `(${id})`).join(" ")}</p>;
}

describe("the names of the agents the page lists", () => {
  it("name an agent by its id and by its aliases: a thread keeps the id it was created with (ADR 0049)", () => {
    const agents: ApiAgent[] = [
      { id: "adam", name: "Adam", aliases: ["coder"] },
      { id: "chat", name: "Chat" },
    ];
    const { getByTestId } = render(
      <AgentNamesProvider agents={agents}>
        <Names ids={["adam", "coder", "chat", "gone"]} />
      </AgentNamesProvider>,
    );
    expect(getByTestId("names").textContent).toBe("Adam Adam Chat (gone)");
  });
});
