import { describe, expect, it } from "vitest";
import { OPERATION_ROLES } from "@/api/roles.generated";
import { productSources, withoutComments } from "@/test/source-files";

// "The SSE invalidation, the write invalidation and the hooks all use the same factories": proved by reading the source, the
// way the lint rule `query-key-literal` reads it, so that a rule that was weakened or a file the lint does not reach cannot
// let a key of another shape in. A key made any other way is a cache that no live event and no write can ever find.

const sources = productSources().map((f) => ({ ...f, code: withoutComments(f.text) }));

const CACHE_METHODS = "getQueryData|setQueryData|getQueriesData|setQueriesData|getQueryState|removeQueries|resetQueries|cancelQueries|invalidateQueries|refetchQueries|fetchQuery|prefetchQuery|ensureQueryData|fetchInfiniteQuery|prefetchInfiniteQuery|ensureInfiniteQueryData|isFetching";

describe("a query key is never written out", () => {
  it("is not a literal array in a `queryKey`, and not a literal array given to a cache method", () => {
    const bad = sources.flatMap(({ path, code }) => {
      const hits: string[] = [];
      if (/queryKey\s*:\s*\[/.test(code)) hits.push(`${path}: queryKey: [`);
      if (new RegExp(`\\.(${CACHE_METHODS})\\(\\s*\\[`).test(code)) hits.push(`${path}: a cache method called with an array literal`);
      return hits;
    });
    expect(bad).toEqual([]);
  });

  it("is never the one the generator made (getXQueryKey, getXQueryOptions): those are literal paths no event can find", () => {
    const bad = sources.filter(({ code }) => /\bget[A-Z][A-Za-z0-9]*Query(Key|Options)\b/.test(code)).map((f) => f.path);
    expect(bad).toEqual([]);
  });

  it("is not made by a generated query hook (useListSwimlanes, useGetMe, ...): its key is the generator's unless someone remembers to override it", () => {
    const gets = Object.entries(OPERATION_ROLES)
      .filter(([, op]) => op.method === "GET")
      .map(([id]) => `use${id.charAt(0).toUpperCase()}${id.slice(1)}`);
    const names = new Set(gets.flatMap((hook) => [hook, `${hook}Suspense`, `${hook}Infinite`]));
    const bad = sources.flatMap(({ path, code }) => [...code.matchAll(/\buse[A-Z][A-Za-z0-9]*\b/g)].filter((m) => names.has(m[0])).map((m) => `${path}: ${m[0]}`));
    expect(bad).toEqual([]);
  });

  it("is built in every colocated query module by a key factory, and only by one", () => {
    const FACTORY = /queryKey\s*:\s*(meKeys|swimlaneKeys|fileKeys|compareKeys|gitKeys|findingKeys|proposalKeys|prKeys|docKeys|auditKeys|adminKeys)\.[a-zA-Z]+\(/g;
    const modules = sources.filter((f) => /^src\/features\/[a-z]+\/queries\.ts$/.test(f.path));
    expect(modules.length).toBeGreaterThanOrEqual(10);
    for (const { path, code } of modules) {
      const all = [...code.matchAll(/queryKey\s*:/g)].length;
      const viaFactory = [...code.matchAll(FACTORY)].length;
      expect(all, `${path} has a queryKey`).toBeGreaterThan(0);
      expect(viaFactory, `${path}: every queryKey comes from a factory`).toBe(all);
    }
  });

  it("is used by the live plan and the write table through the same factories (their source names no literal key)", () => {
    for (const file of ["src/live/invalidation.ts", "src/lib/query/invalidate.ts", "src/lib/query/targets.ts", "src/lib/query/optimistic.ts"]) {
      const found = sources.find((f) => f.path === file);
      expect(found, file).toBeDefined();
      expect(found?.code, file).not.toMatch(/key\(\s*\[/);
      expect(found?.code, file).not.toMatch(/queryKey\s*:\s*\[/);
    }
  });

  it("checks itself: the scan finds a literal key when there is one", () => {
    const sample = withoutComments("useQuery({ queryKey: ['swimlanes'], queryFn });");
    expect(/queryKey\s*:\s*\[/.test(sample)).toBe(true);
    const commented = withoutComments("// queryKey: ['swimlanes'] is what the lint bans\nconst x = 1;");
    expect(/queryKey\s*:\s*\[/.test(commented)).toBe(false);
  });
});

describe("invalidation is always filtered", () => {
  it("has no call that names nothing: the only bulk invalidation of the app is invalidateTargets, and it passes a predicate", () => {
    const bad = sources.flatMap(({ path, code }) => {
      const hits: string[] = [];
      for (const m of code.matchAll(/\.(invalidateQueries|refetchQueries|resetQueries)\(\s*(\)|\{\s*\})/g)) hits.push(`${path}: ${m[0]}`);
      return hits;
    });
    expect(bad).toEqual([]);
    const callers = sources.filter(({ code }) => /\.invalidateQueries\(/.test(code)).map((f) => f.path);
    expect(callers).toEqual(["src/lib/query/targets.ts"]);
    const targets = sources.find((f) => f.path === "src/lib/query/targets.ts");
    expect(targets?.code).toMatch(/invalidateQueries\(\{\s*predicate/);
  });
});
