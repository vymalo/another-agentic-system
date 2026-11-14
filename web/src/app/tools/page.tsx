import type { Metadata } from "next";
import { SignInGate } from "@/features/session/components/sign-in-screen";
import { MyServersPage } from "@/features/tools/components/my-servers/my-servers-page";

export const metadata: Metadata = { title: "My tool servers", robots: { index: false } };

/**
 * The MCP servers a person adds for themselves (ADR 0046). One exported page, `tools.html`, which the static server
 * answers for `/tools` by its own `try_files {path} {path}.html` (no shell and no rewrite: `lib/static-routes.ts` maps it
 * to itself). The page holds no data: it reads `GET /api/user-tool-servers` with the person's own session.
 */
export default function Tools() {
  return (
    <SignInGate>
      <MyServersPage />
    </SignInGate>
  );
}
