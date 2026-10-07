/** The one place the page is left; tests replace it, as jsdom cannot navigate. */
export const navigation = {
  go(url: string) {
    window.location.assign(url);
  },
  /** Reads the page again: when the session turns out to be another person's (session-refresh.ts). */
  reload() {
    window.location.reload();
  },
};
