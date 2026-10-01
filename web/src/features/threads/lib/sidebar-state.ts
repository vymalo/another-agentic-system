/** Where the desktop sidebar's open or closed state is remembered (localStorage). */
export const SIDEBAR_KEY = "chat.sidebar";

/**
 * Runs in `<head>` before the first paint: a sidebar the person closed is marked on `<html>`
 * (`data-sidebar="closed"`), and the stylesheet hides it by that mark until React's state, which
 * starts open for the server's render, catches up. Storage may be blocked: then nothing happens.
 */
export const SIDEBAR_SCRIPT = `try{if(localStorage.getItem(${JSON.stringify(SIDEBAR_KEY)})==="closed")document.documentElement.dataset.sidebar="closed"}catch(e){}`;
