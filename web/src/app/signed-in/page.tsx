import type { Metadata } from "next";
import { SignedIn } from "@/features/session/components/signed-in";

export const metadata: Metadata = { title: "Signed in", robots: { index: false } };

/** The end of the sign-in popup (`openSignIn`): it closes itself. */
export default function SignedInPage() {
  return <SignedIn />;
}
