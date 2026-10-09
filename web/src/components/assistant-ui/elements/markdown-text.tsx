"use client";

import type { TextMessagePartProps } from "@assistant-ui/react";
import {
  type CodeHeaderProps,
  MarkdownTextPrimitive,
  unstable_memoizeMarkdownComponents as memoizeMarkdownComponents,
  useIsMarkdownCodeBlock,
} from "@assistant-ui/react-markdown";
import { CheckIcon, CopyIcon, ImageIcon } from "lucide-react";
import { createContext, type FC, memo, useContext, useMemo, useRef } from "react";
import remarkGfm from "remark-gfm";

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

const MarkdownTextImpl: FC<MarkdownTextProps> = ({ components }) => {
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
      remarkPlugins={[remarkGfm]}
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

const defaultComponents = memoizeMarkdownComponents({
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
  pre: ({ className, ...props }) => (
    <pre
      className={cn(
        "aui-md-pre border-border bg-muted/40 text-foreground mb-4 overflow-x-auto rounded-t-none rounded-b-xl border border-t-0 p-3.5 font-mono text-[13px] leading-relaxed last:mb-0",
        className,
      )}
      {...props}
    />
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
});
