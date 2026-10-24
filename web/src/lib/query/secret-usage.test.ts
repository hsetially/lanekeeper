import { dirname, posix } from "node:path";
import { MutationCache, QueryCache, QueryClient } from "@tanstack/react-query";
import ts from "typescript";
import { describe, expect, it } from "vitest";
import { productSources, type SourceFile } from "@/test/source-files";
import { SECRET_OPERATIONS } from "./invalidate";

// "A secret-bearing operation can only be called through `useSecretMutation`" (S7, S8, S21 in the browser; plan, "Secret-bearing
// mutations"). A plain `useMutation` keeps its variables and its result in the MutationCache; `useSecretMutation` has gcTime 0,
// forgets the call in a `finally`, never retries, and gives the answer to the caller alone. Nothing but a rule stops the next screen
// from using the plain hook with a token, so this reads the source, the way keys-usage.test.ts and mutation-deps.test.ts do.
//
// The list is NOT written again here: it is `SECRET_OPERATIONS` of src/lib/query/invalidate.ts, the one list that the fetch layer's
// never-retry paths and the optimistic registry are already checked against. Add an operation there and this test guards it.
//
// What is checked, per operation (`createMcpToken`, ...):
//  1. The generated function may be referenced only as (a) an argument of `useSecretMutation(...)`, or (b) inside a REQUEST: a
//     top-level plain function (no JSX, no hook, nothing from @tanstack/react-query or src/lib/query) that calls the operation and
//     returns its answer, such as `createMcpTokenRequest = (body) => createMcpToken(body)`.
//  2. A request is a secret too: it may be referenced, in any file, only as an argument of `useSecretMutation(...)` or inside another
//     request. A screen that passes it to `useMutation`, `useQuery`, `fetchQuery`, `writeMutation` or calls it from a handler fails.
//  3. The generated mutation hook and options of the operation (`useCreateMcpToken`, `getCreateMcpTokenMutationOptions`, ...) are
//     never used, and no `writeMutation({ operation: "<secret>" })` is made.
// An APPROXIMATION in the way the other source scans are: names are followed through imports, not through the type system, and a
// request that is stored in a variable first is a violation (keep the reference in the `useSecretMutation(` call).

const SECRET = new Set<string>(SECRET_OPERATIONS);
const cap = (name: string): string => name.charAt(0).toUpperCase() + name.slice(1);
const GENERATED_HELPERS = new Set(SECRET_OPERATIONS.flatMap((op) => [`use${cap(op)}`, `get${cap(op)}MutationOptions`, `get${cap(op)}MutationKey`]));
const SECRET_HOOK = "useSecretMutation";
const QUERY_LAYER = /^(@tanstack\/react-query|@\/lib\/query\/(?!secret-mutation$).+)$/;
// The methods of the caches themselves, read from the library and not written out: a call to one of them in a request would keep the
// answer in a cache (`client.fetchQuery(...)`, `cache.build(...)`).
const CACHE_METHODS = new Set([QueryClient, QueryCache, MutationCache].flatMap((cache) => Object.getOwnPropertyNames(cache.prototype)).filter((name) => name !== "constructor"));

type Parsed = { path: string; sf: ts.SourceFile };
type Scan = { violations: string[]; guarded: Map<string, number>; requests: Map<string, Set<string>> };

function parse({ path, text }: SourceFile): Parsed {
  return { path, sf: ts.createSourceFile(path, text, ts.ScriptTarget.Latest, true, path.endsWith("x") ? ts.ScriptKind.TSX : ts.ScriptKind.TS) };
}

/** The file a module specifier names, among the files being scanned (`@/x/y`, `./y`, with or without an index). */
function resolveModule(from: string, specifier: string, known: Set<string>): string | undefined {
  const base = specifier.startsWith("@/") ? `src/${specifier.slice(2)}` : specifier.startsWith(".") ? posix.normalize(posix.join(dirname(from), specifier)) : undefined;
  if (base === undefined) return undefined;
  return [`${base}.ts`, `${base}.tsx`, `${base}/index.ts`, `${base}/index.tsx`].find((candidate) => known.has(candidate));
}

