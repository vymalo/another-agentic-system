import type { ApiMe, ApiShare, Sharing } from "@/lib/api/types";

/*
 * What `GET /api/me`'s `sharing` and a thread's `share` say, as questions the screens ask
 * (ADR 0040, section 12). Nothing here is a check: the orchestrator holds the cap and the
 * permission whatever the page shows, and says why in the problem's words when it refuses.
 */

/** What a thread can be shared as. `private` is not shared: the dialog's "Stop sharing". */
export type ShareLevel = "private" | "internal" | "public";
/** Who a reader is: a signed-in person (`internal`) or anybody (`public`). */
export type Audience = "internal" | "public";

/** Where a reader finds the thread: its link's token and which route serves it. */
export type ShareSource = { token: string; audience: Audience };

/** What the person may share their threads as: `disabled` when `/api/me` says nothing or could not be read. */
export const sharingOf = (me: ApiMe | null): Sharing => me?.sharing ?? "disabled";

const RANK: Record<ShareLevel, number> = { private: 0, internal: 1, public: 2 };
const CAP: Record<Sharing, number> = { disabled: 0, internal: 1, public: 2 };

export type LevelChoice = {
  level: ShareLevel;
  label: string;
  /** What the choice means, one line. */
  hint: string;
  /** The hint is a warning (the public choice): drawn on the warning colours. */
  warning?: true;
  allowed: boolean;
  /** Why not, when it is not allowed. */
  reason?: string;
};

/** The one line the public choice carries (ADR 0040, section 12): what is read, and that the e-mail is not. */
export const PUBLIC_WARNING =
  "Anyone with this link can read this conversation, including what you pasted in it. Your e-mail is not shown.";

const CHOICES: readonly Pick<LevelChoice, "level" | "label" | "hint" | "warning">[] = [
  { level: "private", label: "Private", hint: "Only you can open this conversation." },
  {
    level: "internal",
    label: "Signed-in people with the link",
    hint: "People who are signed in and have the link can read it.",
  },
  {
    level: "public",
    label: "Anyone with the link",
    hint: PUBLIC_WARNING,
    warning: true,
  },
];

/** The three choices of the dialog, each with whether the cap lets the person pick it and the reason when not. */
export function levelsFor(cap: Sharing): LevelChoice[] {
  return CHOICES.map((choice) => {
    const allowed = RANK[choice.level] <= CAP[cap];
    if (allowed) return { ...choice, allowed };
    return {
      ...choice,
      allowed,
      reason:
        cap === "disabled"
          ? "Sharing is turned off here."
          : "This deployment shares only with signed-in people.",
    };
  });
}

/**
 * Whether the thread's menu has "Share…". Sharing on, or a thread that is shared already: taking a
 * link down needs only ownership, never the cap, so its owner can always reach "Stop sharing".
 */
export const menuOffersShare = (cap: Sharing, share: ApiShare | undefined): boolean =>
  cap !== "disabled" || share !== undefined;

/** The chip in the top bar: who can read the thread now (`effective`, what is served, not what is stored). */
export function chipLabel(share: ApiShare): string {
  switch (share.effective) {
    case "internal":
      return "Shared · signed-in";
    case "public":
      return "Shared · public";
    default:
      return "Sharing paused";
  }
}

/** The link for the page to copy: `url` is `/s/<token>` when the deployment has no public URL. */
export const absoluteLink = (url: string, origin: string): string => new URL(url, origin).href;

/** 43 characters of the unpadded base64url alphabet (`nonce || MAC`): anything else is the one 404. */
const TOKEN = /^[A-Za-z0-9_-]{43}$/;
export const isShareToken = (value: string): boolean => TOKEN.test(value);

export const sharedPath = (token: string): string => `/s/${token}`;

/** A file of a shared thread: the route of the reader's kind, not the owner's own. */
export const sharedFileHref = (token: string, audience: Audience, sha256: string): string =>
  audience === "public"
    ? `/api/public/shared/${token}/artifacts/${sha256}`
    : `/api/shared/${token}/artifacts/${sha256}`;
