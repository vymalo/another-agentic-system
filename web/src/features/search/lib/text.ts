/**
 * Every word the search box shows or says, in one place so that a later slice can translate them
 * (the web has no catalogue of strings yet; these are the search's own).
 */

export const SEARCH_LABEL = "Search chats";
export const SEARCH_PLACEHOLDER = "Search chats";
export const CLEAR_LABEL = "Clear search";
export const RESULTS_LABEL = "Search results";
export const SEARCHING = "Searching…";
export const RETRY = "Retry";
export const RETRY_NOW = "Retry now";
export const SHOW_MORE = "Show more chats";
export const ARCHIVED = "Archived";
export const ELLIPSIS = "…";

export const ERROR = "Could not search your chats.";
export const BUSY = "Search is busy right now. Try again in a moment.";
export const NO_TERMS = "Search needs words of at least two letters or digits.";

export const RATE_LIMITED = "You are searching too fast. Trying again shortly.";
export const STILL_LIMITED = "Search is still limited. Try again in a moment.";

export const noMatch = (query: string) => `No chats match “${query}”.`;

export const onlyFirst = (max: number) => `Only the first ${max} words are used.`;

/** What a screen reader is told when the results change; `more` is true when another page follows. */
export const found = (count: number, more: boolean) =>
  `${count}${more ? " or more" : ""} ${count === 1 && !more ? "chat" : "chats"} found`;

export const moreMessages = (count: number) =>
  `and ${count} more ${count === 1 ? "message" : "messages"}`;

export const WHO = { person: "You", agent: "Agent" } as const;
export const FIELD = { title: "Title", description: "Description" } as const;

/** The key hint beside the box and the `aria-keyshortcuts` that says the same. */
export const SHORTCUT_APPLE = "⌘K";
export const SHORTCUT_OTHER = "Ctrl K";
export const KEYSHORTCUTS = "Control+K Meta+K";
/** On an Apple platform only ⌘ counts (Ctrl+K is the text field's own). */
export const KEYSHORTCUTS_APPLE = "Meta+K";
