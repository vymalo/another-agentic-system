"use client";

import { type TextMessagePartProps, useMessagePartText } from "@assistant-ui/react";
import {
  type CodeHeaderProps,
  MarkdownTextPrimitive,
  unstable_memoizeMarkdownComponents as memoizeMarkdownComponents,
  useIsMarkdownCodeBlock,
} from "@assistant-ui/react-markdown";
import { CheckIcon, CopyIcon, ImageIcon } from "lucide-react";
import {
  type ComponentProps,
  type ComponentPropsWithoutRef,
  createContext,
  type FC,
  memo,
  type RefObject,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import remarkGfm from "remark-gfm";

import { SyntaxHighlighter } from "@/components/assistant-ui/elements/syntax-highlighter";
import { TooltipIconButton } from "@/components/assistant-ui/elements/tooltip-icon-button";
import { FileImage } from "@/features/chat/components/cards/kept-file-card";
import { useSharedImage } from "@/features/chat/hooks/use-inline-images";
import { safeHttpUrl } from "@/features/chat/lib/a2ui/url";
import { plainName } from "@/features/chat/lib/files";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import { cn } from "@/lib/utils";

type MarkdownTextProps = Partial<TextMessagePartProps> & {
  components?: Parameters<typeof memoizeMarkdownComponents>[0];
};

const useShallowStable = <T extends Record<string, unknown> | undefined>(value: T): T => {
  const ref = useRef(value);
  if (value !== ref.current) {
    const prev = ref.current;
    const stable =
      value !== undefined &&
      prev !== undefined &&
      Object.keys(prev).length === Object.keys(value).length &&
      Object.keys(value).every((key) => prev[key] === value[key]);
    if (!stable) ref.current = value;
  }
  return ref.current;
};

type PrimitiveProps = ComponentProps<typeof MarkdownTextPrimitive>;
type Remark = NonNullable<PrimitiveProps["remarkPlugins"]>;
type Rehype = NonNullable<PrimitiveProps["rehypePlugins"]>;
type MathModule = typeof import("@/lib/math");

const NO_REHYPE: Rehype = [];
const GFM: Remark = [remarkGfm];

/**
 * Whether a text can have display math in it: a line that starts with `$$` (after the marks of a quote or a list item).
 * `$$` in the middle of a sentence is text (src/lib/remark-display-math.ts), and a single dollar is a price.
 */
const DISPLAY_MATH = /^[ \t>*+-]*(?:\d+[.)])?[ \t]*\$\$/m;
const hasMath = (text: string) => DISPLAY_MATH.test(text);

let loadedMath: MathModule | undefined;

/**
 * Math is its own chunk: KaTeX is 77 KB gzipped and most threads have no formula. It is fetched when a text first
 * holds `$$`, and until it is there (or if it never comes) the text is shown as it was written.
 */
function useMath(text: string): MathModule | undefined {
  const [math, setMath] = useState(loadedMath);
  const wanted = !math && hasMath(text);
  useEffect(() => {
    if (!wanted) return;
    let current = true;
    import("@/lib/math").then(
      (module) => {
        loadedMath = module;
        if (current) setMath(module);
      },
      () => {
        // the text stays text
      },
    );
    return () => {
      current = false;
    };
  }, [wanted]);
  return math;
}

/**
 * A display formula wider than the column scrolls sideways inside its own box. It is a tab stop (and named, for a screen
 * reader that lands on it) only while it overflows: a formula that fits has nothing to scroll, and a tab stop with
 * nothing to do is noise. Its width changes with the column and when KaTeX's fonts arrive, so it is measured again then.
 */
const INNER = ".katex-display > .katex";

function useScrollableFormulas(root: RefObject<HTMLElement | null>) {
  useEffect(() => {
    const el = root.current;
    if (!el) return;
    const sync = () => {
      for (const formula of el.querySelectorAll<HTMLElement>(".katex-display")) {
        if (formula.scrollWidth > formula.clientWidth) {
          formula.setAttribute("tabindex", "0");
          formula.setAttribute("role", "group");
          formula.setAttribute("aria-label", "Formula");
        } else {
          formula.removeAttribute("tabindex");
          formula.removeAttribute("role");
          formula.removeAttribute("aria-label");
        }
      }
    };
    const resize = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(sync);
    const watch = () => {
      sync();
      if (resize) for (const inner of el.querySelectorAll(INNER)) resize.observe(inner);
    };
    resize?.observe(el);
    watch();
    const mutate = new MutationObserver(watch);
    mutate.observe(el, { childList: true, subtree: true });
    document.fonts?.ready.then(sync, () => {});
    return () => {
      resize?.disconnect();
      mutate.disconnect();
    };
  }, [root]);
}

