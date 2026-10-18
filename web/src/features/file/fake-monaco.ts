// A stand-in for Monaco in the tests of the edit surface. Monaco does not run in jsdom; this fakes the few calls the surface makes
// (a model, an editor, markers, the theme). The real editor is measured under the production CSP by e2e/editor/csp.spec.ts.
import { vi } from "vitest";
import type { MonacoApi } from "@/editor/loader";

type Listener = () => void;

export function fakeMonaco() {
  const listeners = new Set<Listener>();
  let value = "";
  const model = {
    getValue: () => value,
    setValue(next: string) {
      value = next;
      for (const listener of [...listeners]) listener();
    },
    onDidChangeContent: (listener: Listener) => {
      listeners.add(listener);
      return { dispose: () => listeners.delete(listener) };
    },
    getLineCount: () => value.split("\n").length,
    getLineMaxColumn: () => 40,
    dispose: vi.fn(),
  };
  const editor = { layout: vi.fn(), focus: vi.fn(), dispose: vi.fn() };
  const monaco = {
    editor: {
      createModel: vi.fn((text: string) => {
        value = text;
        return model;
      }),
      create: vi.fn(() => editor),
      setModelMarkers: vi.fn(),
      setTheme: vi.fn(),
      defineTheme: vi.fn(),
    },
    MarkerSeverity: { Error: 8, Warning: 4 },
  };
  return { api: monaco as unknown as MonacoApi, monaco, model, editor, listeners };
}

