import type { Metadata } from "next";
import type { ReactNode } from "react";
import "@fontsource-variable/inter";
import { TooltipProvider } from "@/components/ui/tooltip";
import "./globals.css";

export const metadata: Metadata = {
  title: "Chat — another-agentic-system",
  description: "Chat surface for the another-agentic-system orchestration layer",
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body className="bg-background text-foreground antialiased">
        <TooltipProvider>{children}</TooltipProvider>
      </body>
    </html>
  );
}