/** The box of the markdown (`.aui-md`): a div, which also watches its formulas. */
const MarkdownContainer: FC<ComponentPropsWithoutRef<"div">> = (props) => {
  const root = useRef<HTMLDivElement>(null);
  useScrollableFormulas(root);
  return <div {...props} ref={root} />;
};

const MarkdownTextImpl: FC<MarkdownTextProps> = ({ components }) => {
  const math = useMath(useMessagePartText().text);
  const remarkPlugins = useMemo(
    () => (math ? [remarkGfm, ...math.remarkPlugins] : GFM) as Remark,
    [math],
  );
  const rehypePlugins = (math?.rehypePlugins ?? NO_REHYPE) as Rehype;
  const stableComponents = useShallowStable(components);
  const markdownComponents = useMemo(() => {
    if (!stableComponents) return defaultComponents;
    return {
      ...defaultComponents,
      ...memoizeMarkdownComponents(stableComponents),
    };
  }, [stableComponents]);

  return (
    <MarkdownTextPrimitive
      containerComponent={MarkdownContainer}
      remarkPlugins={remarkPlugins}
      rehypePlugins={rehypePlugins}
      className="aui-md"
      components={markdownComponents}
      smooth={false}
      defer
    />
  );
};

export const MarkdownText = memo(MarkdownTextImpl);

const CodeHeader: FC<CodeHeaderProps> = ({ language, code }) => {
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const onCopy = () => {
    if (!code || isCopied) return;
    copyToClipboard(code);
  };

  return (
    <div className="aui-code-header-root border-border bg-muted mt-4 flex items-center justify-between rounded-t-xl border border-b-0 py-1 ps-3.5 pe-1.5 text-xs">
      <span className="aui-code-header-language text-muted-foreground font-medium lowercase">
        {language}
      </span>
      <TooltipIconButton tooltip="Copy" onClick={onCopy}>
        {!isCopied && <CopyIcon className="animate-in zoom-in-75 fade-in duration-150" />}
        {isCopied && <CheckIcon className="animate-in zoom-in-50 fade-in duration-200 ease-out" />}
      </TooltipIconButton>
    </div>
  );
};

/** Whether a markdown element sits inside a link (an image there must not be a link too). */
const InLink = createContext(false);

/*
 * The box of a code block. It is NOT memoized with the others: `memoizeMarkdownComponents` compares the hast node and
 * ignores the children, and the syntax highlighter changes a block's children (plain text, then coloured tokens)
 * without the node ever changing, so a memoized box would keep showing the first.
 */
const CodePre: FC<ComponentPropsWithoutRef<"pre"> & { node?: unknown }> = ({
  className,
  node: _node,
  ...props
}) => (
  <pre
    className={cn(
      "aui-md-pre border-border bg-muted/40 text-foreground mb-4 overflow-x-auto rounded-t-none rounded-b-xl border border-t-0 p-3.5 font-mono text-[13px] leading-relaxed last:mb-0",
      className,
    )}
    {...props}
  />
);

