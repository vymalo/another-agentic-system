"use client";

import {
  type GenerativeUIComponent,
  type GenerativeUILibrary,
  renderGenerativeUI,
} from "@assistant-ui/react-generative-ui";
import { ImageOffIcon } from "lucide-react";
import {
  Component,
  createContext,
  type ErrorInfo,
  type ReactNode,
  useContext,
  useId,
  useMemo,
  useState,
} from "react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import { contextTooLarge, resolveFields } from "@/features/chat/lib/a2ui/context";
import {
  CHECK_BOX,
  MAX_CONTEXT_BYTES,
  OPEN_URL,
  TEXT_FIELD,
  UNSUPPORTED,
  USER_MESSAGE,
} from "@/features/chat/lib/a2ui/limits";
import type { Prepared } from "@/features/chat/lib/a2ui/prepare";
import { safeHttpUrl } from "@/features/chat/lib/a2ui/url";
import { isTerminal } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { useSurfaceHost } from "./surface-host";

/*
 * The shadcn vocabulary of A2UI surfaces, in the format `JSONGenerativeUI({ library })` takes
 * (`GenerativeUILibrary`), and the view that draws a validated surface with it.
 *
 * What reaches this file has passed `prepareSurface` (lib/a2ui/prepare.ts): a component outside the
 * vocabulary, an oversize or too deep surface and a bad URL never get here. What is still decided
 * here is what a component DOES, and the rules are ADR 0013's:
 *  - text is React text (no markdown, no HTML), and an image is never fetched (its alt text shows);
 *  - a control acts only in its own click handler: nothing sends on render, on an update or on a
 *    timer;
 *  - `openUrl` is a plain link (`target="_blank" rel="noopener noreferrer"`) to a URL checked
 *    twice; a `userMessage` goes to the message box unsent; anything else the agent could ask for
 *    is a disabled button.
 */

type ViewState = {
  surfaceId: string;
  /** The newest copy of the surface: an older copy is read-only. */
  live: boolean;
  values: Readonly<Record<string, unknown>>;
  setValue: (key: string, value: unknown) => void;
};
const ViewCtx = createContext<ViewState | null>(null);
const useView = (): ViewState => {
  const view = useContext(ViewCtx);
  if (!view) throw new Error("a surface component outside a surface");
  return view;
};

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const text = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const gap = (v: unknown): { gap: string } | undefined =>
  typeof v === "number" && Number.isFinite(v)
    ? { gap: `${Math.min(Math.max(v, 0), 32)}px` }
    : undefined;

const ALIGN: Record<string, string> = {
  start: "items-start",
  center: "items-center",
  end: "items-end",
};
const JUSTIFY: Record<string, string> = {
  start: "justify-start",
  center: "justify-center",
  end: "justify-end",
  between: "justify-between",
};

type Action = { name?: string; sourceComponentId?: string; context?: Rec } & Rec;

function SurfaceButton({
  label,
  buttonStyle,
  block,
  $action,
  children,
}: {
  label?: unknown;
  buttonStyle?: unknown;
  block?: unknown;
  $action?: Action;
  children?: ReactNode;
}) {
  const view = useView();
  const host = useSurfaceHost();
  const variant =
    buttonStyle === "primary" ? "default" : buttonStyle === "ghost" ? "ghost" : "outline";
  const className = cn("h-auto min-h-9 whitespace-normal", block === true && "w-full");
  const content = text(label) ?? children;
  const name = $action?.name;

  if (name === OPEN_URL) {
    // A link, not a script: the target was validated, and is validated again here.
    const href = safeHttpUrl($action?.context?.url);
    if (href) {
      return (
        <Button asChild variant={variant} className={className}>
          <a href={href} target="_blank" rel="noopener noreferrer">
            {content}
          </a>
        </Button>
      );
    }
    return (
      <Button type="button" variant={variant} className={className} disabled>
        {content}
      </Button>
    );
  }

  if (name === USER_MESSAGE) {
    const draft = text($action?.context?.text);
    return (
      <Button
        type="button"
        variant={variant}
        className={className}
        disabled={!view.live || !host.canCompose || draft === undefined}
        onClick={() => {
          if (draft !== undefined) host.fillComposer(draft);
        }}
      >
        {content}
      </Button>
    );
  }

  const sendable =
    $action !== undefined && name !== undefined && name !== UNSUPPORTED && typeof name === "string";
  return (
    <Button
      type="button"
      variant={variant}
      className={className}
      disabled={!sendable || !view.live || !host.canSend}
      onClick={() => {
        if (!sendable) return;
        const context = resolveFields($action?.context ?? {}, view.values);
        if (!isRecord(context) || contextTooLarge(context)) {
          host.reject(`The action carries more than ${MAX_CONTEXT_BYTES / 1024} KiB of context.`);
          return;
        }
        host.send({
          name: name as string,
          surfaceId: view.surfaceId,
          sourceComponentId: text($action?.sourceComponentId) ?? "",
          context,
        });
      }}
    >
      {content}
    </Button>
  );
}

