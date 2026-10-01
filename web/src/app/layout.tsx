import type { Metadata } from "next";
import type { ReactNode } from "react";
import "@fontsource-variable/inter";
import { TooltipProvider } from "@/components/ui/tooltip";
import { SIDEBAR_SCRIPT } from "@/features/threads/lib/sidebar-state";
import "./globals.css";

export const metadata: Metadata = {
  title: "Chat — another-agentic-system",
  description: "Chat surface for the another-agentic-system orchestration layer",
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    // the head script below may mark <html> before React hydrates it
    <html lang="en" suppressHydrationWarning>
      <head>
        <script
          // biome-ignore lint/security/noDangerouslySetInnerHtml: a constant of ours, no input in it
          dangerouslySetInnerHTML={{ __html: SIDEBAR_SCRIPT }}
        />
      </head>
      <body className="bg-background text-foreground antialiased">
        <TooltipProvider>{children}</TooltipProvider>
      </body>
    </html>
  );
}
