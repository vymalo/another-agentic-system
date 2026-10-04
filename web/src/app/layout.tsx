import type { Metadata, Viewport } from "next";
import type { ReactNode } from "react";
import "@fontsource-variable/inter";
import { TooltipProvider } from "@/components/ui/tooltip";
import { PANEL_SCRIPT } from "@/features/panel/lib/panel-state";
import { SessionBanner } from "@/features/session/components/session-banner";
import { SIDEBAR_SCRIPT } from "@/features/threads/lib/sidebar-state";
import "./globals.css";

// The icons are the file conventions of this folder (favicon.ico, icon.svg, apple-icon.png) and
// manifest.ts; web/DESIGN.md "Brand" says where they come from.
export const metadata: Metadata = {
  title: { default: "another·agentic", template: "%s · another·agentic" },
  applicationName: "another·agentic",
  description: "Chat surface for the another-agentic-system orchestration layer",
};

export const viewport: Viewport = {
  themeColor: [
    { media: "(prefers-color-scheme: light)", color: "#ffffff" },
    { media: "(prefers-color-scheme: dark)", color: "#141614" },
  ],
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
        <script
          // biome-ignore lint/security/noDangerouslySetInnerHtml: a constant of ours, no input in it
          dangerouslySetInnerHTML={{ __html: PANEL_SCRIPT }}
        />
      </head>
      <body className="bg-background text-foreground antialiased">
        <TooltipProvider>{children}</TooltipProvider>
        <SessionBanner />
      </body>
    </html>
  );
}
