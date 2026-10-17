import { defineConfig } from "orval";

// The generated client lives in src/api/generated/ and is committed: the image build copies only web/, so it cannot read
// ../api at build time. `pnpm gen:api && git diff --exit-code src/api` is the freshness check (just verify-08).
export default defineConfig({
  lanekeeper: {
    input: {
      target: "../api/openapi.yaml",
      override: { transformer: "./scripts/api/openapi-transformer.mjs" },
    },
    output: {
      mode: "tags-split",
      target: "src/api/generated/endpoints",
      schemas: "src/api/generated/model",
      client: "react-query",
      httpClient: "fetch",
      mock: { generators: [{ type: "msw", delay: false }] },
      clean: true,
      override: {
        mutator: { path: "src/api/client.ts", name: "apiFetch" },
        fetch: { includeHttpResponseReturnType: false },
        // Generated handlers are typed against the strict flags (exactOptionalPropertyTypes), so every optional property is
        // filled instead of being `undefined`. They are a base only: scenarios in src/mocks override what a screen needs.
        mock: { required: true },
      },
    },
  },
});
