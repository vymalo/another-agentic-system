import path from "node:path";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { "@": path.resolve("src") } },
  test: {
    include: ["src/**/*.test.ts", "src/**/*.test.tsx", "mock/**/*.test.ts"],
    environment: "node",
    // the jsdom tests run the whole app against the mock orchestrator, and CI shares its cores
    testTimeout: 20_000,
  },
});