function TextFieldInput({
  label,
  fieldKey,
  variant,
}: {
  label?: unknown;
  fieldKey?: unknown;
  variant?: unknown;
}) {
  const view = useView();
  const id = useId();
  const key = text(fieldKey) ?? id;
  const value = view.values[key];
  const kind = variant === "obscured" ? "password" : variant === "number" ? "number" : "text";
  const caption = text(label);
  const props = {
    id,
    value: value === undefined || value === null ? "" : String(value),
    disabled: !view.live,
    onChange: (e: { target: { value: string } }) => view.setValue(key, e.target.value),
  };
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      {caption ? (
        <label htmlFor={id} className="text-sm font-medium">
          {caption}
        </label>
      ) : null}
      {variant === "longText" ? (
        <textarea
          {...props}
          rows={3}
          aria-label={caption ? undefined : "Text"}
          className="min-h-16 w-full min-w-0 rounded-md border border-input bg-transparent px-2.5 py-1.5 text-base shadow-xs outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 md:text-sm dark:bg-input/30"
        />
      ) : (
        <Input {...props} type={kind} aria-label={caption ? undefined : "Text"} />
      )}
    </div>
  );
}

function CheckBoxInput({ label, fieldKey }: { label?: unknown; fieldKey?: unknown }) {
  const view = useView();
  const id = useId();
  const key = text(fieldKey) ?? id;
  return (
    <div className="flex items-center gap-2">
      <input
        id={id}
        type="checkbox"
        className="size-4 accent-primary disabled:cursor-not-allowed disabled:opacity-50"
        checked={view.values[key] === true}
        disabled={!view.live}
        onChange={(e) => view.setValue(key, e.target.checked)}
        aria-label={text(label) ? undefined : "Option"}
      />
      {text(label) ? (
        <label htmlFor={id} className="text-sm">
          {text(label)}
        </label>
      ) : null}
    </div>
  );
}

const entry = (render: (props: never) => ReactNode): GenerativeUIComponent =>
  // `properties` (a zod schema) only builds the parameters of a model's `present` tool, and no
  // model runs in the browser (ADR 0013): this library is rendered, never offered to one.
  ({ description: "", render }) as unknown as GenerativeUIComponent;

type P = { children?: ReactNode } & Rec;

