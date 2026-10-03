import { describe, expect, it } from "vitest";
import type { ApiMe } from "@/lib/api/types";
import {
  absoluteLink,
  chipLabel,
  isShareToken,
  levelsFor,
  menuOffersShare,
  type ShareLevel,
  sharedFileHref,
  sharedPath,
  sharingOf,
} from "./sharing";

const TOKEN = "A".repeat(43);
const me = (sharing?: ApiMe["sharing"]): ApiMe => ({
  user: "dev@example.com",
  roles: ["user"],
  permissions: [{ permission: "thread.read", scope: "own" }],
  agents: { read: ["*"], invoke: ["*"] },
  ...(sharing ? { sharing } : {}),
});

describe("what the person may share as", () => {
  it("is what /api/me says, and disabled when it says nothing or cannot be read", () => {
    expect(sharingOf(me("public"))).toBe("public");
    expect(sharingOf(me("internal"))).toBe("internal");
    expect(sharingOf(me())).toBe("disabled");
    expect(sharingOf(null)).toBe("disabled");
  });

  it("allows each level up to the cap and says why the others are not", () => {
    const allowed = (cap: "disabled" | "internal" | "public"): ShareLevel[] =>
      levelsFor(cap)
        .filter((l) => l.allowed)
        .map((l) => l.level);
    expect(allowed("disabled")).toEqual(["private"]);
    expect(allowed("internal")).toEqual(["private", "internal"]);
    expect(allowed("public")).toEqual(["private", "internal", "public"]);
    for (const l of levelsFor("internal")) {
      expect(l.reason === undefined).toBe(l.allowed);
    }
    expect(levelsFor("internal").find((l) => l.level === "public")?.reason).toMatch(
      /signed-in people/,
    );
    expect(levelsFor("disabled").find((l) => l.level === "internal")?.reason).toMatch(/turned off/);
  });

  it("puts Share… in the menu when sharing is on, and for a shared thread whatever the cap says", () => {
    expect(menuOffersShare("disabled", undefined)).toBe(false);
    expect(menuOffersShare("internal", undefined)).toBe(true);
    // revoking needs only ownership: the owner of a shared thread can always reach Stop sharing
    expect(menuOffersShare("disabled", { visibility: "internal", effective: "private" })).toBe(
      true,
    );
  });
});

describe("the chip", () => {
  it("says who can read it now", () => {
    expect(chipLabel({ visibility: "internal", effective: "internal" })).toBe("Shared · signed-in");
    expect(chipLabel({ visibility: "public", effective: "public" })).toBe("Shared · public");
    // a public thread under a cap of internal is served as internal
    expect(chipLabel({ visibility: "public", effective: "internal" })).toBe("Shared · signed-in");
    expect(chipLabel({ visibility: "public", effective: "private" })).toBe("Sharing paused");
  });
});

describe("the link", () => {
  it("is made absolute against the page's origin, and left alone when it already is", () => {
    expect(absoluteLink(`/s/${TOKEN}`, "https://chat.example.com")).toBe(
      `https://chat.example.com/s/${TOKEN}`,
    );
    expect(absoluteLink(`https://chat.example.com/s/${TOKEN}`, "http://127.0.0.1:3000")).toBe(
      `https://chat.example.com/s/${TOKEN}`,
    );
  });

  it("knows a token's shape: 43 characters of the unpadded base64url alphabet", () => {
    expect(isShareToken(TOKEN)).toBe(true);
    expect(isShareToken(`${"a-_Z9".repeat(8)}abc`)).toBe(true);
    expect(isShareToken(TOKEN.slice(1))).toBe(false);
    expect(isShareToken(`${TOKEN}A`)).toBe(false);
    expect(isShareToken(`${"A".repeat(42)}=`)).toBe(false);
    expect(isShareToken(`${"A".repeat(42)}/`)).toBe(false);
  });

  it("is /s/<token>", () => {
    expect(sharedPath(TOKEN)).toBe(`/s/${TOKEN}`);
  });

  it("serves a file by the shared route of the reader's kind, never the owner's", () => {
    const sha = "b".repeat(64);
    expect(sharedFileHref(TOKEN, "internal", sha)).toBe(`/api/shared/${TOKEN}/artifacts/${sha}`);
    expect(sharedFileHref(TOKEN, "public", sha)).toBe(
      `/api/public/shared/${TOKEN}/artifacts/${sha}`,
    );
  });
});
