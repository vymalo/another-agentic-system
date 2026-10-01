/**
 * Draws a mermaid graph to an SVG string. The only file that imports `mermaid`, and it does so with
 * a dynamic `import()` inside `renderMermaid`: the library (about 120 MB of diagram types, in
 * separate chunks of their own) is fetched the first time a graph is drawn and never for a page
 * that has none, so the code every thread loads does not grow. Configuration and what is done with
 * the SVG are in `mermaid.ts` (pure).
 */

let queue: Promise<unknown> = Promise.resolve();
let counter = 0;

/**
 * The SVG of `code`, drawn with `config` (`mermaidConfig`). Calls run one at a time: mermaid keeps
 * one global configuration, and `initialize` followed by `render` must not be interleaved with
 * another call's. A graph that does not parse, or a library that cannot be loaded, is a rejection.
 */
export function renderMermaid(code: string, config: Record<string, unknown>): Promise<string> {
  const run = async (): Promise<string> => {
    const { default: mermaid } = await import("mermaid");
    mermaid.initialize(config);
    const { svg } = await mermaid.render(`mmd-${++counter}`, code);
    return svg;
  };
  const result = queue.then(run, run);
  queue = result.catch(() => undefined);
  return result;
}