const isGeneratedModule = (specifier: string): boolean => /(^|\/)api\/generated(\/|$)/.test(specifier);

/** Whether an identifier is a use of a name, not the place that declares one (a parameter, an import, a key, a member, a type). */
function isUse(node: ts.Identifier): boolean {
  const parent = node.parent;
  if (ts.isImportSpecifier(parent) || ts.isImportClause(parent) || ts.isNamespaceImport(parent)) return false;
  if ((ts.isVariableDeclaration(parent) || ts.isFunctionDeclaration(parent) || ts.isParameter(parent) || ts.isBindingElement(parent)) && parent.name === node) return false;
  if ((ts.isPropertyAssignment(parent) || ts.isPropertySignature(parent) || ts.isJsxAttribute(parent) || ts.isMethodSignature(parent)) && parent.name === node) return false;
  if (ts.isPropertyAccessExpression(parent) && parent.name === node) return false;
  for (let up: ts.Node = parent; !ts.isSourceFile(up); up = up.parent) if (ts.isTypeNode(up)) return false;
  return true;
}

/** The top-level statement a node is in. */
function topLevel(node: ts.Node): ts.Statement {
  let up: ts.Node = node;
  while (!ts.isSourceFile(up.parent)) up = up.parent;
  return up as ts.Statement;
}

function declaredFunctions(statement: ts.Statement): string[] | undefined {
  if (ts.isFunctionDeclaration(statement) && statement.name) return [statement.name.text];
  if (!ts.isVariableStatement(statement)) return undefined;
  const names: string[] = [];
  for (const declaration of statement.declarationList.declarations) {
    const fn = declaration.initializer;
    if (!ts.isIdentifier(declaration.name) || !fn || !(ts.isArrowFunction(fn) || ts.isFunctionExpression(fn))) return undefined;
    names.push(declaration.name.text);
  }
  return names;
}

/** A request is plain: no JSX, no hook, no call of a cache method, and nothing of react-query or of the query layer in its body. */
function isPlainRequest(statement: ts.Statement, queryLayerNames: Set<string>): boolean {
  let plain = true;
  const visit = (node: ts.Node): void => {
    if (!plain) return;
    if (ts.isJsxElement(node) || ts.isJsxSelfClosingElement(node) || ts.isJsxFragment(node)) plain = false;
    if (ts.isCallExpression(node)) {
      const callee = ts.isIdentifier(node.expression) ? node.expression.text : ts.isPropertyAccessExpression(node.expression) ? node.expression.name.text : "";
      if (/^use[A-Z]/.test(callee) || (ts.isPropertyAccessExpression(node.expression) && CACHE_METHODS.has(callee))) plain = false;
    }
    if (ts.isIdentifier(node) && isUse(node) && queryLayerNames.has(node.text)) plain = false;
    ts.forEachChild(node, visit);
  };
  visit(statement);
  return plain;
}

/** Whether `node` lies inside an argument of a `useSecretMutation(...)` call. */
function insideSecretHook(node: ts.Node): boolean {
  let child: ts.Node = node;
  while (!ts.isSourceFile(child.parent)) {
    const up = child.parent;
    const was = child;
    if (ts.isCallExpression(up) && ts.isIdentifier(up.expression) && up.expression.text === SECRET_HOOK && up.arguments.some((argument) => argument === was)) return true;
    child = up;
  }
  return false;
}

