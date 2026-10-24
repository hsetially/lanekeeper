import ts from "typescript";
import { describe, expect, it } from "vitest";
import { productSources } from "@/test/source-files";

// A mutation's result (`const save = useMutation(...)`) is a new object on every render. Listed whole in the dependencies of an
// effect or a memo, it makes the hook run again on every render, and an effect that sets state then renders without end: the
// raise-PR dialog did exactly that while it was closed. What is stable is `mutate`, `mutateAsync` and `reset` (take them out of the
// result first: `const { mutate, reset } = save`), and the primitives (`save.isPending`).
//
// This reads the source: it finds a variable declared from `useMutation`, `useSecretMutation` or `useQuery`, and says where it is
// listed whole in the dependency array of `useEffect`, `useLayoutEffect`, `useMemo`, `useCallback` or `useImperativeHandle`.
// An APPROXIMATION: one file at a time, the declared name only (a result passed through another variable is not followed).

const HOOKS = new Set(["useEffect", "useLayoutEffect", "useMemo", "useCallback", "useImperativeHandle"]);
const RESULT_HOOKS = new Set(["useMutation", "useSecretMutation", "useQuery", "useInfiniteQuery"]);

export function findResultsInDependencies(source: string, fileName = "x.tsx"): Array<{ name: string; hook: string; line: number }> {
  const file = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, true, fileName.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const results = new Set<string>();
  const collect = (node: ts.Node): void => {
    if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer && ts.isCallExpression(node.initializer) && RESULT_HOOKS.has(node.initializer.expression.getText(file))) {
      results.add(node.name.text);
    }
    ts.forEachChild(node, collect);
  };
  collect(file);
  const found: Array<{ name: string; hook: string; line: number }> = [];
  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && HOOKS.has(node.expression.getText(file))) {
      const last = node.arguments[node.arguments.length - 1];
      if (last !== undefined && ts.isArrayLiteralExpression(last)) {
        for (const element of last.elements) {
          if (ts.isIdentifier(element) && results.has(element.text)) found.push({ name: element.text, hook: node.expression.getText(file), line: file.getLineAndCharacterOfPosition(element.getStart(file)).line + 1 });
        }
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  return found;
}

describe("findResultsInDependencies (the scanner itself)", () => {
  it.each([
    ["an effect", "const save = useMutation(options);\nuseEffect(() => { if (!open) save.reset(); }, [open, save]);", "useEffect"],
    ["a memo", "const save = useMutation(options);\nconst x = useMemo(() => save.isPending, [save]);", "useMemo"],
    ["a callback", "const q = useQuery(options);\nconst go = useCallback(() => q.refetch(), [q]);", "useCallback"],
    ["a secret mutation", "const put = useSecretMutation(options);\nuseEffect(() => undefined, [put]);", "useEffect"],
  ])("finds the result of %s listed whole", (_name, code, hook) => {
    expect(findResultsInDependencies(code).map((f) => f.hook)).toEqual([hook]);
  });

  it.each([
    ["mutate and reset taken out of the result", "const save = useMutation(options);\nconst { mutate, reset } = save;\nuseEffect(() => { if (!open) reset(); }, [open, mutate, reset]);"],
    ["a property of the result", "const save = useMutation(options);\nuseEffect(() => undefined, [save.isPending, save.mutate]);"],
    ["something else with the same shape", "const save = makeSave();\nuseEffect(() => undefined, [save]);"],
    ["a result that is not in the list", "const save = useMutation(options);\nuseEffect(() => undefined, [open]);"],
  ])("does not flag %s", (_name, code) => {
    expect(findResultsInDependencies(code)).toEqual([]);
  });
});

describe("the app's own source", () => {
  it("lists no mutation or query result whole in the dependencies of an effect or a memo", () => {
    const found = productSources().flatMap(({ path, text }) => findResultsInDependencies(text, path).map((f) => `${path}:${f.line} ${f.hook} lists ${f.name}`));
    expect(found).toEqual([]);
  });
});
