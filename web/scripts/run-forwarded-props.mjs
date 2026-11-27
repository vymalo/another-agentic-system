#!/usr/bin/env node
// The `forwardedProps` of one run, built by the web's own code, for the stack scenarios that prove a
// contract against the real agents (`dev/locale-real-e2e.sh`, ADR 0085): the browser's shape, not a
// hand-written one that could drift from it.
//
//   node web/scripts/run-forwarded-props.mjs locale fr-FR   -> {"vymalo.locale":"fr-FR"}
//   node web/scripts/run-forwarded-props.mjs locale en_CM   -> {}
//
// Node 24 loads the TypeScript module as it stands (type stripping), so this needs no install and no
// build: the scenario that calls it needs only `node` beside `curl` and `jq`. A body the browser would
// not send is not printed by it, and a tag the builder refuses comes out as no member at all.
import { localeProps } from "../src/features/locale/lib/locale.ts";

const [kind, value] = process.argv.slice(2);

if (kind !== "locale") {
  console.error("usage: run-forwarded-props.mjs locale <tag>");
  process.exit(2);
}

process.stdout.write(JSON.stringify(localeProps(value)));
