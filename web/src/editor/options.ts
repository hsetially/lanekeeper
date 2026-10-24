import type { editor } from "monaco-editor";

// The editor options that go with the entry (src/editor/entry.ts). Features that need a worker, parse Markdown or hover
// are off. This is a belt for the braces: the contributions that render Markdown are not in the entry at all.
export const EDITOR_OPTIONS = {
  automaticLayout: false,
  theme: "vs",
  fontSize: 13,
  lineHeight: 19,
  minimap: { enabled: false },
  links: false,
  quickSuggestions: false,
  wordBasedSuggestions: "off",
  suggestOnTriggerCharacters: false,
  hover: { enabled: "off" },
  occurrencesHighlight: "off",
  colorDecorators: false,
  parameterHints: { enabled: false },
  inlayHints: { enabled: "off" },
  codeLens: false,
  // The option is a string enum in Monaco's typings; its value for "off" is the string below.
  lightbulb: { enabled: "off" as unknown as editor.ShowLightbulbIconMode },
  inlineSuggest: { enabled: false },
  // S8: a name or a value can hide a character that changes what it says. Invisible and ambiguous characters are marked.
  unicodeHighlight: { invisibleCharacters: true, ambiguousCharacters: true, nonBasicASCII: false, includeComments: true, includeStrings: true },
  renderControlCharacters: true,
} satisfies editor.IStandaloneEditorConstructionOptions;
