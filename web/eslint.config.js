import js from "@eslint/js";
import tseslint from "typescript-eslint";
import react from "eslint-plugin-react";
import { restrictedImports } from "./lint/restricted-imports.mjs";

// Globals for the Node scripts (build tooling, extractors, lint rules). Spelled out so no globals package is needed.
const nodeGlobals = Object.fromEntries(
  ["process", "console", "Buffer", "URL", "URLSearchParams", "setTimeout", "clearTimeout", "setInterval", "clearInterval", "queueMicrotask", "structuredClone", "TextDecoder", "TextEncoder"].map((name) => [name, "readonly"]),
);

export default tseslint.config(
  { ignores: ["dist", "dist-perf", "node_modules", "src/api/generated/**", "**/*.generated.ts"] },
  js.configs.recommended,
  ...tseslint.configs.strict,
  {
    plugins: { react },
    settings: { react: { version: "detect" } },
    rules: {
      // S12: no raw HTML injection. The only exception is the server-sanitised docs renderer, added in prompt 08.
      "react/no-danger": "error",
      "no-eval": "error",
      "no-implied-eval": "error",
    },
  },
  { files: ["scripts/**/*.{js,mjs}", "lint/**/*.{js,mjs}", "*.config.{js,mjs}"], languageOptions: { globals: nodeGlobals } },
  ...restrictedImports,
);
