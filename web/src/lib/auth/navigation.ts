/** The one place the page is left; tests replace it, as jsdom cannot navigate. */
export const navigation = {
  go(url: string) {
    window.location.assign(url);
  },
  /** Goes there without leaving the page behind in the history: the callback's own address is not worth a Back. */
  replace(url: string) {
    window.location.replace(url);
  },
  /** Reads the page again: when the session turns out to be another person's (session-refresh.ts). */
  reload() {
    window.location.reload();
  },
};
