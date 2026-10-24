import { describe, expect, it } from "vitest";
import { adminKeys, auditKeys, fileKeys, meKeys, prKeys, proposalKeys, swimlaneKeys } from "@/lib/query/keys";
import { isSignal } from "./newer";

// Which caches stand for "something audited may have happened": the ones the live events that come with an audited action refresh.

describe("the caches the count of updates watches", () => {
  it("are the swimlanes, the proposals, the pull requests and the access requests", () => {
    for (const key of [swimlaneKeys.all(), swimlaneKeys.detail("sitb"), proposalKeys.lists(), proposalKeys.list({ status: "pending" }), proposalKeys.detail(12), prKeys.lists(), adminKeys.section("access-requests"), adminKeys.list("access-requests", {})]) {
      expect(isSignal(key), JSON.stringify(key)).toBe(true);
    }
  });

  it("are not the rest: the audit log itself, the users, the files, the signed-in person", () => {
    for (const key of [auditKeys.all(), auditKeys.list({}), adminKeys.section("users"), adminKeys.list("users", { limit: 500 }), fileKeys.head("sitb", "a.yml"), meKeys.all()]) {
      expect(isSignal(key), JSON.stringify(key)).toBe(false);
    }
  });
});
