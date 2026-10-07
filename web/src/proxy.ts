import { type NextRequest, NextResponse } from "next/server";
import { contentSecurityPolicy, makeNonce } from "@/lib/csp";

/**
 * Sets the content security policy of every page (ADR 0054, decision 10, `lib/csp.ts`) with a fresh
 * nonce. Next reads the nonce from the request's policy and puts it on its own scripts; the layout
 * reads `x-nonce` for the scripts of ours. The API and the edge's routes are not pages.
 */
export function proxy(request: NextRequest) {
  const nonce = makeNonce();
  const policy = contentSecurityPolicy({
    nonce,
    connect: process.env.WEB_CSP_CONNECT_SRC,
    dev: process.env.NODE_ENV === "development",
  });
  const headers = new Headers(request.headers);
  headers.set("x-nonce", nonce);
  headers.set("Content-Security-Policy", policy);
  const response = NextResponse.next({ request: { headers } });
  response.headers.set("Content-Security-Policy", policy);
  return response;
}

export const config = {
  matcher: [
    {
      source:
        "/((?!api/|agui/|oauth2/|_next/static|_next/image|favicon.ico|icon.svg|apple-icon.png|manifest.webmanifest).*)",
      missing: [
        { type: "header", key: "next-router-prefetch" },
        { type: "header", key: "purpose", value: "prefetch" },
      ],
    },
  ],
};
