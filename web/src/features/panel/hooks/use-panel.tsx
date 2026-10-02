"use client";

import {
  createContext,
  type ReactNode,
  type RefObject,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { focusTurn } from "../lib/jump";
import {
  clampWidth,
  maxPanelWidth,
  PANEL_DEFAULT_WIDTH,
  PANEL_ID,
  PANEL_KEY,
  PANEL_TAB_KEY,
  PANEL_WIDTH_KEY,
  type PanelLayout,
  type PanelTab,
  panelLayout,
  parseTab,
  parseWidth,
  resolveOpen,
} from "../lib/panel-state";
import { type StepsPanel, StepsPanelContext } from "./use-steps-panel";

/** What the panel's own components read; the step tree reads the smaller `useStepsPanel()`. */
export type PanelController = {
  panelId: string;
  /** How the panel is shown at this window width; null until the window is measured. */
  layout: PanelLayout | null;
  /** The panel is showing: docked and open, or its sheet is open. */
  open: boolean;
  tab: PanelTab;
  /** The docked panel's width in px, within the limits of the window. */
  width: number;
  maxWidth: number;
  /** The header's toggle: closing the panel from inside it puts the focus back there. */
  toggleRef: RefObject<HTMLButtonElement | null>;
  setOpen(open: boolean): void;
  toggle(): void;
  setTab(tab: PanelTab): void;
  /** `remember` keeps it for next time: not on every step of a drag, only at its end. */
  setWidth(width: number, remember?: boolean): void;
  /** Scrolls the chat to a turn and focuses its header; a sheet closes first. */
  showTurn(turnId: string): void;
  /** The turn a closing sheet was asked to show, once: its `onCloseAutoFocus` takes it. */
  takePendingTurn(): string | null;
  /**
   * Where the focus goes when a sheet closes: what had it when the sheet opened (the toggle, the
   * box the shortcut was pressed in), else the toggle. A Radix dialog without a trigger of its own
   * would drop it on the page.
   */
  returnFocus(): void;
};

const Context = createContext<PanelController | null>(null);

/** The panel's controller; null outside a thread (the new-chat page has no panel). */
export const usePanel = (): PanelController | null => useContext(Context);

function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null; // storage may be blocked: nothing is remembered
  }
}

function write(key: string, value: string) {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // not remembered, still done
  }
}

/** A sheet takes the focus away: remember where it was (the page's body is nowhere). */
function rememberOpener(into: { current: HTMLElement | null }) {
  const active = document.activeElement;
  into.current = active instanceof HTMLElement && active !== document.body ? active : null;
}

/** `<html data-panel="open">` is what the stylesheet docks by before React knows (PANEL_SCRIPT). */
function markDocked(open: boolean) {
  if (open) document.documentElement.dataset.panel = "open";
  else delete document.documentElement.dataset.panel;
}

/**
 * The right-hand panel's state for one thread: open or closed (docked: remembered per browser;
 * a sheet: this visit only), the tab and the width (remembered), what the step tree asked to show,
 * and the shortcut Ctrl/⌘+Shift+. that toggles it. The state starts as the server renders it
 * (closed, the narrowest layout unknown) and catches up in an effect; what the person sees before
 * that is `PANEL_SCRIPT` and the stylesheet.
 */
