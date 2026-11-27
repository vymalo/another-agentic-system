// The `::: in-depth [title]` container: a native <details>, closed by default, that holds the part of a page
// a reader may skip. Self-contained (no plugin): copy this file and `extras.css` to another VitePress site,
// then `md.use(inDepth)` in `markdown.config`. See docs/.vitepress/STRUCTURE.md, "Writing pages".
//
//   ::: in-depth The whole lifecycle
//   Text, tables, code, and another `::: in-depth` block (nesting works at the same marker length).
//   :::
//
// Rendered: <details class="in-depth"><summary>title</summary><div class="in-depth-body">…</div></details>
const NAME = "in-depth";
const DEFAULT_TITLE = "In depth";

export function inDepth(md: any) {
  const parse = (state: any, line: number) => {
    const start = state.bMarks[line] + state.tShift[line];
    const max = state.eMarks[line];
    if (state.sCount[line] - state.blkIndent >= 4) return null; // an indented code block
    const text = state.src.slice(start, max);
    const m = /^(:{3,})\s*(.*?)\s*$/.exec(text);
    return m ? { marks: m[1].length, rest: m[2] } : null;
  };

  md.block.ruler.before(
    "fence",
    "in_depth",
    (state: any, startLine: number, endLine: number, silent: boolean) => {
      const open = parse(state, startLine);
      if (!open) return false;
      const head = /^in-depth(?:\s+(.*))?$/.exec(open.rest);
      if (!head) return false;
      if (silent) return true;

      // Find the closing line: a bare marker at least as long as the opening one, past any nested opener.
      let depth = 1;
      let next = startLine + 1;
      let closed = false;
      let fence: string | null = null; // inside a fenced code block, `:::` is code
      for (; next < endLine; next++) {
        const s = state.bMarks[next] + state.tShift[next];
        const line = state.src.slice(s, state.eMarks[next]);
        const f = /^(`{3,}|~{3,})/.exec(line);
        if (f) {
          if (fence === null) fence = f[1][0];
          else if (f[1][0] === fence) fence = null;
          continue;
        }
        if (fence !== null) continue;
        const p = parse(state, next);
        if (!p) continue;
        if (p.rest === "") {
          if (p.marks >= open.marks && --depth === 0) {
            closed = true;
            break;
          }
        } else if (/^in-depth(\s|$)/.test(p.rest)) depth++;
      }

      const openTok = state.push("in_depth_open", "details", 1);
      openTok.block = true;
      openTok.markup = ":".repeat(open.marks);
      openTok.info = (head[1] ?? "").trim() || DEFAULT_TITLE;
      openTok.map = [startLine, next];

      const oldParent = state.parentType;
      const oldLineMax = state.lineMax;
      state.parentType = "container";
      state.lineMax = next;
      state.md.block.tokenize(state, startLine + 1, next);
      state.parentType = oldParent;
      state.lineMax = oldLineMax;

      const closeTok = state.push("in_depth_close", "details", -1);
      closeTok.block = true;
      closeTok.markup = openTok.markup;
      state.line = next + (closed ? 1 : 0);
      return true;
    },
    { alt: ["paragraph", "reference", "blockquote", "list"] },
  );

  md.renderer.rules.in_depth_open = (tokens: any, i: number) =>
    `<details class="in-depth"><summary>${md.utils.escapeHtml(tokens[i].info)}</summary><div class="in-depth-body">\n`;
  md.renderer.rules.in_depth_close = () => "</div></details>\n";
}
