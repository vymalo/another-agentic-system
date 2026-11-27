import { describe, expect, it } from "vitest";
import { interfaceTag, isInterfaceTag, LOCALE_PROP, localeProps } from "./locale";

describe("the interface language of a message (locale/v1, ADR 0085)", () => {
  it("names the forwardedProps member the contract has", () => {
    expect(LOCALE_PROP).toBe("vymalo.locale");
  });

  it("accepts the four interface tags of the deployment, and the shapes a page writes", () => {
    for (const tag of [
      "en-GB",
      "fr-FR",
      "gsw-CH",
      "fur-IT",
      "en",
      "zh-Hant-TW",
      "es-419",
      "EN-gb",
      "sr-Latn-RS",
    ]) {
      expect(isInterfaceTag(tag), tag).toBe(true);
    }
  });

  it("sends a tag as the page wrote it, and nothing else with it", () => {
    expect(localeProps("fr-FR")).toEqual({ "vymalo.locale": "fr-FR" });
    // subtags are case-insensitive: the page's spelling is what the agent is sent
    expect(localeProps("EN-gb")).toEqual({ "vymalo.locale": "EN-gb" });
  });

  it("sends nothing without a page tag", () => {
    expect(localeProps(undefined)).toEqual({});
    expect(localeProps(null)).toEqual({});
  });

  it("refuses what the orchestrator would refuse, so a hint never holds a message back", () => {
    for (const bad of [
      "en_CM", // a separator that is not `-`
      "en CM",
      "en--CM", // an empty subtag
      "-en",
      "en-",
      "", // the empty tag
      "x-twain", // private use only
      "i-klingon", // a grandfathered tag
      "en-GB-oed",
      "en-GB-x", // a private-use part this client never sends
      "de-CH-1901", // a variant
      "a".repeat(40), // longer than 35 bytes
      "français", // not ASCII
    ]) {
      expect(isInterfaceTag(bad), bad).toBe(false);
      expect(localeProps(bad), bad).toEqual({});
    }
  });

  it("reads no tag outside a browser", () => {
    // this suite runs in Node (vitest's default environment): importing the module never throws,
    // and a run sent without a page carries no member
    expect(interfaceTag()).toBeUndefined();
    expect(localeProps(interfaceTag())).toEqual({});
  });
});
