import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import { productSources, WEB } from "@/test/source-files";

// "Server data kept out of global client state": the only global stores are the QueryClient and the non-server prefs. There is
// no client-state library among the dependencies (not even a transitive one), and no React context or module-level variable is
// typed with a type of the generated API.

const STATE_LIBRARIES = ["zustand", "redux", "@reduxjs/toolkit", "react-redux", "redux-thunk", "jotai", "valtio", "recoil", "mobx", "mobx-react", "mobx-react-lite", "nanostores", "@nanostores/react", "@preact/signals-react", "unstated-next"];

describe("no client-state library is a dependency", () => {
  const pkg = JSON.parse(readFileSync(join(WEB, "package.json"), "utf8")) as { dependencies?: Record<string, string>; devDependencies?: Record<string, string> };

  it("is not in package.json", () => {
    const names = Object.keys({ ...pkg.dependencies, ...pkg.devDependencies });
    expect(names.filter((n) => STATE_LIBRARIES.includes(n))).toEqual([]);
  });

  it("is not anywhere in the lockfile, so not as a dependency of a dependency either", () => {
    const lock = readFileSync(join(WEB, "pnpm-lock.yaml"), "utf8");
    const packages = [...lock.matchAll(/^ {2}'?((?:@[^/@\s']+\/)?[^@\s':]+)@[^\s':]+'?:/gm)].map((m) => m[1] ?? "");
    expect(packages.length).toBeGreaterThan(100);
    expect(packages.filter((n) => STATE_LIBRARIES.includes(n))).toEqual([]);
  });

  it("has TanStack Query as the one store of server data", () => {
    expect(pkg.dependencies).toHaveProperty("@tanstack/react-query");
  });
});

// ---- contexts and module-level variables typed with the generated API ----

type GlobalUse = { name: string; kind: "context" | "module-variable"; line: number };

const GENERATED_MODEL = /generated\/model/;

