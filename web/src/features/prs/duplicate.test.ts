import { describe, expect, it } from "vitest";
import type { PrLink } from "@/api/generated/model";
import { ApiProblem } from "@/api/problem";
import { duplicatePrOf, parsePrLink } from "./duplicate";

// The 409 of `createPr` for a second pull request on a change that already has an open one (D77, CG13): `reason` is
// `duplicate_pr` and `link` is the open one. The app branches on that and on nothing the server wrote in `detail`. The link comes
// from the wire, so it is checked before it is drawn.

const LINK: PrLink = {
  id: 3,
  swimlane: "sitb",
  path: "tx-infinity-api/tx-infinity-core-sit7.yml",
  nfs_hash: "a".repeat(64),
  state: "open",
  pr_url: "https://github.com/csp/csp-tenant-data/pull/418",
  pr_number: 418,
  author: "00000000-0000-4000-8000-000000000001:00000000-0000-4000-8000-000000000002",
  created_at: 1_760_000_000_000,
  closed_at: null,
};

const problem = (status: number, body: Record<string, unknown>, kind = "conflict") => new ApiProblem({ status, kind, title: "Conflict", requestId: "req-1", body });

describe("duplicatePrOf", () => {
  it("reads the open pull request from the 409", () => {
    expect(duplicatePrOf(problem(409, { reason: "duplicate_pr", link: LINK }))).toEqual(LINK);
  });

  it("is null for any other problem", () => {
    expect(duplicatePrOf(problem(409, { current: "a".repeat(64) }))).toBeNull();
    expect(duplicatePrOf(problem(409, { reason: "stale", link: LINK }))).toBeNull();
    expect(duplicatePrOf(problem(422, { reason: "duplicate_pr", link: LINK }, "unprocessable"))).toBeNull();
    expect(duplicatePrOf(problem(500, { reason: "duplicate_pr", link: LINK }, "internal"))).toBeNull();
  });

  it("is null when the link is missing or is not a whole link", () => {
    expect(duplicatePrOf(problem(409, { reason: "duplicate_pr" }))).toBeNull();
    expect(duplicatePrOf(problem(409, { reason: "duplicate_pr", link: null }))).toBeNull();
    expect(duplicatePrOf(problem(409, { reason: "duplicate_pr", link: "https://github.com/x" }))).toBeNull();
    expect(duplicatePrOf(problem(409, { reason: "duplicate_pr", link: { ...LINK, pr_number: 0 } }))).toBeNull();
  });

  it("does not read what is not a problem", () => {
    expect(duplicatePrOf(new Error("x"))).toBeNull();
    expect(duplicatePrOf(null)).toBeNull();
    expect(duplicatePrOf("duplicate_pr")).toBeNull();
  });
});

describe("parsePrLink", () => {
  it("accepts a whole link, and drops a member the contract does not have", () => {
    expect(parsePrLink({ ...LINK, extra: "x" })).toEqual(LINK);
  });

  it("accepts a closed time", () => {
    expect(parsePrLink({ ...LINK, state: "merged", closed_at: 1_760_000_100_000 })).toMatchObject({ state: "merged", closed_at: 1_760_000_100_000 });
  });

  it.each([
    ["an id that is not a whole positive number", { id: 1.5 }],
    ["an id of zero", { id: 0 }],
    ["a swimlane that is not an id", { swimlane: "Sit B" }],
    ["an empty path", { path: "" }],
    ["a path of more than 1,024 characters", { path: "a".repeat(1025) }],
    ["a state the contract does not have", { state: "draft" }],
    ["an address that is not text", { pr_url: 5 }],
    ["an address of more than 512 characters", { pr_url: `https://github.com/${"a".repeat(600)}` }],
    ["a number that is not whole", { pr_number: 4.2 }],
    ["a time that is not a number", { created_at: "yesterday" }],
    ["a closed time that is neither a number nor null", { closed_at: "never" }],
    ["a hash that is not text", { nfs_hash: 7 }],
    ["an author that is not text", { author: null }],
  ])("refuses %s", (_name, over) => {
    expect(parsePrLink({ ...LINK, ...over })).toBeNull();
  });

  it("refuses a value that is not an object", () => {
    for (const value of [null, undefined, 1, "x", [], [LINK]]) expect(parsePrLink(value)).toBeNull();
  });
});
