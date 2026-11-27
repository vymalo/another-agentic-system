// Diagrams: vitepress-mermaid-renderer (zoom, pan, fullscreen, copy, download) with Mermaid drawn in the
// Material 3 tokens of the theme. Mermaid cannot take `var(--x)` (it derives shades from the colour),
// so the tokens are read from the page, in the scheme it is in, and handed over as plain colours.
// The same colours are read again on every light/dark switch. Styles: ./mermaid.css.
import { h, nextTick, onMounted, onUnmounted, watch } from "vue";
import { useData } from "vitepress";
import { Layout as Md3Layout } from "@vaam-apps/vitepress-md3";
import "./mermaid.css";

const token = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

function themeVariables() {
  const c = (role: string) => token(`--md-sys-color-${role}`);
  return {
    fontFamily: token("--md-sys-typescale-plain-font") || "sans-serif",
    fontSize: "16px",
    background: c("surface"),
    primaryColor: c("primary-container"),
    primaryTextColor: c("on-primary-container"),
    primaryBorderColor: c("primary"),
    secondaryColor: c("secondary-container"),
    secondaryTextColor: c("on-secondary-container"),
    secondaryBorderColor: c("secondary"),
    tertiaryColor: c("tertiary-container"),
    tertiaryTextColor: c("on-tertiary-container"),
    tertiaryBorderColor: c("tertiary"),
    lineColor: c("on-surface-variant"),
    textColor: c("on-surface"),
    mainBkg: c("primary-container"),
    nodeBorder: c("primary"),
    nodeTextColor: c("on-primary-container"),
    clusterBkg: c("surface-container-low"),
    clusterBorder: c("outline-variant"),
    titleColor: c("on-surface"),
    edgeLabelBackground: c("surface"),
    // sequence
    actorBkg: c("primary-container"),
    actorBorder: c("primary"),
    actorTextColor: c("on-primary-container"),
    actorLineColor: c("outline"),
    signalColor: c("on-surface"),
    signalTextColor: c("on-surface"),
    labelBoxBkgColor: c("secondary-container"),
    labelBoxBorderColor: c("secondary"),
    labelTextColor: c("on-secondary-container"),
    loopTextColor: c("on-surface"),
    noteBkgColor: c("tertiary-container"),
    noteBorderColor: c("tertiary"),
    noteTextColor: c("on-tertiary-container"),
    activationBkgColor: c("secondary-container"),
    activationBorderColor: c("secondary"),
    sequenceNumberColor: c("on-primary"),
    // state
    labelColor: c("on-surface"),
    altBackground: c("surface-container-low"),
    // er and the rest
    attributeBackgroundColorOdd: c("surface-container-low"),
    attributeBackgroundColorEven: c("surface-container"),
    errorBkgColor: c("error-container"),
    errorTextColor: c("on-error-container"),
  };
}

// A diagram whose natural height is tall gets room for it, so it is read at its own size and the page scrolls;
// the renderer's own box is 50vh at most, which would shrink a long sequence diagram to unreadable text.
function sizeBoxes() {
  document.querySelectorAll<HTMLElement>(".mermaid-container:not(.dialog-fullscreen-active):not(.static-mermaid-container)").forEach((box) => {
    const svg = box.querySelector<SVGSVGElement>(".diagram-wrapper svg[aria-roledescription]");
    const vb = svg?.viewBox?.baseVal;
    if (!svg || !vb || !vb.width) return;
    // the 3rem strip on top holds the toolbar (mermaid.css)
    const natural = (box.clientWidth * vb.height) / vb.width + 48;
    box.style.setProperty("--diagram-height", `${Math.round(Math.min(Math.max(natural, 240), 2600))}px`);
  });
}

async function start(dark: boolean) {
  const { createMermaidRenderer } = await import("vitepress-mermaid-renderer");
  const renderer = createMermaidRenderer({
    theme: "base",
    themeVariables: themeVariables(),
    startOnLoad: false,
    securityLevel: "strict",
    flowchart: { useMaxWidth: true, htmlLabels: true, padding: 12 },
    // Labels wrap between the lifelines, so the drawing stays about as wide as the page column and its text stays readable.
    sequence: { useMaxWidth: true, wrap: true, width: 150, height: 44, actorMargin: 16, boxMargin: 8, messageMargin: 30, wrapPadding: 6, mirrorActors: true, actorFontSize: 15, messageFontSize: 15, noteFontSize: 15 },
    state: { useMaxWidth: true },
    er: { useMaxWidth: true },
  });
  renderer.setToolbar({
    showLanguageLabel: false,
    downloadFormat: "svg",
    fullscreenMode: "dialog",
    desktop: { copyCode: "enabled", toggleFullscreen: "enabled", resetView: "enabled", zoomOut: "enabled", zoomIn: "enabled", zoomLevel: "enabled", download: "enabled", positions: { vertical: "top", horizontal: "right" } },
    mobile: { positions: { vertical: "top", horizontal: "right" } },
    fullscreen: { copyCode: "enabled", toggleFullscreen: "enabled", resetView: "enabled", zoomOut: "enabled", zoomIn: "enabled", zoomLevel: "enabled", download: "enabled" },
  });
  document.documentElement.dataset.diagramScheme = dark ? "dark" : "light";
}

// Layout of the theme: the md3 layout, with the renderer started in the browser once the page is up.
export const Layout = {
  name: "DiagramsLayout",
  setup() {
    const { isDark } = useData();
    // The renderer closes its fullscreen dialog by a click on the backdrop or the key `f`; Escape does it too.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") document.querySelector<HTMLElement>(".mermaid-dialog-backdrop")?.click();
    };
    onUnmounted(() => document.removeEventListener("keydown", onKey));
    onMounted(() => {
      document.addEventListener("keydown", onKey);
      nextTick(() => start(isDark.value));
      // A drawing arrives, or is redrawn in the other scheme, as new nodes or a new viewBox: size its box again.
      let queued = false;
      new MutationObserver(() => {
        if (queued) return;
        queued = true;
        requestAnimationFrame(() => { queued = false; sizeBoxes(); });
      }).observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["viewBox"] });
      window.addEventListener("resize", sizeBoxes);
    });
    // VitePress flips the `dark` class after this watcher fires: wait a frame so the tokens read are the new ones.
    watch(isDark, (dark) => nextTick(() => requestAnimationFrame(() => start(dark))));
    return () => h(Md3Layout);
  },
};
