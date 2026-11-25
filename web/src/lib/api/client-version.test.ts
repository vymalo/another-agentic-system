import { beforeEach, describe, expect, it } from "vitest";
import {
  CLIENT_VERSION,
  noteUpdateRequired,
  resetUpdateNeed,
  updateNeed,
  withClientVersion,
} from "./client-version";

const problem = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/problem+json" },
  });

describe("the application's version", () => {
  beforeEach(resetUpdateNeed);

  it("is a version the API accepts", () => {
    expect(CLIENT_VERSION).toMatch(/^\d{1,9}(\.\d{1,9}){0,3}$/);
  });

  it("is stated on a request", () => {
    const request = withClientVersion(new Request("http://x.test/api/agents"));
    expect(request.headers.get("X-Client-Version")).toBe(CLIENT_VERSION);
  });

  it("notes the update-required problem and keeps the body readable", async () => {
    const res = problem(426, { code: "client_update_required", minClientVersion: "2.1.0" });
    await noteUpdateRequired(res);
    expect(updateNeed()).toEqual({ minClientVersion: "2.1.0" });
    expect((await res.json()).code).toBe("client_update_required");
  });

  it("ignores another 426, another status and a body that is not a problem", async () => {
    await noteUpdateRequired(problem(426, { code: "something_else" }));
    await noteUpdateRequired(problem(400, { code: "client_update_required" }));
    await noteUpdateRequired(new Response("nope", { status: 426 }));
    expect(updateNeed()).toBeNull();
  });
});
