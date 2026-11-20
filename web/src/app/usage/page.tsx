import type { Metadata } from "next";
import { UsagePage } from "@/features/me/components/usage-page";
import { SignInGate } from "@/features/session/components/sign-in-screen";

export const metadata: Metadata = { title: "Usage", robots: { index: false } };

/**
 * What the person used in a month (ADR 0065). One exported page, `usage.html`, which the static server answers for
 * `/usage` by its own `try_files {path} {path}.html` (no shell and no rewrite: `lib/static-routes.ts` maps it to itself).
 */
export default function Usage() {
  return (
    <SignInGate>
      <UsagePage />
    </SignInGate>
  );
}
