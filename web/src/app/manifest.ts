import type { MetadataRoute } from "next";

/** The installable app: the panda on the brand green (web/DESIGN.md "Brand"). */
export default function manifest(): MetadataRoute.Manifest {
  return {
    name: "another·agentic",
    short_name: "agentic",
    description: "Chat surface for the another-agentic-system orchestration layer",
    start_url: "/",
    display: "standalone",
    theme_color: "#3F7341",
    background_color: "#FFFFFF",
    icons: [
      { src: "/brand/icon-192.png", sizes: "192x192", type: "image/png", purpose: "any" },
      { src: "/brand/icon-512.png", sizes: "512x512", type: "image/png", purpose: "any" },
      {
        src: "/brand/icon-maskable-512.png",
        sizes: "512x512",
        type: "image/png",
        purpose: "maskable",
      },
    ],
  };
}
