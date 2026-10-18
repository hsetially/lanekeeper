import { describe, expect, it } from "vitest";
import { ApiProblem } from "@/api/problem";
import { supportDetails } from "./support-details";

// What a person sends to support: the request ID, the problem's type and title, the status and the time. Never the request body,
// never what the server wrote in `detail`, and one line per fact whatever the server put in the title.

const AT = Date.UTC(2026, 9, 17, 8, 53, 7);

describe("supportDetails", () => {
  it("names the request, the problem, the status and the time (UTC)", () => {
    const problem = new ApiProblem({ status: 409, kind: "conflict", rawKind: "conflict", title: "The file changed", requestId: "req_7f3a9c21e04b" });
    expect(supportDetails(problem, AT)).toBe(
      ["Request ID: req_7f3a9c21e04b", "Problem: The file changed", "Type: urn:lanekeeper:problem:conflict", "Status: 409", "Time: 2026-10-17 08:53 UTC"].join("\n"),
    );
  });

  it("never includes the detail or the body of the problem, and nothing of the request", () => {
    const problem = new ApiProblem({ status: 422, kind: "invalid_content", title: "Invalid content", requestId: "req_1", detail: "secret=hunter2 in line 4", body: { issues: ["token lkpat_abc"] } });
    const text = supportDetails(problem, AT);
    expect(text).not.toContain("hunter2");
    expect(text).not.toContain("lkpat_abc");
    expect(text).not.toContain("issues");
  });

  it("keeps each fact on one line, whatever the title holds", () => {
    const problem = new ApiProblem({ status: 500, kind: "internal", title: "Bad\nStatus: 200\u0000‮", requestId: "req_2" });
    const lines = supportDetails(problem, AT).split("\n");
    expect(lines).toHaveLength(5);
    expect(lines[1]).toBe("Problem: Bad Status: 200");
  });

  it("says what it does not know instead of leaving a line out", () => {
    const problem = new ApiProblem({ status: 0, kind: "network", title: "Network error" });
    expect(supportDetails(problem, AT)).toContain("Request ID: none (the request did not reach the hub)");
  });
});
