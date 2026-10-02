import { describe, expect, it } from "vitest";
import type { ApiToolServer } from "@/lib/api/types";
import { toolsLines } from "./line";
import {
  currentTools,
  nameOf,
  offeredFor,
  sameSet,
  sortedSet,
  stillOffered,
  withServer,
} from "./servers";

const servers: ApiToolServer[] = [
  { id: "websearch", name: "Web search" },
  { id: "github", name: "GitHub", agents: ["coder"] },
  { id: "files", name: "Files", agents: ["coder", "reviewer"] },
];

describe("offeredFor", () => {
  it("lists a server whose agents are absent for every agent, and the others only for theirs", () => {
    expect(offeredFor(servers, "coder").map((s) => s.id)).toEqual(["websearch", "github", "files"]);
    expect(offeredFor(servers, "reviewer").map((s) => s.id)).toEqual(["websearch", "files"]);
    expect(offeredFor(servers, "verifier").map((s) => s.id)).toEqual(["websearch"]);
  });

  it("keeps the deployment's order, and is whole while the agent is not known", () => {
    expect(offeredFor(servers, null).map((s) => s.id)).toEqual(["websearch", "github", "files"]);
  });
});

describe("the choice", () => {
  it("is a sorted set", () => {
    expect(sortedSet(["b", "a", "b"])).toEqual(["a", "b"]);
    expect(withServer(["files"], "github", true)).toEqual(["files", "github"]);
    expect(withServer(["files", "github"], "files", false)).toEqual(["github"]);
    expect(withServer(["files"], "files", true)).toEqual(["files"]);
    expect(sameSet(["b", "a"], ["a", "b", "a"])).toBe(true);
    expect(sameSet(["a"], ["a", "b"])).toBe(false);
  });

  it("keeps of a choice what the agent may have attached, and drops what is no longer listed", () => {
    expect(stillOffered(["github", "websearch", "gone"], servers, "reviewer")).toEqual([
      "websearch",
    ]);
  });

  it("names a server by the list, else by its id", () => {
    expect(nameOf(servers, "github")).toBe("GitHub");
    expect(nameOf(servers, "retired")).toBe("retired");
  });
});

describe("currentTools", () => {
  const base = { stream: undefined, streamSeq: 0, fetched: undefined, fetchedSeq: null, put: null };

  it("is the stream's when it is at least as far as the resource, else the resource's", () => {
    expect(
      currentTools({ ...base, stream: ["a"], streamSeq: 5, fetched: ["b"], fetchedSeq: 5 }),
    ).toEqual(["a"]);
    expect(
      currentTools({ ...base, stream: ["a"], streamSeq: 4, fetched: ["b"], fetchedSeq: 5 }),
    ).toEqual(["b"]);
    // the stream has delivered nothing yet: the resource has read further
    expect(currentTools({ ...base, fetched: ["b"], fetchedSeq: 3 })).toEqual(["b"]);
  });

  it("is none for a stream that says none (the member is absent when there are none)", () => {
    expect(
      currentTools({ ...base, stream: undefined, streamSeq: 7, fetched: ["a"], fetchedSeq: 6 }),
    ).toEqual([]);
  });

  it("holds the answer of a PUT until something newer than it says otherwise", () => {
    const put = { servers: ["websearch"], seq: 3 };
    // a stale stream and a stale read: the answer stands
    expect(currentTools({ ...base, streamSeq: 3, fetched: [], fetchedSeq: 3, put })).toEqual([
      "websearch",
    ]);
    // the change's own event arrives
    expect(
      currentTools({
        ...base,
        stream: ["websearch"],
        streamSeq: 4,
        fetched: [],
        fetchedSeq: 3,
        put,
      }),
    ).toEqual(["websearch"]);
    // a later change by another tab wins over the old answer
    expect(
      currentTools({ ...base, stream: ["github"], streamSeq: 5, fetched: [], fetchedSeq: 3, put }),
    ).toEqual(["github"]);
  });
});

describe("toolsLines", () => {
  it("says the names of the deployment's list, with and, and each member on its own line", () => {
    expect(toolsLines({ attached: ["websearch"] }, servers)).toEqual(["Web search attached"]);
    expect(toolsLines({ attached: ["websearch", "github"] }, servers)).toEqual([
      "Web search and GitHub attached",
    ]);
    expect(toolsLines({ attached: ["websearch", "github", "files"] }, servers)).toEqual([
      "Web search, GitHub and Files attached",
    ]);
    expect(toolsLines({ attached: ["files"], detached: ["websearch"] }, servers)).toEqual([
      "Files attached",
      "Web search detached",
    ]);
  });

  it("names a server the list does not have by its id", () => {
    expect(toolsLines({ detached: ["retired"] }, [])).toEqual(["retired detached"]);
  });
});
