import type { Metadata, Viewport } from "next";
import type { ReactNode } from "react";
import "@fontsource-variable/inter";
import { ThemeSync } from "@/components/theme/theme-menu";
import { TooltipProvider } from "@/components/ui/tooltip";
import { PANEL_SCRIPT } from "@/features/panel/lib/panel-state";
import { ClaimedCallbackListener } from "@/features/session/components/claimed-callback-listener";
import { SessionBanner } from "@/features/session/components/session-banner";
import { SIDEBAR_SCRIPT } from "@/features/threads/lib/sidebar-state";
import { THEME_COLORS, THEME_SCRIPT } from "@/lib/theme";
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
    { media: "(prefers-color-scheme: light)", color: THEME_COLORS.light },
    { media: "(prefers-color-scheme: dark)", color: THEME_COLORS.dark },
  ],
};

export default function RootLayout({ children }: { children: ReactNode }) {
  // The three head scripts are allowed by their hash, which `scripts/csp-meta.ts` writes into every page of
  // the export (ADR 0047, Amendment 2026-10-09; `lib/csp.ts`).
  return (
    // the head script below may mark <html> before React hydrates it
    <html lang="en" suppressHydrationWarning>
      <head>
        <script
          // biome-ignore lint/security/noDangerouslySetInnerHtml: a constant of ours, no input in it
          dangerouslySetInnerHTML={{ __html: THEME_SCRIPT }}
        />
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
        <ThemeSync />
        <SessionBanner />
        <ClaimedCallbackListener />
      </body>
    </html>
  );
}