/** The vocabulary: a `$type` of the converter's output (and of our own inputs) to a component. */
export const surfaceLibrary: GenerativeUILibrary = {
  Header: entry(({ text: t }: P) => (
    // biome-ignore lint/a11y/useSemanticElements: the level is the surface's, not the page's
    <p role="heading" aria-level={3} className="text-base font-semibold [overflow-wrap:anywhere]">
      {text(t)}
    </p>
  )),
  Markdown: entry(({ value }: P) => (
    // A2UI `Text` is markdown-flavoured; it is drawn as plain text: no HTML and no link or image
    // from text (ADR 0013 rules 5 and 6).
    <p className="text-sm whitespace-pre-wrap [overflow-wrap:anywhere]">{text(value)}</p>
  )),
  Caption: entry(({ value }: P) => (
    <p className="text-xs whitespace-pre-wrap text-muted-foreground [overflow-wrap:anywhere]">
      {text(value)}
    </p>
  )),
  Image: entry(({ alt }: P) => (
    <div className="flex items-center gap-2 rounded-md border border-dashed px-2.5 py-2 text-xs text-muted-foreground">
      <ImageOffIcon aria-hidden="true" className="size-4 shrink-0" />
      <span className="[overflow-wrap:anywhere]">
        Image not shown{text(alt) ? `: ${text(alt)}` : ""}
      </span>
    </div>
  )),
  Row: entry(({ gap: g, align, justify, children }: P) => (
    <div
      className={cn(
        "flex min-w-0 flex-row flex-wrap gap-2",
        ALIGN[String(align)] ?? "items-center",
        JUSTIFY[String(justify)],
      )}
      style={gap(g)}
    >
      {children}
    </div>
  )),
  Col: entry(({ gap: g, align, children }: P) => (
    <div className={cn("flex min-w-0 flex-col gap-2", ALIGN[String(align)])} style={gap(g)}>
      {children}
    </div>
  )),
  ListView: entry(({ children }: P) => (
    // biome-ignore lint/a11y/noRedundantRoles: without a bullet Safari would drop the list role
    <ul role="list" className="m-0 flex min-w-0 list-none flex-col gap-2 p-0">
      {children}
    </ul>
  )),
  ListViewItem: entry(({ children }: P) => <li className="min-w-0">{children}</li>),
  Card: entry(({ title, children }: P) => (
    <Card size="sm" className="min-w-0 shadow-none">
      {text(title) ? (
        <CardHeader>
          <CardTitle className="[overflow-wrap:anywhere]">{text(title)}</CardTitle>
        </CardHeader>
      ) : null}
      <CardContent className="flex min-w-0 flex-col gap-2">{children}</CardContent>
    </Card>
  )),
  Divider: entry(() => <Separator />),
  Button: entry(SurfaceButton as (props: never) => ReactNode),
  [TEXT_FIELD]: entry(TextFieldInput as (props: never) => ReactNode),
  [CHECK_BOX]: entry(CheckBoxInput as (props: never) => ReactNode),
};

class Boundary extends Component<
  { children: ReactNode; fallback: ReactNode },
  { failed: boolean }
> {
  override state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  override componentDidCatch(error: Error, info: ErrorInfo) {
    console.warn("An A2UI surface failed to render", error, info.componentStack);
  }
  override render() {
    return this.state.failed ? this.props.fallback : this.props.children;
  }
}

/** What a control that cannot act says, once per surface. */
function hint(canSend: boolean, finished: boolean): string | null {
  if (canSend) return null;
  return finished
    ? "This thread is finished: the actions of this interface are off."
    : "The actions of this interface work while the thread waits for you.";
}

/** A validated surface, drawn. `prepared.kind` is "surface" (the caller decided). */
export function SurfaceView({
  prepared,
  live,
  fallback,
}: {
  prepared: Extract<Prepared, { kind: "surface" }>;
  live: boolean;
  fallback: ReactNode;
}) {
  const host = useSurfaceHost();
  const [edits, setEdits] = useState<Record<string, unknown>>({});
  const view = useMemo<ViewState>(
    () => ({
      surfaceId: prepared.surfaceId,
      live,
      values: { ...prepared.fields, ...edits },
      setValue: (key, value) => setEdits((e) => ({ ...e, [key]: value })),
    }),
    [prepared.surfaceId, prepared.fields, live, edits],
  );
  const note =
    live && prepared.eventActions > 0 ? hint(host.canSend, isTerminal(host.state)) : null;
  return (
    <ViewCtx.Provider value={view}>
      <Boundary fallback={fallback}>
        <div className="flex min-w-0 flex-col gap-2">
          {renderGenerativeUI(prepared.spec, surfaceLibrary, { status: "done" })}
        </div>
      </Boundary>
      {note ? <p className="mt-2 text-xs text-muted-foreground">{note}</p> : null}
    </ViewCtx.Provider>
  );
}
