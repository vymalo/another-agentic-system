import { afterEach, describe, expect, it, vi } from "vitest";
import { exportFilename, webRevision } from "./export-thread";

const ID = "0190aaaa-0000-7000-8000-000000000123";

describe("exportFilename", () => {
  it("takes the plain name the server gave", () => {
    expect(exportFilename(ID, `attachment; filename="thread-${ID}.json"`)).toBe(
      `thread-${ID}.json`,
    );
  });

  it("names the file after the thread when the header is missing or says nothing usable", () => {
    const fallback = `thread-${ID}.json`;
    expect(exportFilename(ID, null)).toBe(fallback);
    expect(exportFilename(ID, "attachment")).toBe(fallback);
    expect(exportFilename(ID, 'attachment; filename=""')).toBe(fallback);
  });

  it("never lets the server choose a path or a hidden file", () => {
    const fallback = `thread-${ID}.json`;
    expect(exportFilename(ID, 'attachment; filename="../../etc/passwd"')).toBe(fallback);
    expect(exportFilename(ID, 'attachment; filename="a/b.json"')).toBe(fallback);
    expect(exportFilename(ID, 'attachment; filename=".bashrc"')).toBe(fallback);
    expect(exportFilename(ID, 'attachment; filename="a b.json"')).toBe(fallback);
  });
});

describe("the revision the web sends with an export (ADR 0053)", () => {
  afterEach(() => {
    vi.unstubAllEnvs();
    vi.unstubAllGlobals();
  });

  it("is the build's commit, trimmed, and nothing for a build that was given none", () => {
    vi.stubEnv("NEXT_PUBLIC_BUILD_REVISION", "  0123abc  ");
    expect(webRevision()).toBe("0123abc");
    vi.stubEnv("NEXT_PUBLIC_BUILD_REVISION", "");
    expect(webRevision()).toBeUndefined();
    vi.stubEnv("NEXT_PUBLIC_BUILD_REVISION", "   ");
    expect(webRevision()).toBeUndefined();
  });

  async function sentWith(env: string | undefined): Promise<Request> {
    vi.stubEnv("NEXT_PUBLIC_BUILD_REVISION", env);
    let seen: Request | undefined;
    // the app calls relative URLs (the page's own origin); Node's Request wants absolute ones, and the API
    // client binds Request when it is created, so it is imported after the patch
    const RealRequest = globalThis.Request;
    vi.stubGlobal(
      "Request",
      class extends RealRequest {
        constructor(input: RequestInfo | URL, init?: RequestInit) {
          super(
            typeof input === "string" && input.startsWith("/")
              ? `http://localhost${input}`
              : (input as RequestInfo),
            init,
          );
        }
      },
    );
    vi.stubGlobal(
      "fetch",
      vi.fn(async (request: Request) => {
        seen = request;
        return new Response("{}", {
          headers: {
            "content-type": "application/json",
            "content-disposition": `attachment; filename="thread-${ID}.json"`,
          },
        });
      }),
    );
    vi.resetModules();
    const { fetchThreadExport } = await import("./export-thread");
    const file = await fetchThreadExport(ID);
    expect(file.filename).toBe(`thread-${ID}.json`);
    if (!seen) throw new Error("nothing was sent");
    return seen;
  }

  it("goes with the request for the export as X-Web-Revision", async () => {
    const request = await sentWith("0123abc");
    expect(new URL(request.url).pathname).toBe(`/api/threads/${ID}/export`);
    expect(request.headers.get("x-web-revision")).toBe("0123abc");
  });

  it("is not sent by a build that has none", async () => {
    const request = await sentWith("");
    expect(request.headers.has("x-web-revision")).toBe(false);
  });
});
