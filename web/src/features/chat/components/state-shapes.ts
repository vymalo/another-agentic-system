import {
  CheckIcon,
  CircleStopIcon,
  LoaderCircleIcon,
  ShieldEllipsisIcon,
  XIcon,
} from "lucide-react";

/**
 * The shapes a run's state is drawn with, in the thread's state pill (`state-badge.tsx`) and in a turn's
 * line and its header in the panel (`steps/turn-glyph.tsx`): one map, so the two never say a state in two
 * ways. Checking the work is a shield with dots, because the shield with a check is the check that passed;
 * a stopped run is the stop of the Stop button, because a ban says "forbidden".
 */
export const STATE_SHAPE = {
  working: LoaderCircleIcon,
  verifying: ShieldEllipsisIcon,
  done: CheckIcon,
  failed: XIcon,
  stopped: CircleStopIcon,
} as const;
