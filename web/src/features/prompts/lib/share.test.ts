import { describe, expect, it } from "vitest";
import type { ApiPromptSummary } from "@/lib/api/types";
import {
  audienceField,
  audienceFits,
  audienceIn,
  audienceOf,
  audienceSentence,
  audienceWords,
  buildShare,
  shareNotice,
  sharingControl,
} from "./share";

const prompt = (over: Partial<ApiPromptSummary> = {}): ApiPromptSummary => ({
  id: "p-aaaaaaaaaaaa",
  name: "a",
  title: "A",
  description: "",
  variableCount: 0,
  own: true,
  revision: "1",
  updatedAt: "2026-11-15T10:00:00Z",
  ...over,
});

describe("sharingControl", () => {
  it("offers Share on a private prompt and Change sharing on a shared one, to a holder of prompts.share", () => {
    expect(sharingControl(prompt({ audience: [] }), true)).toBe("share");
    expect(sharingControl(prompt({}), true)).toBe("share");
    expect(sharingControl(prompt({ audience: ["user"] }), true)).toBe("change");
    expect(sharingControl(prompt({ audience: ["*"] }), true)).toBe("change");
  });

  it("offers nothing to a person who lacks prompts.share, but Stop sharing on what they shared before", () => {
    expect(sharingControl(prompt({ audience: [] }), false)).toBeNull();
    expect(sharingControl(prompt({ audience: ["user"] }), false)).toBe("stop");
  });

  it("offers nothing while the permission is unknown, and nothing on a prompt shared with the person", () => {
    expect(sharingControl(prompt({ audience: [] }), null)).toBeNull();
    expect(sharingControl(prompt({ audience: ["user"] }), null)).toBeNull();
    for (const permitted of [true, false, null] as const) {
      expect(sharingControl(prompt({ own: false, audience: ["*"] }), permitted)).toBeNull();
      expect(sharingControl(prompt({ own: false }), permitted)).toBeNull();
    }
  });
});

describe("audienceOf", () => {
  it("splits on commas, trims, drops the empty values and keeps the order typed", () => {
    expect(audienceOf("user, prompt-team")).toEqual(["user", "prompt-team"]);
    expect(audienceOf("  prompt-team ,, user , ")).toEqual(["prompt-team", "user"]);
    expect(audienceOf("user,\n\tprompt-team")).toEqual(["user", "prompt-team"]);
  });

  it("keeps each role once, the first place it was typed", () => {
    expect(audienceOf("user, prompt-team, user")).toEqual(["user", "prompt-team"]);
  });

  it("passes * through, alone or with what else was typed: the server judges it", () => {
    expect(audienceOf("*")).toEqual(["*"]);
    expect(audienceOf(" * ")).toEqual(["*"]);
    expect(audienceOf("* user")).toEqual(["* user"]);
    expect(audienceOf("* , user")).toEqual(["*", "user"]);
  });

  it("is empty for nothing typed", () => {
    expect(audienceOf("")).toEqual([]);
    expect(audienceOf("   ")).toEqual([]);
    expect(audienceOf(" , , ")).toEqual([]);
  });
});

describe("audienceField", () => {
  it("writes the audience as the field holds it", () => {
    expect(audienceField(undefined)).toBe("");
    expect(audienceField([])).toBe("");
    expect(audienceField(["*"])).toBe("*");
    expect(audienceField(["prompt-team", "user"])).toBe("prompt-team, user");
  });
});

describe("audienceFits", () => {
  it("says whether the field can hold the audience: a role may hold the comma it separates with", () => {
    expect(audienceFits(undefined)).toBe(true);
    expect(audienceFits([])).toBe(true);
    expect(audienceFits(["*"])).toBe(true);
    expect(audienceFits(["prompt-team", "user"])).toBe(true);
    expect(audienceFits(["a,b"])).toBe(false);
    expect(audienceFits(["user", "a,b"])).toBe(false);
  });
});

describe("audienceIn", () => {
  it("reads the field as typed when it differs from what the prompt has", () => {
    expect(audienceIn("", [])).toEqual([]);
    expect(audienceIn(" user , prompt-team, user ", ["team"])).toEqual(["user", "prompt-team"]);
    expect(audienceIn("*", ["user"])).toEqual(["*"]);
  });

  it("keeps the audience the prompt has while the field is exactly what that audience renders to", () => {
    expect(audienceIn("prompt-team, user", ["prompt-team", "user"])).toEqual([
      "prompt-team",
      "user",
    ]);
    // the contract allows a comma in a role value, and the field separates roles with one: an untouched field must not
    // split a stored role into two
    expect(audienceIn("a,b", ["a,b"])).toEqual(["a,b"]);
    expect(audienceIn("a,b, user", ["a,b", "user"])).toEqual(["a,b", "user"]);
  });

  it("splits a stored role with a comma once the field is changed: that is what the dialog says it will do", () => {
    expect(audienceIn("a,b, prompt-team", ["a,b"])).toEqual(["a", "b", "prompt-team"]);
  });
});

describe("audienceWords", () => {
  it("says Everyone, Only you, or the roles", () => {
    expect(audienceWords(undefined)).toBe("Only you");
    expect(audienceWords([])).toBe("Only you");
    expect(audienceWords(["*"])).toBe("Everyone");
    expect(audienceWords(["*", "user"])).toBe("Everyone and user");
    expect(audienceWords(["prompt-team", "user"])).toBe("prompt-team, user");
  });
});

describe("audienceSentence", () => {
  it("says who can read it, in a sentence", () => {
    expect(audienceSentence([])).toBe("Only you can read it.");
    expect(audienceSentence(["*"])).toBe("Everyone can read it.");
    expect(audienceSentence(["*", "user"])).toBe(
      "“*” is only allowed alone; the server will refuse this.",
    );
    expect(audienceSentence(["user"])).toBe("Everyone with one of these roles can read it: user.");
  });
});

describe("buildShare", () => {
  it("sends the revision and the audience, and nothing else", () => {
    const body = buildShare("7", ["user", "prompt-team"]);
    expect(body).toStrictEqual({ revision: "7", audience: ["user", "prompt-team"] });
    expect(Object.keys(body).sort()).toEqual(["audience", "revision"]);
  });

  it("sends an empty audience for a prompt made private again", () => {
    expect(buildShare("8", [])).toStrictEqual({ revision: "8", audience: [] });
  });
});

describe("shareNotice", () => {
  it("says who can read it now", () => {
    expect(shareNotice("Plan", [])).toBe("“Plan” is private again.");
    expect(shareNotice("Plan", ["*"])).toBe("“Plan” is shared with Everyone.");
    expect(shareNotice("Plan", ["prompt-team", "user"])).toBe(
      "“Plan” is shared with prompt-team, user.",
    );
  });
});
