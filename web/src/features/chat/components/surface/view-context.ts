import { createContext, useContext } from "react";

/** What the components of one drawn surface share (surface-view.tsx provides it). */
export type ViewState = {
  surfaceId: string;
  /** The newest copy of the surface: an older copy is read-only. */
  live: boolean;
  values: Readonly<Record<string, unknown>>;
  setValue: (key: string, value: unknown) => void;
};

export const ViewCtx = createContext<ViewState | null>(null);

export const useView = (): ViewState => {
  const view = useContext(ViewCtx);
  if (!view) throw new Error("a surface component outside a surface");
  return view;
};
