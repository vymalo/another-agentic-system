"use client";

import { createContext, type ReactNode, useCallback, useContext, useMemo, useState } from "react";
import { PANEL_ID } from "../lib/panel-state";

/**
 * What the step tree (plan 03, S5.4) needs of the panel's shell, and nothing else: the panel's id for
 * `aria-controls`, whether it is showing, which tab, a request to focus a turn, and the call that
 * makes the panel show a turn's steps. The tree's own components take this through
 * `useStepsPanel()`, so they need no runtime and no layout; tests give them
 * `StepsPanelTestProvider`.
 */
export type StepsPanel = {
  /** The id of the panel (`aria-controls` of whatever opens it). */
  panelId: string;
  /** The panel is showing: docked and open, or its sheet is open. */
  open: boolean;
  /** The selected tab: `"activity"` or `"sources"`. */
  tab: string;
  /**
   * The turn the panel was asked to show; `key` changes on every request, so a second click on the
   * same turn focuses it again. Null until something asks. `stepId` is a step of that turn to show
   * (the chat's failed chip): its way is opened and its input and output with it.
   */
  focus: { turnId: string; key: number; stepId?: string } | null;
  /**
   * Show a turn's steps, and one step of it when `stepId` is given: opens the panel if it is
   * closed (the sheet on a phone), selects the Activity tab and sets `focus` with a new `key`.
   */
  openSteps(turnId: string, stepId?: string): void;
};

/** What the shell keeps for the tree besides the contract: its expansion state, opaque to the shell. */
type StepsPanelState = StepsPanel & {
  expansion: unknown;
  setExpansion(next: unknown): void;
};

const NO_STEPS_PANEL: StepsPanelState = {
  panelId: PANEL_ID,
  open: false,
  tab: "activity",
  focus: null,
  openSteps: () => {},
  expansion: undefined,
  setExpansion: () => {},
};

export const StepsPanelContext = createContext<StepsPanelState>(NO_STEPS_PANEL);

/**
 * The contract of the step tree. Outside a panel (the new-chat page, a test without a provider) the
 * panel is closed and `openSteps` does nothing.
 */
export function useStepsPanel(): StepsPanel {
  return useContext(StepsPanelContext);
}

/**
 * Which turns and nodes of the tree are open. It lives here, above the panel, so closing the
 * panel (a sheet unmounts its content) keeps what was open, and a new thread starts closed (the
 * shell is mounted per thread). The shell does not read it: `T` is the tree's own type.
 */
export function useStepsExpansion<T>(initial: T): [T, (next: T) => void] {
  const { expansion, setExpansion } = useContext(StepsPanelContext);
  return [(expansion === undefined ? initial : expansion) as T, setExpansion as (next: T) => void];
}

/** A provider with the panel's contract and a working expansion state, for the tree's tests. */
export function StepsPanelTestProvider({
  children,
  value,
}: {
  children: ReactNode;
  value?: Partial<StepsPanel>;
}) {
  const [expansion, setExpansion] = useState<unknown>(undefined);
  const [focus, setFocus] = useState<StepsPanel["focus"]>(null);
  const [open, setOpen] = useState(false);
  const openSteps = useCallback((turnId: string, stepId?: string) => {
    setOpen(true);
    setFocus((f) => ({ turnId, key: (f?.key ?? 0) + 1, ...(stepId ? { stepId } : {}) }));
  }, []);
  const state = useMemo<StepsPanelState>(
    () => ({
      panelId: PANEL_ID,
      open,
      tab: "activity",
      focus,
      openSteps,
      expansion,
      setExpansion,
      ...value,
    }),
    [open, focus, openSteps, expansion, value],
  );
  return <StepsPanelContext.Provider value={state}>{children}</StepsPanelContext.Provider>;
}
