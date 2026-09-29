import path from "node:path";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { "@": path.resolve("src") } },
  test: {
    include: ["src/**/*.test.ts", "src/**/*.test.tsx", "mock/**/*.test.ts"],
    environment: "node",
  },
});