export function scanSecretUsage(files: readonly SourceFile[]): Scan {
  const parsed = files.map(parse);
  const known = new Set(parsed.map((file) => file.path));
  const violations: string[] = [];
  const guarded = new Map<string, number>();
  // Per file: the names of its top-level requests, each with the operations whose secret passes through it.
  const requests = new Map<string, Map<string, Set<string>>>();
  const at = (file: Parsed, node: ts.Node): string => `${file.path}:${file.sf.getLineAndCharacterOfPosition(node.getStart(file.sf)).line + 1}`;
  const seen = new Set<string>();
  const report = (file: Parsed, node: ts.Node, what: string): void => {
    const line = `${at(file, node)} ${what}`;
    if (!seen.has(line)) {
      seen.add(line);
      violations.push(line);
    }
  };

  // Which local names stand for a secret in each file: the generated operations it imports, and the requests it imports.
  const localSecrets = (file: Parsed): { names: Map<string, Set<string>>; namespaces: Set<string>; queryLayer: Set<string> } => {
    const names = new Map<string, Set<string>>();
    const namespaces = new Set<string>();
    const queryLayer = new Set<string>();
    for (const statement of file.sf.statements) {
      if (!ts.isImportDeclaration(statement) || !ts.isStringLiteral(statement.moduleSpecifier)) continue;
      const specifier = statement.moduleSpecifier.text;
      const clause = statement.importClause;
      if (!clause || clause.isTypeOnly) continue;
      const bindings = clause.namedBindings;
      if (bindings && ts.isNamespaceImport(bindings) && isGeneratedModule(specifier)) namespaces.add(bindings.name.text);
      if (!bindings || !ts.isNamedImports(bindings)) continue;
      const target = resolveModule(file.path, specifier, known);
      for (const element of bindings.elements) {
        if (element.isTypeOnly) continue;
        const imported = (element.propertyName ?? element.name).text;
        const local = element.name.text;
        if (isGeneratedModule(specifier) && SECRET.has(imported)) names.set(local, new Set([imported]));
        else if (target !== undefined && requests.get(target)?.has(imported)) names.set(local, new Set(requests.get(target)?.get(imported)));
        else if (QUERY_LAYER.test(specifier)) queryLayer.add(local);
      }
    }
    for (const [name, ops] of requests.get(file.path) ?? []) names.set(name, new Set(ops));
    return { names, namespaces, queryLayer };
  };

  // Follow the requests to a fixed point: a request defined in one file and used in another is found on the next pass.
  for (let pass = 0, changed = true; changed && pass < 10; pass += 1) {
    changed = false;
    for (const file of parsed) {
      const { names, namespaces, queryLayer } = localSecrets(file);
      const mine = requests.get(file.path) ?? new Map<string, Set<string>>();
      const visit = (node: ts.Node): void => {
        let ops: Set<string> | undefined;
        let label = "";
        if (ts.isIdentifier(node) && isUse(node) && names.has(node.text)) {
          ops = names.get(node.text);
          label = node.text;
        } else if (ts.isPropertyAccessExpression(node) && ts.isIdentifier(node.expression) && namespaces.has(node.expression.text) && SECRET.has(node.name.text)) {
          ops = new Set([node.name.text]);
          label = node.getText(file.sf);
        }
        if (ops !== undefined && !(ts.isIdentifier(node) && ts.isPropertyAccessExpression(node.parent) && node.parent.name === node)) {
          if (insideSecretHook(node)) {
            for (const op of ops) guarded.set(op, (guarded.get(op) ?? 0) + 1);
          } else {
            const statement = topLevel(node);
            const declared = declaredFunctions(statement);
            if (declared && isPlainRequest(statement, queryLayer)) {
              for (const name of declared) {
                const known = mine.get(name) ?? new Set<string>();
                const before = known.size;
                for (const op of ops) known.add(op);
                mine.set(name, known);
                if (known.size !== before) changed = true;
              }
            } else {
              report(file, node, `uses "${label}" outside useSecretMutation (a secret-bearing operation: ${[...ops].join(", ")})`);
            }
          }
        }
        // The generated mutation hook or options of a secret operation, and a plain write that names one.
        if (ts.isIdentifier(node) && GENERATED_HELPERS.has(node.text) && isUse(node)) report(file, node, `uses the generated "${node.text}", which is a plain useMutation`);
        if (ts.isPropertyAssignment(node) && ts.isIdentifier(node.name) && node.name.text === "operation" && ts.isStringLiteralLike(node.initializer) && SECRET.has(node.initializer.text)) {
          report(file, node, `makes a plain write of "${node.initializer.text}", a secret-bearing operation`);
        }
        ts.forEachChild(node, visit);
      };
      visit(file.sf);
      if (mine.size > 0) requests.set(file.path, mine);
    }
  }

  const requestNames = new Map<string, Set<string>>();
  for (const [path, byName] of requests) for (const name of byName.keys()) requestNames.set(`${path}#${name}`, byName.get(name) ?? new Set());
  return { violations, guarded, requests: requestNames };
}

