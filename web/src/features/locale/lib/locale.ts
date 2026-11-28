/**
 * The interface language a message carries (`locale/v1`, ADR 0085,
 * [`docs/api/locale-v1.md`](../../../../../docs/api/locale-v1.md)): the BCP 47 tag of the page the
 * person reads, sent in `forwardedProps["vymalo.locale"]` as a **hint** and never enforced.
 *
 * The tag is a hint, so a tag the orchestrator refuses is a **400 that holds the person's message
 * back** — exactly what a hint must never do. This module therefore sends a conservative subset of
 * what the orchestrator's own `well_formed` (`orchestrator/crates/core/src/locale.rs`) accepts: a
 * language, an optional script and an optional region, and nothing else. The four interface tags of
 * the deployment (`en-GB`, `fr-FR`, `gsw-CH`, `fur-IT`) all pass; a page whose `lang` is anything
 * else sends nothing at all, and the agent falls back to its own default.
 *
 * [`isInterfaceTag`] and [`localeProps`] are pure, so the send and the tests say the same thing;
 * [`interfaceTag`] is the one read of the page, at send time and never at module load. The module
 * imports nothing on purpose: `web/scripts/run-forwarded-props.mjs` loads it in plain Node for the
 * stack scenario, so a dependency here (React, `next/*`) would break `dev/locale-real-e2e.sh`.
 */

/** `forwardedProps["vymalo.locale"]` of a run that carries a message: the page's BCP 47 tag. */
export const LOCALE_PROP = "vymalo.locale";

/**
 * The longest tag sent, in bytes: RFC 5646 section 4.4.1's floor, as the orchestrator bounds it. The
 * widest shape [`INTERFACE_TAG_SOURCE`] accepts is 17 bytes (`abcdefgh-Latn-419`), so this bound is
 * defence in depth rather than a live guard: it is here because the orchestrator's own bound is in
 * bytes, and a client that ever widened the shape must not pass a tag it would refuse.
 */
export const MAX_LOCALE_LEN = 35;

/**
 * A language of two to eight ASCII letters (RFC 5646's `2*3ALPHA`, `4ALPHA` and `5*8ALPHA`), an
 * optional script of four letters, an optional region of two letters or three digits, and nothing
 * else: no extlang, no variant, no extension, no private use, no grandfathered tag. Subtags are
 * matched case-insensitively, and the tag is sent as the page wrote it.
 *
 * The **source** is exported, and the regex built from it here, because the head script that puts
 * the picked language on `<html lang>` cannot import (`web/src/lib/language.ts`): it is written into
 * the page as a string, and it must be this rule and no second copy of it.
 */
export const INTERFACE_TAG_SOURCE = "^[A-Za-z]{2,8}(?:-[A-Za-z]{4})?(?:-[A-Za-z]{2}|-[0-9]{3})?$";

const INTERFACE_TAG = new RegExp(INTERFACE_TAG_SOURCE);

/**
 * Whether `tag` is a tag this client sends: a language, an optional script and an optional region,
 * case-insensitively, at most 35 bytes. Deliberately narrower than the orchestrator's `well_formed`
 * (`orchestrator/crates/core/src/locale.rs`), which also takes extlangs, variants, extensions and a
 * private-use part: a tag this client does not send costs nothing (the agent's own default), while
 * a tag the orchestrator refuses is a 400 that holds the person's message back. `ui.languages` is
 * held to the same shape at startup (`orchestrator/crates/config/src/rules.rs`), so the picker can
 * never offer a tag this client would not send.
 */
export function isInterfaceTag(tag: string): boolean {
  // ASCII only, so the bound is the same in characters and in bytes.
  return tag.length <= MAX_LOCALE_LEN && INTERFACE_TAG.test(tag);
}

/**
 * What a run carries for the page's tag: the one member when it is a tag this client sends, nothing
 * otherwise. `null` is the contract's "no hint", as an absent member is.
 */
export function localeProps(lang: string | undefined | null): Record<string, string> {
  return lang !== undefined && lang !== null && isInterfaceTag(lang) ? { [LOCALE_PROP]: lang } : {};
}

/**
 * The tag of the page the person reads (`<html lang>`), or `undefined` outside a browser: the module
 * is imported by the chat's client code, and a test that imports it in Node must not throw. Read
 * when a run goes out, so a page that changes its language mid-thread sends the new one.
 */
export function interfaceTag(): string | undefined {
  return typeof document === "undefined" ? undefined : document.documentElement.lang;
}
