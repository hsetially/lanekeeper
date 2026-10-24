import { describe, expect, it } from "vitest";
import { cn } from "./utils";

describe("cn", () => {
  it("joins classes and drops falsy values", () => {
    const off = Math.random() > 2; // always false, but not a constant the linter can see
    expect(cn("a", off && "b", undefined, null, "c")).toBe("a c");
    expect(cn({ a: true, b: false }, ["c"])).toBe("a c");
  });

  it("lets the later utility win within one property group", () => {
    expect(cn("px-2", "px-3")).toBe("px-3");
    expect(cn("text-sm", "text-xs")).toBe("text-xs");
    expect(cn("bg-primary", "bg-secondary")).toBe("bg-secondary");
  });

  it("knows the handoff's own utilities, so they do not erase each other", () => {
    expect(cn("text-mono", "text-muted-foreground")).toBe("text-mono text-muted-foreground");
    expect(cn("text-xs", "text-status-sync-fg")).toBe("text-xs text-status-sync-fg");
    expect(cn("bg-background", "bg-diff-empty")).toBe("bg-background bg-diff-empty");
    expect(cn("tracking-tight", "tracking-caps")).toBe("tracking-caps");
    expect(cn("font-mono", "font-medium")).toBe("font-mono font-medium");
    expect(cn("animate-lk-spin", "animate-lk-pulse")).toBe("animate-lk-pulse");
  });
});