export function PanelProvider({ children }: { children: ReactNode }) {
  const [viewport, setViewport] = useState<number | null>(null);
  const [dockedOpen, setDockedOpen] = useState(false);
  const [sheetOpen, setSheetOpen] = useState(false);
  const [tab, setTabState] = useState<PanelTab>("activity");
  const [wanted, setWanted] = useState(PANEL_DEFAULT_WIDTH);
  const [focus, setFocus] = useState<StepsPanel["focus"]>(null);
  const [expansion, setExpansion] = useState<unknown>(undefined);
  const toggleRef = useRef<HTMLButtonElement | null>(null);
  const pendingTurn = useRef<string | null>(null);
  const opener = useRef<HTMLElement | null>(null);

  const layout = viewport === null ? null : panelLayout(viewport);
  const docked = layout === null || layout === "docked";
  const open = docked ? dockedOpen : sheetOpen;
  const width = clampWidth(wanted, viewport ?? 1440);
  const maxWidth = maxPanelWidth(viewport ?? 1440);

  // what the browser remembers, and the window's width
  useEffect(() => {
    const vw = window.innerWidth;
    setViewport(vw);
    const remembered = resolveOpen(read(PANEL_KEY), vw);
    setDockedOpen(remembered);
    markDocked(remembered);
    const savedTab = parseTab(read(PANEL_TAB_KEY));
    if (savedTab) setTabState(savedTab);
    const savedWidth = parseWidth(read(PANEL_WIDTH_KEY));
    if (savedWidth) setWanted(savedWidth);

    let frame = 0;
    const onResize = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => setViewport(window.innerWidth));
    };
    window.addEventListener("resize", onResize);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", onResize);
    };
  }, []);

  // the stylesheet reads the width from here (and PANEL_SCRIPT sets it before the first paint)
  useEffect(() => {
    if (viewport !== null) {
      document.documentElement.style.setProperty("--panel-width", `${width}px`);
    }
  }, [width, viewport]);

  const latest = useRef({ docked, open });
  latest.current = { docked, open };

  const setOpen = useCallback((next: boolean) => {
    if (!latest.current.docked) {
      if (next) rememberOpener(opener);
      setSheetOpen(next);
      return;
    }
    // a docked panel that closes with the focus inside it would leave the focus nowhere
    const inside = document.getElementById(PANEL_ID)?.contains(document.activeElement);
    if (!next && inside) toggleRef.current?.focus();
    setDockedOpen(next);
    markDocked(next);
    write(PANEL_KEY, next ? "open" : "closed");
  }, []);

  const toggle = useCallback(() => setOpen(!latest.current.open), [setOpen]);

  const setTab = useCallback((next: PanelTab) => {
    setTabState(next);
    write(PANEL_TAB_KEY, next);
  }, []);

  const setWidth = useCallback((next: number, remember = true) => {
    const clamped = clampWidth(next, window.innerWidth);
    setWanted(clamped);
    if (remember) write(PANEL_WIDTH_KEY, String(clamped));
  }, []);

  const openSteps = useCallback((turnId: string, stepId?: string) => {
    // asked for, not chosen: it opens the panel for now and leaves the remembered choice alone
    if (latest.current.docked) {
      setDockedOpen(true);
      markDocked(true);
    } else {
      rememberOpener(opener);
      setSheetOpen(true);
    }
    setTabState("activity");
    setFocus((f) => ({ turnId, key: (f?.key ?? 0) + 1, ...(stepId ? { stepId } : {}) }));
  }, []);

  const showTurn = useCallback((turnId: string) => {
    if (latest.current.docked) {
      focusTurn(turnId);
      return;
    }
    // the sheet covers the chat: close it, and let its `onCloseAutoFocus` focus the turn
    pendingTurn.current = turnId;
    setSheetOpen(false);
  }, []);

  const takePendingTurn = useCallback(() => {
    const turn = pendingTurn.current;
    pendingTurn.current = null;
    return turn;
  }, []);

  const returnFocus = useCallback(() => {
    const target = opener.current?.isConnected ? opener.current : toggleRef.current;
    opener.current = null;
    target?.focus();
  }, []);

  // Ctrl/⌘+Shift+. : `code`, not `key` (the character depends on the layout), never while an IME composes
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      if (e.code !== "Period" || !e.shiftKey || e.altKey || !(e.ctrlKey || e.metaKey)) return;
      e.preventDefault();
      toggle();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [toggle]);

  const controller = useMemo<PanelController>(
    () => ({
      panelId: PANEL_ID,
      layout,
      open,
      tab,
      width,
      maxWidth,
      toggleRef,
      setOpen,
      toggle,
      setTab,
      setWidth,
      showTurn,
      takePendingTurn,
      returnFocus,
    }),
    [
      layout,
      open,
      tab,
      width,
      maxWidth,
      setOpen,
      toggle,
      setTab,
      setWidth,
      showTurn,
      takePendingTurn,
      returnFocus,
    ],
  );
  const steps = useMemo(
    () => ({ panelId: PANEL_ID, open, tab, focus, openSteps, expansion, setExpansion }),
    [open, tab, focus, openSteps, expansion],
  );
  return (
    <Context.Provider value={controller}>
      <StepsPanelContext.Provider value={steps}>{children}</StepsPanelContext.Provider>
    </Context.Provider>
  );
}
