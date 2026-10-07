import type { Metadata } from "next";
import { SignOut } from "@/features/session/components/sign-out";

export const metadata: Metadata = { title: "Sign out", robots: { index: false } };

export default function SignOutPage() {
  return <SignOut />;
}
