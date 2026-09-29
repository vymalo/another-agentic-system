import { readFileSync } from "node:fs";
import path from "node:path";
import { RUN_DIR } from "./env";

/**
 * restart.spec.ts replaces the orchestrator that Playwright started with one of its own, which
 * Playwright does not know about: stop whichever one the pid file names.
 */
export default function teardown() {
  try {
    const pid = Number(readFileSync(path.join(RUN_DIR, "orchestrator.pid"), "utf8").trim());
    if (pid > 1) process.kill(pid, "SIGTERM");
  } catch {
    // no pid file, or the process is already gone
  }
}
