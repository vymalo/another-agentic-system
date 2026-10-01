"use client";

import { type KeyboardEvent, type PointerEvent, useRef } from "react";
import { Sheet, SheetContent, SheetHeader, SheetTitle } from "@/components/ui/sheet";
import { cn } from "@/lib/utils";
import { type PanelController, usePanel } from "../hooks/use-panel";
import { focusTurn } from "../lib/jump";
import { PANEL_MIN_WIDTH } from "../lib/panel-state";
import { PanelBody } from "./panel-body";

/** How far an arrow key moves the edge, in px. */
const KEY_STEP = 16;

/**
 * The edge between the chat and the docked panel, as a window splitter: a vertical separator the
 * keyboard can focus (Left widens the panel, Right narrows it, Home and End go to the narrowest
 * and the widest) and a pointer can drag. It reports its value as the panel's width in px.
 */
function ResizeHandle({ panel }: { panel: PanelController }) {
  const drag = useRef<{ startX: number; startWidth: number; last: number } | null>(null);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const next =
      e.key === "ArrowLeft"
        ? panel.width + KEY_STEP
        : e.key === "ArrowRight"
          ? panel.width - KEY_STEP
          : e.key === "Home"
            ? PANEL_MIN_WIDTH
            : e.key === "End"
              ? panel.maxWidth
              : null;
    if (next === null) return;
    e.preventDefault();
    panel.setWidth(next);
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.currentTarget.setPointerCapture?.(e.pointerId);
    drag.current = { startX: e.clientX, startWidth: panel.width, last: panel.width };
    document.documentElement.dataset.panelDragging = "";
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    // the panel is on the right: dragging the edge to the left widens it
    d.last = d.startWidth + (d.startX - e.clientX);
    panel.setWidth(d.last, false);
  };
  const endDrag = () => {
    const d = drag.current;
    if (!d) return;
    drag.current = null;
    delete document.documentElement.dataset.panelDragging;
    panel.setWidth(d.last, true);
  };

  return (
    // biome-ignore lint/a11y/useSemanticElements: a window splitter is a focusable separator, which <hr> is not
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize the details panel"
      aria-valuemin={PANEL_MIN_WIDTH}
      aria-valuemax={panel.maxWidth}
      aria-valuenow={panel.width}
      tabIndex={0}
      onKeyDown={onKeyDown}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      data-slot="panel-resize"
      className="absolute inset-y-0 start-0 z-10 w-1.5 cursor-col-resize touch-none bg-transparent transition-colors outline-none hover:bg-brand/50 focus-visible:bg-brand"
    />
  );
}

/**
 * Wide windows: a column beside the chat, between the sidebar and the edge. It stays in the page when
 * closed (width 0, inert, hidden from the accessibility tree) so the toggle's `aria-controls` is
 * valid and the width can animate; its width is `--panel-width`, which the stylesheet also reads
 * before React has run (`PANEL_SCRIPT`, globals.css).
 */
function DockedPanel({ panel }: { panel: PanelController }) {
  const { open } = panel;
  return (
    <aside
      id={panel.panelId}
      data-slot="panel"
      data-state={open ? "open" : "closed"}
      aria-label="Thread details"
      aria-hidden={open ? undefined : true}
      inert={!open}
      className={cn(
        "relative min-h-0 shrink-0 overflow-hidden bg-background motion-safe:transition-[width] motion-safe:duration-200 motion-safe:ease-out",
        open
          ? "w-(--panel-width,22.5rem) max-w-[min(35rem,45vw,calc(100vw-52rem))] border-s"
          : "w-0 border-s-0",
      )}
    >
      <div
        className={cn(
          "flex h-full w-(--panel-width,22.5rem) max-w-[min(35rem,45vw,calc(100vw-52rem))] flex-col",
          !open && "invisible",
        )}
      >
        <h2 className="sr-only">Thread details</h2>
        <ResizeHandle panel={panel} />
        <PanelBody onClose={() => panel.setOpen(false)} />
      </div>
    </aside>
  );
}

/**
 * Narrower windows: the same panel in a sheet, from the right on a tablet and from the bottom on
 * a phone. A sheet is a dialog: it takes the focus, Escape closes it and the focus goes back to
 * what opened it; unless the person asked to see a turn, where it goes instead.
 */
function PanelSheet({ panel }: { panel: PanelController }) {
  const bottom = panel.layout === "sheet-bottom";
  return (
    <Sheet open={panel.open} onOpenChange={panel.setOpen}>
      <SheetContent
        id={panel.panelId}
        side={bottom ? "bottom" : "right"}
        showCloseButton={false}
        aria-describedby={undefined}
        onCloseAutoFocus={(event) => {
          // the dialog has no trigger of its own to give the focus back to: we say where it goes
          event.preventDefault();
          const turn = panel.takePendingTurn();
          if (turn === null) panel.returnFocus();
          else focusTurn(turn);
        }}
        className={cn(
          "gap-0 p-0",
          bottom
            ? "rounded-t-2xl data-[side=bottom]:h-[85dvh]"
            : "data-[side=right]:w-[min(26rem,92vw)] data-[side=right]:sm:max-w-none",
        )}
      >
        <SheetHeader className="sr-only">
          <SheetTitle>Thread details</SheetTitle>
        </SheetHeader>
        <PanelBody onClose={() => panel.setOpen(false)} />
      </SheetContent>
    </Sheet>
  );
}

/** The right-hand panel of a thread: docked beside the chat when there is room, else a sheet. */
export function ThreadPanel() {
  const panel = usePanel();
  if (!panel) return null;
  return panel.layout === "sheet-right" || panel.layout === "sheet-bottom" ? (
    <PanelSheet panel={panel} />
  ) : (
    <DockedPanel panel={panel} />
  );
}
