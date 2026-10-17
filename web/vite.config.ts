import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { bundleReport } from "./scripts/build/bundle-report.mjs";

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
  // Plain TypeScript libraries run without a DOM. A .tsx test under src/lib (a hook, a provider) runs under jsdom.
  "src/lib/**/*.test.ts",
  "src/api/**/*.test.ts",
  "src/mocks/**/*.test.ts",
  "e2e/fixtures/**/*.test.ts",
];

// `vite build --mode perf` builds the same app plus the test-only routes of src/router.tsx (the overlay primitives, the
// editor and, in T2, the performance pages) into dist-perf/, which the e2e and performance runs serve through the hub's own
// header code. It is never the production dist and never in the image.
export default defineConfig(({ mode }) => ({
  plugins: [react(), tailwindcss(), bundleReport()],
  resolve: { alias },
  // The build writes the placeholder into every generated `nonce` attribute and into <meta property="csp-nonce">; the hub
  // replaces it with the response's own nonce (crates/hub/src/static_files.rs refuses to start without it). S12.
  html: { cspNonce: "__CSP_NONCE__" },
  build: {
    target: "es2022",
    outDir: mode === "perf" ? "dist-perf" : "dist",
    // Fonts and everything else are files under /assets/ (the CSP has no data: for fonts), never inlined.
    assetsInlineLimit: 0,
    // dist-check and the bundle gate read the manifest to find the initial chunks.
    manifest: true,
    // Maps are served by the hub (image: web/dist is copied whole), so production builds have none. LK_SOURCEMAP=1 is for
    // local debugging only (threat model, W22).
    sourcemap: process.env["LK_SOURCEMAP"] === "1",
  },
  // The editor's web worker is one classic script, created with `new Worker(url)` through a Trusted Types policy.
  worker: { format: "iife" },
  server: { proxy: { "/api": process.env["LK_HUB_URL"] ?? "http://127.0.0.1:8080", "/auth": process.env["LK_HUB_URL"] ?? "http://127.0.0.1:8080" } },
  test: {
    // Playwright specs live in e2e/ and are run by Playwright, never by Vitest.
    exclude: ["e2e/**/*.spec.ts", "node_modules/**", "dist/**"],
    projects: [
      {
        extends: true,
        test: {
          name: "node",
          environment: "node",
          include: NODE_TEST_DIRS,
          exclude: ["e2e/**/*.spec.ts", "node_modules/**", "dist/**"],
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
}));
