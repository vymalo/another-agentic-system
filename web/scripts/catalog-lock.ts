/**
 * `pnpm catalog:lock [<version>]`: writes `catalog.lock.json` for the catalog of this build.
 *
 * The version is bumped by hand and orders the catalogs a thread has seen (docs/api/ui-catalog-v1.md),
 * so this refuses to put a new digest under the version it already had: change the catalog, then
 * run it with the next version. An unchanged catalog needs no argument and writes nothing.
 * After a bump, add the pair to `RELEASED` in `catalog.test.ts` (the test says so).
 */
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { catalogDigest } from "../src/features/chat/lib/a2ui/catalog/digest";

const dir = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "../src/features/chat/lib/a2ui/catalog",
);
const catalogFile = path.join(dir, "catalog.json");
const lockFile = path.join(dir, "catalog.lock.json");

const read = (file: string): unknown => JSON.parse(readFileSync(file, "utf8"));
const fail = (message: string): never => {
  console.error(`catalog:lock: ${message}`);
  process.exit(1);
};

const digest = await catalogDigest(read(catalogFile));
const current = read(lockFile) as { version: number; digest: string };
const arg = process.argv[2];
const requested = arg === undefined ? undefined : Number(arg);
if (requested !== undefined && (!Number.isSafeInteger(requested) || requested < 1)) {
  fail(`the version must be an integer of at least 1, not ${JSON.stringify(arg)}`);
}

if (digest === current.digest) {
  if (requested !== undefined && requested !== current.version) {
    fail(
      `the catalog did not change, so it stays version ${current.version} (asked for ${requested})`,
    );
  }
  console.log(`catalog:lock: version ${current.version}, ${digest}: up to date`);
} else {
  if (requested === undefined || requested <= current.version) {
    fail(
      `the catalog changed (${current.digest} -> ${digest}) but the version did not: ` +
        `run \`pnpm catalog:lock ${current.version + 1}\` (the lock has version ${current.version})`,
    );
  }
  writeFileSync(lockFile, `${JSON.stringify({ version: requested, digest }, null, 2)}\n`);
  console.log(
    `catalog:lock: version ${requested}, ${digest}: written. ` +
      "Add the pair to RELEASED in catalog.test.ts.",
  );
}
