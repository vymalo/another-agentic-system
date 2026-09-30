import { truncate } from "./findings";

/**
 * Reading a CI report for the card (ADR 0017). The closed set of conclusions is that of
 * `docs/api/webhooks.md#conclusions`, plus `startup_failure`, which only GitHub reports. A newer
 * orchestrator may send one this UI has not heard of: it is shown by its own name, and `passed`
 * (which the orchestrator computes) still says how it counts.
 */
export const CI_CONCLUSIONS = [
  "success",
  "neutral",
  "skipped",
  "failure",
  "cancelled",
  "timed_out",
  "action_required",
  "stale",
  "startup_failure",
] as const;
export type CiConclusion = (typeof CI_CONCLUSIONS)[number];

export const isKnownConclusion = (c: string): c is CiConclusion =>
  (CI_CONCLUSIONS as readonly string[]).includes(c);

const LABELS: Record<CiConclusion, string> = {
  success: "Success",
  neutral: "Neutral",
  skipped: "Skipped",
  failure: "Failure",
  cancelled: "Cancelled",
  timed_out: "Timed out",
  action_required: "Action required",
  stale: "Stale",
  startup_failure: "Startup failure",
};

/** The conclusion in words: a known one by its label, an unknown one (untrusted) by its own name, shortened. */
export function conclusionLabel(conclusion: string): string {
  if (isKnownConclusion(conclusion)) return LABELS[conclusion];
  const words = truncate(conclusion.replaceAll("_", " "), 40).text;
  return words.charAt(0).toUpperCase() + words.slice(1);
}

const PROVIDERS: Record<string, string> = { github: "GitHub", generic: "Generic webhook" };

/** The provider as the orchestrator names it (`github`, `generic`), an unknown one shortened. */
export const providerLabel = (provider: string): string =>
  PROVIDERS[provider] ?? truncate(provider, 40).text;
