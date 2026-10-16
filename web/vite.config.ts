import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// No CDN, no third-party origins (S12). Monaco and fonts are self-hosted later (prompt 08).
const alias = { "@": fileURLToPath(new URL("./src", import.meta.url)) };

// Directories whose tests run without a DOM: build scripts, lint rules, handoff data, plain libraries.
// Everything else under src/ (components, features, live UI) runs under jsdom.
const NODE_TEST_DIRS = [
  "scripts/**/*.test.{ts,mjs}",
  "lint/**/*.test.{ts,mjs}",
  "src/handoff/**/*.test.{ts,tsx}",
  "src/styles/**/*.test.{ts,tsx}",
  "src/assets/**/*.test.{ts,tsx}",
  "src/icons/**/*.test.{ts,tsx}",
  "src/lib/**/*.test.{ts,tsx}",
  "src/api/**/*.test.{ts,tsx}",
];

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias },
  build: { sourcemap: true, target: "es2022" },
  server: { proxy: { "/api": "http://127.0.0.1:8080", "/auth": "http://127.0.0.1:8080" } },
  test: {
    // Playwright specs live in e2e/ and are run by Playwright, never by Vitest.
    exclude: ["e2e/**", "node_modules/**", "dist/**"],
    projects: [
      {
        extends: true,
        test: {
          name: "node",
          environment: "node",
          include: NODE_TEST_DIRS,
          exclude: ["e2e/**", "node_modules/**", "dist/**"],
        },
      },
      {
        extends: true,
        test: {
          name: "jsdom",
          environment: "jsdom",
          include: ["src/**/*.test.{ts,tsx}"],
          exclude: ["e2e/**", "node_modules/**", "dist/**", ...NODE_TEST_DIRS],
          setupFiles: ["./src/test/setup.ts"],
        },
      },
    ],
  },
});
