import js from "@eslint/js";
import { defineConfig } from "eslint/config";
import tseslint from "typescript-eslint";
import { lanekeeperRules } from "./lint/lanekeeper-rules.mjs";
import { restrictedImports } from "./lint/restricted-imports.mjs";

// Globals for the Node scripts (build tooling, extractors, lint rules). Spelled out so no globals package is needed.
const nodeGlobals = Object.fromEntries(
  ["process", "console", "Buffer", "URL", "URLSearchParams", "setTimeout", "clearTimeout", "setInterval", "clearInterval", "queueMicrotask", "structuredClone", "TextDecoder", "TextEncoder", "fetch", "AbortController", "AbortSignal", "Response", "Request", "Headers"].map((name) => [name, "readonly"]),
);

// Type-aware linting needs every linted file to be in a tsconfig: tsconfig.app.json (src) and tsconfig.node.json (e2e,
// scripts, lint, config files). The project service finds the right one through the solution-style tsconfig.json.
export default defineConfig(
  { ignores: ["dist", "dist-e2e", "dist-perf", "node_modules", "test-results", "playwright-report", "src/api/generated/**", "**/*.generated.ts", "e2e/visual/.refs/**"] },
  js.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  {
    languageOptions: { parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname } },
    rules: {
      // S21 equivalents: no any, no non-null assertions, no floating promises, only Error objects are thrown.
      "@typescript-eslint/no-explicit-any": "error",
      "@typescript-eslint/no-non-null-assertion": "error",
      "@typescript-eslint/no-floating-promises": "error",
      // The router's own control flow: `throw redirect(...)` and `throw notFound()` are objects it catches, not errors.
      "@typescript-eslint/only-throw-error": ["error", { allow: [{ from: "package", package: "@tanstack/router-core", name: ["Redirect", "NotFoundError"] }] }],
      // Numbers in template strings are fine (counts, sizes); anything else must be converted on purpose.
      "@typescript-eslint/restrict-template-expressions": ["error", { allowNumber: true }],
      // `onClick={() => setOpen(true)}` is how React code is written; a void expression in the body of an arrow is fine.
      "@typescript-eslint/no-confusing-void-expression": ["error", { ignoreArrowShorthand: true }],
      // `_name` marks a value that is deliberately not used (a rest-sibling, a parameter that fixes a signature).
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrorsIgnorePattern: "^_", ignoreRestSiblings: true }],
    },
  },
  {
    // The one mutator is generic in its return type, which is how the generated client says what an operation returns.
    files: ["src/api/client.ts"],
    rules: { "@typescript-eslint/no-unnecessary-type-parameters": "off" },
  },
  {
    // Plain JavaScript (the build scripts and lint rules) has no declared types, so every value read from an untyped import
    // is `any` and the no-unsafe-* family can only complain about the language. The rules that still work on inferred
    // types (floating promises, thrown values, awaiting non-promises) stay on, with the project service.
    files: ["**/*.{js,mjs}"],
    rules: {
      "@typescript-eslint/no-unsafe-assignment": "off",
      "@typescript-eslint/no-unsafe-member-access": "off",
      "@typescript-eslint/no-unsafe-call": "off",
      "@typescript-eslint/no-unsafe-return": "off",
      "@typescript-eslint/no-unsafe-argument": "off",
      "@typescript-eslint/restrict-plus-operands": "off",
      "@typescript-eslint/restrict-template-expressions": "off",
      // The inferred type of an untyped function can be `void` or `never` where the code is fine.
      "@typescript-eslint/no-confusing-void-expression": "off",
      "@typescript-eslint/no-unnecessary-condition": "off",
    },
  },
  { files: ["scripts/**/*.{js,mjs}", "lint/**/*.{js,mjs}", "e2e/**/*.{js,mjs}", "*.config.{js,mjs}"], languageOptions: { globals: nodeGlobals } },
  {
    // e2e/visual/*.mjs hand functions to `page.evaluate`, which run in the browser: they use its globals, nothing else of it.
    files: ["e2e/visual/*.mjs"],
    languageOptions: { globals: Object.fromEntries(["document", "getComputedStyle", "atob", "createImageBitmap", "OffscreenCanvas", "ImageData", "Blob", "FileReader"].map((name) => [name, "readonly"])) },
  },
  {
    // public/ holds classic browser scripts (theme-init.js), copied to dist as they are: no module, no types, browser globals.
    files: ["public/**/*.js"],
    ...tseslint.configs.disableTypeChecked,
    languageOptions: { sourceType: "script", globals: { window: "readonly", document: "readonly" }, parserOptions: { projectService: false } },
  },
  ...lanekeeperRules,
  ...restrictedImports,
);