const memoizedComponents = memoizeMarkdownComponents({
  h1: ({ className, ...props }) => (
    <h1
      className={cn(
        "aui-md-h1 mt-5 mb-2 scroll-m-20 text-xl font-semibold first:mt-0 last:mb-0",
        className,
      )}
      {...props}
    />
  ),
  h2: ({ className, ...props }) => (
    <h2
      className={cn(
        "aui-md-h2 mt-5 mb-2 scroll-m-20 text-lg font-semibold first:mt-0 last:mb-0",
        className,
      )}
      {...props}
    />
  ),
  h3: ({ className, ...props }) => (
    <h3
      className={cn(
        "aui-md-h3 mt-4 mb-1.5 scroll-m-20 text-base font-semibold first:mt-0 last:mb-0",
        className,
      )}
      {...props}
    />
  ),
  h4: ({ className, ...props }) => (
    <h4
      className={cn(
        "aui-md-h4 mt-3.5 mb-1 scroll-m-20 text-base font-medium first:mt-0 last:mb-0",
        className,
      )}
      {...props}
    />
  ),
  h5: ({ className, ...props }) => (
    <h5
      className={cn("aui-md-h5 mt-3 mb-1 text-sm font-semibold first:mt-0 last:mb-0", className)}
      {...props}
    />
  ),
  h6: ({ className, ...props }) => (
    <h6
      className={cn("aui-md-h6 mt-3 mb-1 text-sm font-medium first:mt-0 last:mb-0", className)}
      {...props}
    />
  ),
  p: ({ className, ...props }) => (
    <p className={cn("aui-md-p my-3 leading-7 first:mt-0 last:mb-0", className)} {...props} />
  ),
  // Agent text is untrusted: every link opens in a new tab without an opener. Raw HTML stays off
  // (react-markdown's default), and react-markdown drops javascript: and data: URLs.
  a: ({ className, children, ...props }) => (
    <a
      className={cn(
        "aui-md-a text-brand hover:text-brand/80 underline underline-offset-2",
        className,
      )}
      target="_blank"
      rel="noopener noreferrer"
      {...props}
    >
      <InLink.Provider value={true}>{children}</InLink.Provider>
    </a>
  ),
  // Agent text is untrusted, and an image is a request the browser makes on its own (a URL can
  // carry what the agent read): a remote image is never drawn. It is its alt text and, when its URL
  // is http(s), a link a person may follow. A path that means a file the agent shared in this
  // thread (`share_file`) is drawn from that file's own route, and from nothing the agent wrote;
  // a path that means none is its alt text with an icon, never a broken image and never a request.
  img: function Img({ src, alt }) {
    // inside a link (`[![x](img)](page)`) it is text: a link in a link is no link
    const inLink = useContext(InLink);
    const source = typeof src === "string" ? src : undefined;
    const shared = useSharedImage(source);
    if (shared) {
      return (
        <span data-slot="md-image-file" className="my-2 block first:mt-0 last:mb-0">
          <FileImage file={shared} alt={plainName(alt ?? "", 300) || undefined} inline />
        </span>
      );
    }
    const href = inLink ? undefined : safeHttpUrl(source);
    const label = alt?.trim() || "image";
    const content = (
      <>
        <ImageIcon aria-hidden="true" className="me-1 inline size-3.5 align-[-0.15em]" />
        {label}
      </>
    );
    return href ? (
      <a
        data-slot="md-image-link"
        href={href}
        target="_blank"
        rel="noopener noreferrer"
        className="aui-md-a text-brand hover:text-brand/80 underline underline-offset-2"
      >
        {content} <span className="sr-only">(image, opens in a new tab)</span>
      </a>
    ) : (
      <span data-slot="md-image-text" className="text-muted-foreground">
        {content}
        <span className="sr-only"> (image not shown)</span>
      </span>
    );
  },
  blockquote: ({ className, ...props }) => (
    <blockquote
      className={cn(
        "aui-md-blockquote border-muted-foreground/30 text-muted-foreground my-3 border-s-2 ps-4",
        className,
      )}
      {...props}
    />
  ),
  ul: ({ className, ...props }) => (
    <ul
      className={cn(
        "aui-md-ul marker:text-muted-foreground my-3 ms-5 list-disc [&>li]:mt-1.5",
        className,
      )}
      {...props}
    />
  ),
  ol: ({ className, ...props }) => (
    <ol
      className={cn(
        "aui-md-ol marker:text-muted-foreground my-3 ms-5 list-decimal [&>li]:mt-1",
        className,
      )}
      {...props}
    />
  ),
  hr: ({ className, ...props }) => (
    <hr className={cn("aui-md-hr border-muted-foreground/20 my-3", className)} {...props} />
  ),
  table: ({ className, ...props }) => (
    <div className="aui-md-table-wrapper my-3 overflow-x-auto">
      <table
        className={cn("aui-md-table w-full border-separate border-spacing-0", className)}
        {...props}
      />
    </div>
  ),
  th: ({ className, ...props }) => (
    <th
      className={cn(
        "aui-md-th bg-muted text-foreground px-3 py-1.5 text-start font-medium first:rounded-ss-lg last:rounded-se-lg [[align=center]]:text-center [[align=right]]:text-right",
        className,
      )}
      {...props}
    />
  ),
  td: ({ className, ...props }) => (
    <td
      className={cn(
        "aui-md-td border-muted-foreground/20 border-s border-b px-3 py-1.5 text-start last:border-e [[align=center]]:text-center [[align=right]]:text-right",
        className,
      )}
      {...props}
    />
  ),
  tr: ({ className, ...props }) => (
    <tr
      className={cn(
        "aui-md-tr m-0 border-b p-0 first:border-t [&:last-child>td:first-child]:rounded-es-lg [&:last-child>td:last-child]:rounded-ee-lg",
        className,
      )}
      {...props}
    />
  ),
  li: ({ className, ...props }) => (
    <li className={cn("aui-md-li leading-7", className)} {...props} />
  ),
  strong: ({ className, ...props }) => (
    <strong className={cn("aui-md-strong font-semibold", className)} {...props} />
  ),
  sup: ({ className, ...props }) => (
    <sup className={cn("aui-md-sup [&>a]:text-xs [&>a]:no-underline", className)} {...props} />
  ),
  code: function Code({ className, ...props }) {
    const isCodeBlock = useIsMarkdownCodeBlock();
    return (
      <code
        className={cn(
          !isCodeBlock &&
            "aui-md-inline-code bg-muted text-foreground rounded-md px-1.5 py-0.5 font-mono text-[0.85em]",
          className,
        )}
        {...props}
      />
    );
  },
  CodeHeader,
  SyntaxHighlighter,
});

const defaultComponents = { ...memoizedComponents, pre: CodePre };