// What the generator writes for an enum is an object constant named like the enum, with `as const`. A name like that is a vocabulary (a role, a
// drift state), not a response: a constant typed with it holds no server data.
const MODEL_DIR = join(WEB, "src", "api", "generated", "model");
const ENUM_LIKE = new Set(readdirSync(MODEL_DIR).flatMap((file) => [...readFileSync(join(MODEL_DIR, file), "utf8").matchAll(/^export const ([A-Za-z0-9]+) = \{/gm)].map((m) => m[1] ?? "")));

/**
 * Finds a React context, or a module-level variable, whose declared type names a type of the generated API (or a type of the
 * same file that is built from one). THIS IS AN APPROXIMATION and says so: it reads the written type (an annotation or a type
 * argument), one file at a time, so a type inferred from a call (`let me = await getMe()`) is not seen, and a type that
 * reaches the API only through another file is not followed.
 */
export function findServerStateInGlobals(source: string, fileName = "x.tsx"): GlobalUse[] {
  const file = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true, fileName.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const flagged = new Set<string>();
  for (const statement of file.statements) {
    if (ts.isImportDeclaration(statement) && ts.isStringLiteral(statement.moduleSpecifier) && GENERATED_MODEL.test(statement.moduleSpecifier.text)) {
      const bindings = statement.importClause?.namedBindings;
      if (bindings && ts.isNamedImports(bindings)) for (const element of bindings.elements) if (!ENUM_LIKE.has((element.propertyName ?? element.name).text)) flagged.add(element.name.text);
      if (bindings && ts.isNamespaceImport(bindings)) flagged.add(bindings.name.text);
    }
  }
  const mentions = (node: ts.Node): boolean => [...flagged].some((name) => new RegExp(`\\b${name}\\b`).test(node.getText(file)));
  // A type of this file that is built from a generated type is one too, however many steps it takes.
  for (let changed = true; changed; ) {
    changed = false;
    for (const statement of file.statements) {
      if ((ts.isTypeAliasDeclaration(statement) || ts.isInterfaceDeclaration(statement)) && !flagged.has(statement.name.text) && mentions(statement)) {
        flagged.add(statement.name.text);
        changed = true;
      }
    }
  }
  const found: GlobalUse[] = [];
  const line = (node: ts.Node) => file.getLineAndCharacterOfPosition(node.getStart(file)).line + 1;
  const isCreateContext = (call: ts.CallExpression) => /(^|\.)createContext$/.test(call.expression.getText(file));
  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && isCreateContext(node) && (node.typeArguments ?? []).some(mentions)) {
      const parent = node.parent;
      found.push({ name: ts.isVariableDeclaration(parent) ? parent.name.getText(file) : "(anonymous)", kind: "context", line: line(node) });
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  for (const statement of file.statements) {
    if (!ts.isVariableStatement(statement)) continue;
    for (const declaration of statement.declarationList.declarations) {
      const annotated = declaration.type !== undefined && mentions(declaration.type);
      const init = declaration.initializer;
      const generic = init !== undefined && ts.isNewExpression(init) && (init.typeArguments ?? []).some(mentions);
      const alreadyContext = found.some((f) => f.line === line(declaration.initializer ?? declaration));
      if ((annotated || generic) && !alreadyContext) found.push({ name: declaration.name.getText(file), kind: "module-variable", line: line(declaration) });
    }
  }
  return found;
}

/** What is allowed, with the reason. An entry that no longer matches anything fails the test: the list cannot go stale. */
const ALLOWED = [
  {
    file: "src/components/app/diff/rows.ts",
    name: "NO_RANGES",
    reason: "A frozen empty array that stands for 'no inline ranges' so that a row without any shares one value. It is never written to and holds no server data; its type is the one of the ranges it stands in for.",
  },
  {
    file: "src/live/events.ts",
    name: "CHECKS",
    reason: "A table of validators keyed by the kinds of event the contract has (its type is `Record<DomainEvent['type'], Check>`, so a kind without a row does not compile). It holds functions, never an event, and the stream's events are handed on and not kept.",
  },
  {
    file: "src/auth/session.tsx",
    name: "SessionContext",
    reason: "A projection of the `me` query (src/features/access/queries.ts), rebuilt from the QueryClient on every change and holding no copy of its own: it exists so that a component under the router reads the session without opening a query of its own. The store of record is the QueryClient.",
  },
] as const;

describe("findServerStateInGlobals (an APPROXIMATION: it reads declared types, one file at a time)", () => {
  const model = 'import type { Me, SwimlaneSummary } from "@/api/generated/model";\n';

  it.each([
    ["a context typed with a generated type", `${model}export const Ctx = createContext<Me | null>(null);`, "context"],
    ["a context typed with a type built from one, in the same file", `${model}type State = { status: "ready"; me: Me };\nexport const Ctx = createContext<State>({ status: "none" } as State);`, "context"],
    ["a context made with React.createContext", `import * as React from "react";\n${model}export const Ctx = React.createContext<SwimlaneSummary[]>([]);`, "context"],
    ["a module variable with an annotation", `${model}let current: Me | undefined;`, "module-variable"],
    ["a module cache typed with a generated type", `${model}const cache = new Map<string, SwimlaneSummary>();`, "module-variable"],
    ["a module variable typed through two steps of aliases", `${model}type A = { x: Me };\ntype B = { a: A };\nexport const store: B | null = null;`, "module-variable"],
    ["a namespace import", `import type * as Api from "@/api/generated/model";\nlet last: Api.Me | null = null;`, "module-variable"],
  ])("finds %s", (_name, code, kind) => {
    const found = findServerStateInGlobals(code);
    expect(found.map((f) => f.kind)).toEqual([kind]);
  });

  it.each([
    ["a context of something that is not server data", `import { createContext } from "react";\nexport const Ctx = createContext<boolean>(false);`],
    ["a module variable of a local type", `type Prefs = { dense: boolean };\nlet prefs: Prefs = { dense: false };`],
    ["a variable inside a function (it is not global)", `${model}export function f() { const me: Me | null = null; return me; }`],
    ["a component prop type", `${model}export function Row({ me }: { me: Me }) { return null; }`],
    ["an exported type", `${model}export type Wrapper = { me: Me };`],
    ["a constant typed with an enum of the contract (a vocabulary, not a response)", `import type { Role } from "@/api/generated/model";\nexport const ORDER: readonly Role[] = [];`],
    ["a name that only looks like a generated type", `${model}let Meeting: string | null = null;`],
    ["a type from another module", `import type { Role } from "./roles";\nlet r: Role | null = null;`],
  ])("does not flag %s", (_name, code) => {
    expect(findServerStateInGlobals(code)).toEqual([]);
  });

  it("does not see a type that is only inferred (the approximation, stated)", () => {
    expect(findServerStateInGlobals(`import { getMe } from "@/api/generated/endpoints/me/me";\nlet me = await getMe();`)).toEqual([]);
  });
});

describe("the app's own source", () => {
  const sources = productSources();
  const findings = sources.flatMap(({ path, text }) => findServerStateInGlobals(text, path).map((f) => ({ file: path, name: f.name, kind: f.kind })));

  it("has no context and no module-level variable typed with the generated API, apart from the ones listed with a reason", () => {
    const unexpected = findings.filter((f) => !ALLOWED.some((a) => a.file === f.file && a.name === f.name));
    expect(unexpected).toEqual([]);
  });

  it("lists only what still exists, and gives a reason for each", () => {
    for (const entry of ALLOWED) {
      expect(findings.some((f) => f.file === entry.file && f.name === entry.name), `${entry.file} ${entry.name}`).toBe(true);
      expect(entry.reason.length).toBeGreaterThan(80);
    }
    expect(ALLOWED).toHaveLength(3);
  });

  it("holds the session in the one place it is allowed to: the QueryClient is where `me` lives", () => {
    const session = sources.find((f) => f.path === "src/auth/session.tsx");
    expect(session?.text).toMatch(/useQuery\(\{ \.\.\.meQuery\(\)/);
  });
});
