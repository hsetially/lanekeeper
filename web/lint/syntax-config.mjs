// The lint rules without type information: what lint/rules.test.mjs and restricted-imports.test.mjs run snippets through.
// (Type-aware presets need a real file in a tsconfig; a snippet has none.) eslint.config.js adds the type-aware presets
// on top of the same rule blocks, so a rule proved here is the rule that runs on the code.
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import { lanekeeperRules } from "./lanekeeper-rules.mjs";
import { restrictedImports } from "./restricted-imports.mjs";

export const syntaxOnlyConfig = [
  js.configs.recommended,
  {
    files: ["**/*.{ts,tsx,mjs,js}"],
    plugins: { "@typescript-eslint": tseslint.plugin },
    languageOptions: { parser: tseslint.parser, parserOptions: { ecmaFeatures: { jsx: true } }, ecmaVersion: 2022, sourceType: "module" },
  },
  ...lanekeeperRules,
  ...restrictedImports,
];
