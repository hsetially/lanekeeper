// Imports the app must not make. Flat-config objects, consumed by eslint.config.js (T1 extends this folder).

// Radix Select and ScrollArea render a <style> whose content is set through innerHTML, and sonner injects a
// <style> too. Under Trusted Types and a nonce-only style-src (S12) they throw or are blocked. radix-ui's umbrella
// package installs them, so they are banned by name; use NativeSelect, native scrolling and the app's own toaster.
const BLOCKED_STYLE_INJECTORS = [
  { name: "sonner", message: "sonner injects a <style> element at runtime, which the CSP refuses (S12). Use the app's own toaster." },
  { name: "@radix-ui/react-select", message: "Radix Select injects a <style> built with innerHTML (S12). Use NativeSelect, or Popover plus Command." },
  { name: "@radix-ui/react-scroll-area", message: "Radix ScrollArea injects a <style> built with innerHTML (S12). Use native overflow scrolling." },
];

export const restrictedImports = [
  {
    files: ["src/**/*.{ts,tsx}"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          paths: [
            ...BLOCKED_STYLE_INJECTORS,
            {
              name: "radix-ui",
              importNames: ["Select", "ScrollArea"],
              message: "Radix Select and ScrollArea inject a <style> built with innerHTML (S12). Use NativeSelect and native scrolling.",
            },
          ],
        },
      ],
    },
  },
  {
    // src/handoff holds data extracted from the design handoff for tests and tooling. It is large and must never
    // reach the bundle, so product code does not import it.
    files: ["src/**/*.{ts,tsx}"],
    ignores: ["src/**/*.test.{ts,tsx}", "src/handoff/**", "src/test/**"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          paths: [
            ...BLOCKED_STYLE_INJECTORS,
            { name: "radix-ui", importNames: ["Select", "ScrollArea"], message: "Radix Select and ScrollArea inject a <style> built with innerHTML (S12)." },
          ],
          patterns: [{ group: ["@/handoff", "@/handoff/*", "**/handoff/*.generated"], message: "src/handoff is test and tooling data from the design handoff; product code must not import it." }],
        },
      ],
    },
  },
];
