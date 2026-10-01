"use client";

import { CircleAlertIcon } from "lucide-react";
import { type ReactNode, useEffect, useState, useSyncExternalStore } from "react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  mermaidConfig,
  readMermaid,
  reasonOf,
  type Scheme,
  type SvgImage,
  svgImage,
  tokensFrom,
} from "@/features/chat/lib/a2ui/mermaid";
import { renderMermaid } from "@/features/chat/lib/a2ui/mermaid-render";

/*
 * The `Mermaid` component of the UI catalog (docs/api/ui-catalog-v1.md, version 3): an agent's graph,
 * drawn as an IMAGE. mermaid runs in the page with `securityLevel: "strict"` and no HTML labels
 * (lib/a2ui/mermaid.ts), its SVG becomes the `src` of an `<img>` (a data URL), so nothing in a
 * graph can run, link or load anything (ADR 0013 rules 5 and 6). The library is imported when a graph
 * is first drawn (lib/a2ui/mermaid-render.ts), not with the page.
 *
 * Text alternative: the image's `alt` is the title (or caption), and the graph's source, which is
 * text, stays one click away under "Diagram source". A graph that cannot be drawn says so and shows its
 * source in place; it is never half drawn and never fails the surface.
 */

const DARK = "(prefers-color-scheme: dark)";
const media = () => (typeof window.matchMedia === "function" ? window.matchMedia(DARK) : undefined);

/** The scheme of the page: dark mode follows the system setting (globals.css), no toggle. */
function useScheme(): Scheme {
  return useSyncExternalStore(
    (notify) => {
      const query = media();
      query?.addEventListener("change", notify);
      return () => query?.removeEventListener("change", notify);
    },
    () => (media()?.matches ? "dark" : "light"),
    () => "light",
  );
}

/** What was drawn of one graph's source: a result belongs to the `code` it was drawn from. */
type Drawing =
  | { kind: "drawn"; code: string; image: SvgImage }
  | { kind: "failed"; code: string; reason: string };

function Source({ code }: { code: string }) {
  return (
    <pre
      // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must be reachable by keyboard
      tabIndex={0}
      data-slot="mermaid-code"
      className="max-h-64 overflow-auto rounded-md bg-muted p-2 text-[0.8125rem] whitespace-pre-wrap text-foreground [overflow-wrap:anywhere]"
    >
      {code}
    </pre>
  );
}

/** The Mermaid of a surface; the props are those of the lowered component (`prepare.ts`). */
export function MermaidDiagram(props: Record<string, unknown>): ReactNode {
  const spec = readMermaid(props);
  const scheme = useScheme();
  const code = spec?.code;
  const [result, setDrawing] = useState<Drawing | undefined>(undefined);

  useEffect(() => {
    if (code === undefined) return;
    let current = true;
    const style = getComputedStyle(document.documentElement);
    const config = mermaidConfig(
      tokensFrom((name) => style.getPropertyValue(name), scheme),
      scheme,
    );
    // a change of scheme draws again over the old picture, which stays until the new one is ready
    renderMermaid(code, config)
      .then((svg) => {
        const image = svgImage(svg);
        if (!current) return;
        setDrawing(
          image
            ? { kind: "drawn", code, image }
            : { kind: "failed", code, reason: "the drawing was not a picture this page may show" },
        );
      })
      .catch((error: unknown) => {
        if (current) setDrawing({ kind: "failed", code, reason: reasonOf(error) });
      });
    return () => {
      current = false;
    };
  }, [code, scheme]);

  if (!spec) return null;
  const { title, caption } = spec;
  // another source than the one that was drawn: draw again, with nothing of the old picture
  const drawing = result?.code === code ? result : undefined;
  return (
    <figure data-slot="mermaid" className="m-0 flex min-w-0 flex-col gap-2">
      {title ? (
        // biome-ignore lint/a11y/useSemanticElements: the level is the surface's, not the page's
        <p
          role="heading"
          aria-level={3}
          className="text-base font-semibold [overflow-wrap:anywhere]"
        >
          {title}
        </p>
      ) : null}
      {drawing?.kind === "failed" ? (
        <Alert variant="destructive" role="none" data-slot="mermaid-error">
          <CircleAlertIcon aria-hidden="true" />
          <AlertTitle>The graph could not be drawn.</AlertTitle>
          <AlertDescription className="flex flex-col gap-2">
            <span className="whitespace-pre-wrap [overflow-wrap:anywhere]">{drawing.reason}</span>
            <Source code={spec.code} />
          </AlertDescription>
        </Alert>
      ) : (
        <>
          {drawing?.kind === "drawn" ? (
            // an image, not markup: whatever is in the SVG cannot run, link or load anything
            // biome-ignore lint/performance/noImgElement: a data URL, nothing to optimise or fetch
            <img
              data-slot="mermaid-image"
              src={drawing.image.src}
              width={drawing.image.width}
              height={drawing.image.height}
              alt={title ?? caption ?? "Diagram"}
              draggable={false}
              onError={() =>
                setDrawing({
                  kind: "failed",
                  code: spec.code,
                  reason: "the browser could not show the drawing",
                })
              }
              className="h-auto max-w-full self-start"
            />
          ) : (
            <p
              aria-busy="true"
              className="flex min-h-24 items-center text-xs text-muted-foreground"
            >
              Drawing the graph…
            </p>
          )}
          <details data-slot="mermaid-source" className="text-sm">
            <summary className="cursor-pointer text-xs text-muted-foreground underline-offset-2 hover:underline">
              Diagram source
            </summary>
            <div className="mt-2">
              <Source code={spec.code} />
            </div>
          </details>
        </>
      )}
      {caption ? (
        <figcaption className="text-xs text-muted-foreground [overflow-wrap:anywhere]">
          {caption}
        </figcaption>
      ) : null}
    </figure>
  );
}
