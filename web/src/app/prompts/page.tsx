import type { Metadata } from "next";
import { MyPromptsPage } from "@/features/prompts/components/my-prompts-page";
import { SignInGate } from "@/features/session/components/sign-in-screen";

export const metadata: Metadata = { title: "My prompts", robots: { index: false } };

/**
 * A person's prompt library (ADR 0070). One exported page, `prompts.html`, which the static server answers for `/prompts` by its
 * own `try_files {path} {path}.html` (no shell and no rewrite: `lib/static-routes.ts` maps it to itself). The page holds no data:
 * it reads `GET /api/prompts` with the person's own session, and keeps nothing on the device.
 */
export default function Prompts() {
  return (
    <SignInGate>
      <MyPromptsPage />
    </SignInGate>
  );
}
