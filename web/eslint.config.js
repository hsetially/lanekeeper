import js from "@eslint/js";
import tseslint from "typescript-eslint";
import react from "eslint-plugin-react";

export default tseslint.config(
  { ignores: ["dist"] },
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
);
