import { describe, expect, expectTypeOf, it } from "vitest";
import { linkTo } from "./links";
import type { PageProps, ParamsOf, RouteId, SearchOf } from "./props";
import { ROUTES } from "./table";

// The props of a page are derived from the route table: exact path parameter names, exact search keys, and a typo is a
// compile error. These are checked by the type checker (`pnpm typecheck` compiles tests too) and by running linkTo.

describe("PageProps", () => {
  it("has the path parameters of the route, as strings", () => {
    expectTypeOf<ParamsOf<"files">>().toEqualTypeOf<{ readonly id: string }>();
    expectTypeOf<ParamsOf<"admin-section">>().toEqualTypeOf<{ readonly section: string }>();
    expectTypeOf<ParamsOf<"doc">>().toEqualTypeOf<{ readonly slug: string }>();
  });

  it("has the search keys the route reads, as optional strings", () => {
    expectTypeOf<SearchOf<"swimlanes">>().toEqualTypeOf<{ readonly tier?: string; readonly state?: string; readonly q?: string }>();
    expectTypeOf<SearchOf<"files">["path"]>().toEqualTypeOf<string | undefined>();
  });

  it("has both, and the id of every route in the table is a RouteId", () => {
    expectTypeOf<PageProps<"files">>().toEqualTypeOf<{ readonly params: { readonly id: string }; readonly search: SearchOf<"files"> }>();
    const ids: RouteId[] = ROUTES.map((r) => r.id) as RouteId[];
    expect(ids).toContain("files");
  });
});

describe("linkTo", () => {
  it("builds the same link as hrefFor, with the names checked", () => {
    expect(linkTo("files", { id: "sit1" }, { path: "a/b.yml" })).toBe("/swimlanes/sit1/files?path=a%2Fb.yml");
    expect(linkTo("swimlanes")).toBe("/swimlanes");
    expect(linkTo("swimlanes", undefined, { tier: "sit" })).toBe("/swimlanes?tier=sit");
  });

  it("does not compile with a wrong parameter or search key", () => {
    // @ts-expect-error `files` needs the swimlane id
    expect(() => linkTo("files")).toThrow();
    // @ts-expect-error `nope` is not a search key of `swimlanes`
    expect(() => linkTo("swimlanes", undefined, { nope: "1" })).toThrow();
    // @ts-expect-error `bogus` is not a route
    expect(() => linkTo("bogus")).toThrow();
  });
});
