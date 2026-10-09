import type { Metadata } from "next";
import { SignInGate } from "@/features/session/components/sign-in-screen";
import { SharedChat } from "@/features/sharing/components/shared-chat";

// A link is a capability: never indexed (the API's answers say it too, `X-Robots-Tag`). The
// `Referrer-Policy: same-origin` of next.config.ts keeps the token out of cross-origin referrers.
export const metadata: Metadata = {
  title: "Shared conversation",
  robots: { index: false },
};

/**
 * `/s/<token>` (ADR 0040). The token is not checked here: a token that cannot be one is the same
 * page as any link that does not work, which the page says (`LinkDoesNotWork`), never Next's own 404.
 */
export default async function SharedPage({ params }: { params: Promise<{ token: string }> }) {
  const { token } = await params;
  // a link that is not public, read by nobody signed in, is the sign-in screen (browser mode); a public reader never sees it
  return (
    <SignInGate check={false}>
      <SharedChat token={token} />
    </SignInGate>
  );
}
