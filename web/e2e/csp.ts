import type { Page } from "@playwright/test";

/**
 * Every content security policy violation the page reports (ADR 0054, decision 10), from the
 * `securitypolicyviolation` event and from the console, which is where a blocked inline script lands
 * before any handler exists. Call before the first `goto`.
 */
export async function watchCsp(page: Page): Promise<() => Promise<string[]>> {
  const violations: string[] = [];
  page.on("console", (m) => {
    if (/Content Security Policy|Refused to/i.test(m.text()))
      violations.push(`console: ${m.text()}`);
  });
  await page.addInitScript(() => {
    const seen: string[] = [];
    (window as unknown as { __csp: string[] }).__csp = seen;
    document.addEventListener("securitypolicyviolation", (e) => {
      seen.push(`${e.violatedDirective} ${e.blockedURI} ${e.sourceFile}:${e.lineNumber}`);
    });
  });
  return async () => {
    const inPage = await page
      .evaluate(() => (window as unknown as { __csp?: string[] }).__csp ?? [])
      .catch(() => []);
    return [...violations, ...inPage];
  };
}
