import type { Metadata } from "next";
import { AuthCallback } from "@/features/session/components/auth-callback";

export const metadata: Metadata = { title: "Signing in", robots: { index: false } };

/** Where the issuer sends the browser back to (ADR 0054): the code is exchanged here. */
export default function AuthCallbackPage() {
  return <AuthCallback />;
}
