// A stand-in for src/components/ui/resizable.tsx in the tests of the swimlane screens. The real one (react-resizable-panels) answers a
// pointer press by asking which separator is under the pointer, and jsdom gives every element the same box (src/test/setup.ts), so
// every press "hits" a separator and the library keeps the press from every other control: no menu opens. A browser lays the
// elements out, so this is a limit of the test environment, not of the screen; the layout itself is checked in the browser (e2e).
// Test code only. Use it as: vi.mock("@/components/ui/resizable", async () => import("@/mocks/design/swimlanes/plain-resizable"));
import type { ReactNode } from "react";

export function ResizablePanelGroup({ children }: { children?: ReactNode; [key: string]: unknown }) {
  return <div data-testid="panel-group">{children}</div>;
}

export function ResizablePanel({ children }: { children?: ReactNode; [key: string]: unknown }) {
  return <div data-testid="panel">{children}</div>;
}

export function ResizableHandle({ "aria-label": label }: { "aria-label"?: string; [key: string]: unknown }) {
  return <div role="separator" aria-label={label} aria-orientation="vertical" />;
}
