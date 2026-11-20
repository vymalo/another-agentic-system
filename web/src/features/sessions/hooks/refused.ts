import { problemMessage } from "@/lib/api/client";
import { refusalWords } from "../lib/refusals";

/** A request the server refused or could not be asked: the problem's code, and what the person is told. */
export type Refused = { ok: false; code?: string; status?: number; words: string };

const text = (error: unknown, member: "code" | "detail"): string | undefined => {
  if (typeof error !== "object" || error === null) return undefined;
  const value = (error as Record<string, unknown>)[member];
  return typeof value === "string" ? value : undefined;
};

/** A refusal in the screen's words (`refusals.ts`), else the server's `detail`. */
export function refusedBy(error: unknown, status: number): Refused {
  const code = text(error, "code");
  return {
    ok: false,
    ...(code ? { code } : {}),
    status,
    words: refusalWords(code, text(error, "detail")),
  };
}

/** A request that did not reach the server (or whose answer could not be read). */
export function unreached(error: unknown): Refused {
  return { ok: false, words: problemMessage(error) };
}