const LAYER_AND_HOOK = {
  generated: 'import { createMcpToken } from "@/api/generated/endpoints/me/me";\n',
  request: 'import { createMcpToken } from "@/api/generated/endpoints/me/me";\nexport const makeTokenRequest = (body: unknown) => createMcpToken(body);\n',
};

describe("the scan finds a wrong caller (it fails where it should)", () => {
  const scan = (files: Record<string, string>): string[] => scanSecretUsage(Object.entries(files).map(([path, text]) => ({ path, text }))).violations;

  it.each([
    [
      "a plain useMutation that calls the generated operation",
      { "src/features/x/Page.tsx": `${LAYER_AND_HOOK.generated}export function Page() { const m = useMutation({ mutationFn: (body: unknown) => createMcpToken(body) }); return null; }\n` },
    ],
    [
      "a plain useMutation given a request from another file",
      {
        "src/features/x/mutations.ts": LAYER_AND_HOOK.request,
        "src/features/x/Page.tsx": 'import { makeTokenRequest } from "./mutations";\nexport function Page() { const m = useMutation({ mutationFn: makeTokenRequest }); return null; }\n',
      },
    ],
    [
      "a request wrapped by a second function and then passed to useMutation",
      {
        "src/features/x/mutations.ts": `${LAYER_AND_HOOK.request}export const again = (body: unknown) => makeTokenRequest(body);\n`,
        "src/features/x/Page.tsx": 'import { again } from "@/features/x/mutations";\nexport function Page() { const m = useMutation({ mutationFn: (b: unknown) => again(b) }); return null; }\n',
      },
    ],
    [
      "a request called from an event handler instead of through the hook",
      {
        "src/features/x/mutations.ts": LAYER_AND_HOOK.request,
        "src/features/x/Page.tsx": 'import { makeTokenRequest } from "./mutations";\nexport function Page() { return <button onClick={() => void makeTokenRequest({})}>x</button>; }\n',
      },
    ],
    [
      "a request given to a query (its answer would sit in the query cache)",
      {
        "src/features/x/mutations.ts": LAYER_AND_HOOK.request,
        "src/features/x/Page.tsx": 'import { makeTokenRequest } from "./mutations";\nexport function Page() { useQuery({ queryKey: k, queryFn: () => makeTokenRequest({}) }); return null; }\n',
      },
    ],
    ["the generated hook of the operation", { "src/features/x/Page.tsx": 'import { useCreateMcpToken } from "@/api/generated/endpoints/me/me";\nexport function Page() { useCreateMcpToken(); return null; }\n' }],
    ["the generated mutation options of the operation", { "src/features/x/Page.tsx": 'import { getCreateMcpTokenMutationOptions } from "@/api/generated/endpoints/me/me";\nexport const o = getCreateMcpTokenMutationOptions();\n' }],
    ["a plain write that names the operation", { "src/features/x/mutations.ts": 'export const m = (client: unknown) => writeMutation(client, { operation: "createMcpToken", run: f, context: g });\n' }],
    ["a request built with the query layer inside it", { "src/features/x/mutations.ts": `${LAYER_AND_HOOK.generated}import { writeMutation } from "@/lib/query/write";\nexport const m = (client: unknown) => writeMutation(client, { run: (b: unknown) => createMcpToken(b) });\n` }],
    ["a request that is a hook", { "src/features/x/mutations.ts": `${LAYER_AND_HOOK.generated}export const useMake = () => useMutation({ mutationFn: (b: unknown) => createMcpToken(b) });\n` }],
    [
      "a request that stores its answer in a cache through the client",
      {
        "src/features/x/mutations.ts": `${LAYER_AND_HOOK.request}export const keep = (client: { fetchQuery: (o: unknown) => unknown }) => client.fetchQuery({ queryFn: () => makeTokenRequest({}) });\n`,
      },
    ],
    ["the operation held in an object", { "src/features/x/mutations.ts": `${LAYER_AND_HOOK.generated}export const ops = { make: createMcpToken };\n` }],
    ["the operation through a namespace import", { "src/features/x/Page.tsx": 'import * as me from "@/api/generated/endpoints/me/me";\nexport function Page() { useMutation({ mutationFn: me.createMcpToken }); return null; }\n' }],
    ["a re-export of a request", { "src/features/x/mutations.ts": `${LAYER_AND_HOOK.request}export { makeTokenRequest as other };\n` }],
  ] as Array<[string, Record<string, string>]>)("flags %s", (_name, files) => {
    expect(scan(files).length, JSON.stringify(Object.keys(files))).toBeGreaterThan(0);
  });

  it.each([
    [
      "a request passed to useSecretMutation",
      {
        "src/features/x/mutations.ts": LAYER_AND_HOOK.request,
        "src/features/x/Page.tsx": 'import { makeTokenRequest } from "./mutations";\nexport function Page() { const make = useSecretMutation(makeTokenRequest); return null; }\n',
      },
    ],
    [
      "a request called inside the function given to useSecretMutation",
      {
        "src/features/x/mutations.ts": LAYER_AND_HOOK.request,
        "src/features/x/Page.tsx": 'import { makeTokenRequest } from "./mutations";\nexport function Page() { const make = useSecretMutation((v: { id: string }) => makeTokenRequest(v)); return null; }\n',
      },
    ],
    [
      "the generated operation given straight to useSecretMutation",
      { "src/features/x/Page.tsx": `${LAYER_AND_HOOK.generated}export function Page() { useSecretMutation(createMcpToken); return null; }\n` },
    ],
    [
      "a request that calls another request, used through the hook",
      {
        "src/features/x/mutations.ts": `${LAYER_AND_HOOK.request}export const again = async (body: unknown) => { const answer = await makeTokenRequest(body); return answer; };\n`,
        "src/features/x/Page.tsx": 'import { again } from "./mutations";\nexport function Page() { useSecretMutation(again); return null; }\n',
      },
    ],
    [
      "the invalidation after a secret write (it names the operation as data, not as a call)",
      { "src/features/x/mutations.ts": 'export const after = (client: unknown) => invalidateAfterWrite(client, "createMcpToken", {});\n' },
    ],
    ["an ordinary write beside it", { "src/features/x/mutations.ts": 'export const m = (client: unknown) => writeMutation(client, { operation: "deleteGithubToken", run: f, context: g });\n' }],
  ] as Array<[string, Record<string, string>]>)("accepts %s", (_name, files) => {
    expect(scan(files)).toEqual([]);
  });

  it("reports the file, the line and the operation", () => {
    const [violation] = scan({ "src/features/x/Page.tsx": `${LAYER_AND_HOOK.generated}export function Page() {\n  useMutation({ mutationFn: createMcpToken });\n  return null;\n}\n` });
    expect(violation).toMatch(/^src\/features\/x\/Page\.tsx:3 uses "createMcpToken" outside useSecretMutation/);
    expect(violation).toMatch(/createMcpToken\)/);
  });
});

describe("the app's own source", () => {
  const sources = productSources();
  const result = scanSecretUsage(sources);

  it("calls every secret-bearing operation only through useSecretMutation", () => {
    expect(result.violations).toEqual([]);
  });

  it("is not empty by accident: every operation of the list reaches useSecretMutation through a request of the app", () => {
    expect(SECRET_OPERATIONS.length).toBeGreaterThanOrEqual(5);
    for (const op of SECRET_OPERATIONS) {
      expect(result.guarded.get(op) ?? 0, `${op} is passed to useSecretMutation (directly or as a request)`).toBeGreaterThan(0);
      const requestsOfOp = [...result.requests.entries()].filter(([, ops]) => ops.has(op)).map(([name]) => name);
      expect(requestsOfOp.length, `${op} has a request function`).toBeGreaterThan(0);
    }
  });

  it("is told about the list by the one place that holds it", () => {
    // If this list moved or were copied, the scan above would guard a stale one.
    const holders = sources.filter(({ text }) => /\bSECRET_OPERATIONS\s*=/.test(text)).map((f) => f.path);
    expect(holders).toEqual(["src/lib/query/invalidate.ts"]);
  });
});
