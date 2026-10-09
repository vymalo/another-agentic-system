import type { Metadata } from "next";
import { SharedRoute } from "@/features/routing/shared-route";

// A link is a capability: never indexed (the API's answers say it too, `X-Robots-Tag`). The static server's
// `Referrer-Policy: same-origin` keeps the token out of cross-origin referrers.
export const metadata: Metadata = {
  title: "Shared conversation",
  robots: { index: false },
};

/** Every share link is this one exported page, `/s/_` (ADR 0047), which reads the token from the address. */
export function generateStaticParams() {
  return [{ token: "_" }];
}
export const dynamicParams = false;

export default function SharedPage() {
  return <SharedRoute />;
}
