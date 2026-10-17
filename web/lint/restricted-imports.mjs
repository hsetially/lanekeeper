// Imports the app must not make. Flat-config objects, consumed by eslint.config.js (and lint/syntax-config.mjs).

// Radix Select and ScrollArea render a <style> whose content is set through innerHTML, and sonner injects a
// <style> too. Under Trusted Types and a nonce-only style-src (S12) they throw or are blocked. radix-ui's umbrella
// package installs them, so they are banned by name; use NativeSelect, native scrolling and the app's own toaster.
const BLOCKED_STYLE_INJECTORS = [
  { name: "sonner", message: "sonner injects a <style> element at runtime, which the CSP refuses (S12). Use the app's own toaster." },
  { name: "@radix-ui/react-select", message: "Radix Select injects a <style> built with innerHTML (S12). Use NativeSelect, or Popover plus Command." },
  { name: "@radix-ui/react-scroll-area", message: "Radix ScrollArea injects a <style> built with innerHTML (S12). Use native overflow scrolling." },
  {
    name: "radix-ui",
    importNames: ["Select", "ScrollArea"],
    message: "Radix Select and ScrollArea inject a <style> built with innerHTML (S12). Use NativeSelect and native scrolling.",
  },
];

// Test and mock tooling that must never reach the production bundle (P10: the initial JavaScript is under 250 KB).
const TEST_ONLY_PATHS = [
  { name: "msw", message: "msw is for tests, mocks and the e2e harness; it must not be in the bundle." },
  { name: "@faker-js/faker", message: "faker is for mocks; it must not be in the bundle." },
];
const TEST_ONLY_PATTERNS = [{ group: ["@/mocks", "@/mocks/*", "**/generated/endpoints/**/*.msw"], message: "Mocks are for tests and the e2e harness; they must not be in the bundle." }];

const HANDOFF_PATTERN = {
  group: ["@/handoff", "@/handoff/*", "**/handoff/*.generated"],
  message: "src/handoff is test and tooling data from the design handoff; product code must not import it.",
};

// Monaco is lazy and lives behind src/editor (T2 loads it on the file and diff routes only). One import elsewhere would put
// 4 MB into the initial chunk.
const MONACO_PATHS = [{ name: "monaco-editor", message: "Only src/editor imports monaco-editor, behind the lazy loader (P10)." }];
const MONACO_PATTERNS = [{ group: ["monaco-editor/*"], message: "Only src/editor imports monaco-editor, behind the lazy loader (P10)." }];

// Only the one HTML sink and the editor may reach the Trusted Types policies.
const POLICY_PATTERN = {
  group: ["@/lib/csp/trusted-types", "**/csp/trusted-types"],
  message: "Only components/app/SanitisedDocHtml.tsx and src/editor use the Trusted Types policies (S12).",
};

const rule = ({ paths = [], patterns = [] }) => ["error", { paths: [...BLOCKED_STYLE_INJECTORS, ...paths], patterns }];

const TESTS = ["src/**/*.test.{ts,tsx}", "src/handoff/**", "src/test/**"];

export const restrictedImports = [
  {
    // Everything under src, tests included.
    files: ["src/**/*.{ts,tsx}"],
    rules: { "no-restricted-imports": rule({}) },
  },
  {
    // Product code.
    files: ["src/**/*.{ts,tsx}"],
    ignores: [...TESTS, "src/mocks/**"],
    rules: { "no-restricted-imports": rule({ paths: [...TEST_ONLY_PATHS, ...MONACO_PATHS], patterns: [HANDOFF_PATTERN, ...TEST_ONLY_PATTERNS, ...MONACO_PATTERNS, POLICY_PATTERN] }) },
  },
  {
    // The one HTML sink and the Trusted Types module's own folder may use the policies.
    files: ["src/components/app/SanitisedDocHtml.tsx", "src/lib/csp/**/*.ts"],
    ignores: TESTS,
    rules: { "no-restricted-imports": rule({ paths: [...TEST_ONLY_PATHS, ...MONACO_PATHS], patterns: [HANDOFF_PATTERN, ...TEST_ONLY_PATTERNS, ...MONACO_PATTERNS] }) },
  },
  {
    // The editor may import Monaco and the policies.
    files: ["src/editor/**/*.{ts,tsx}"],
    ignores: TESTS,
    rules: { "no-restricted-imports": rule({ paths: TEST_ONLY_PATHS, patterns: [HANDOFF_PATTERN, ...TEST_ONLY_PATTERNS] }) },
  },
];
