import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// No CDN, no third-party origins (S12). Monaco and fonts are self-hosted later (prompt 08).
export default defineConfig({
  plugins: [react()],
  build: { sourcemap: true, target: "es2022" },
  server: { proxy: { "/api": "http://127.0.0.1:8080", "/auth": "http://127.0.0.1:8080" } },
  test: { environment: "node" },
});
