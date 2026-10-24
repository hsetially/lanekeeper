import { isValidElement } from "react";
import { describe, expect, it } from "vitest";
import { productSources } from "@/test/source-files";
import { checkAgentAction } from "./check-agent";

// "Check agent": the way from a banner that says an agent is offline to the page where an Admin can look at it. The file view and the
// findings page each drew the same button.

describe("checkAgentAction", () => {
  it("is nothing for someone who cannot open the agents", () => {
    expect(checkAgentAction(false)).toEqual({});
  });

  it("is a secondary small link to the agents of the Admin page for someone who can", () => {
    const { action } = checkAgentAction(true);
    expect(isValidElement(action)).toBe(true);
    expect(JSON.stringify(action, (_key, value: unknown) => (typeof value === "function" || typeof value === "symbol" ? undefined : value))).toMatch(/Check agent/);
  });

  it("is written once: the banners of the file view and of the findings page both take it from here", () => {
    const holders = productSources().filter(({ text }) => /route="admin-swimlanes"[^>]*>\s*Check agent/.test(text)).map((f) => f.path);
    expect(holders).toEqual(["src/features/shell/check-agent.tsx"]);
  });
});
