import { describe, expect, it } from "vitest";
import {
  StaticEvalError,
  collectConsts,
  decodeEntities,
  evalConst,
  evalExpression,
  extractScript,
  findCalls,
  parseScript,
} from "./dc-parse.mjs";

const run = (src, name = "out", opts = {}) => evalConst(src, name, opts);

describe("extractScript", () => {
  it("returns the code and the decoded data-props of the page script", () => {
    const html = `<x-dc></x-dc>
<script type="text/x-dc" data-dc-script data-props="{&quot;theme&quot;:{&quot;default&quot;:&quot;light&quot;,&quot;options&quot;:[&quot;light&quot;,&quot;dark&quot;]}}">
class Component {}
</script>`;
    const { code, props } = extractScript(html);
    expect(code.trim()).toBe("class Component {}");
    expect(props).toEqual({ theme: { default: "light", options: ["light", "dark"] } });
  });

  it("returns empty props when the script declares none", () => {
    const { props } = extractScript(`<script type="text/x-dc" data-dc-script>\nconst a = 1;\n</script>`);
    expect(props).toEqual({});
  });

  it("fails when there is no page script or more than one", () => {
    expect(() => extractScript("<p>nothing</p>")).toThrow(/no .*script/i);
    const two = `<script type="text/x-dc" data-dc-script>1</script><script type="text/x-dc" data-dc-script>2</script>`;
    expect(() => extractScript(two)).toThrow(/more than one/i);
  });
});

describe("decodeEntities", () => {
  it("decodes the entities the handoff writes into attributes", () => {
    expect(decodeEntities("&quot;a&quot; &amp; &lt;b&gt; &#39;c&#39; &#x27;d&#x27;")).toBe(`"a" & <b> 'c' 'd'`);
  });
});

describe("evalExpression: what it computes", () => {
  it("evaluates literals, arrays with spread, objects and template strings", () => {
    expect(run("const out = [1, 'a', true, null, ...[2, 3]];")).toEqual([1, "a", true, null, 2, 3]);
    expect(run("const k = 'x'; const out = { a: 1, b: [2], k, 'q-r': 3 };")).toEqual({ a: 1, b: [2], k: "x", "q-r": 3 });
    expect(run("const n = 'sync'; const out = `--status-${n}-fg`;")).toBe("--status-sync-fg");
  });

  it("resolves file-level consts lazily and in any order", () => {
    expect(run("const out = [B, A]; const A = 'a'; const B = A + 'b';")).toEqual(["ab", "a"]);
  });

  it("supports the array, string and Object helpers the handoff pages use", () => {
    expect(run("const out = [1, 2, 3].map(n => n * 2).filter(n => n > 2);")).toEqual([4, 6]);
    expect(run("const out = ['a', 'b'].flatMap(s => [s, s + s]);")).toEqual(["a", "aa", "b", "bb"]);
    expect(run("const out = String(3).padStart(2, '0');")).toBe("03");
    expect(run("const out = ['a', 'b'].includes('b') ? 'yes' : 'no';")).toBe("yes");
    expect(run("const out = Object.fromEntries([['a', 1], ['b', 2]]);")).toEqual({ a: 1, b: 2 });
    expect(run("const out = Object.keys({ a: 1, b: 2 }).join('|');")).toBe("a|b");
    expect(run("const out = 'a,b'.split(',').map(s => s.toUpperCase());")).toEqual(["A", "B"]);
  });

  it("supports destructured parameters, block bodies with const and return, and || with ??", () => {
    const src = `
      const W = { x: ['a'] };
      const out = [['a', 1], ['b', 2]].map(([k, v], i) => { const w = (W.x || []).includes(k); return { id: k + String(i + 1).padStart(2, '0'), v, w }; });`;
    expect(run(src)).toEqual([
      { id: "a01", v: 1, w: true },
      { id: "b02", v: 2, w: false },
    ]);
    expect(run("const out = ({ a, b }) => 0; const x = null ?? 'd';", "x")).toBe("d");
  });

  it("evaluates call arguments in place with findCalls", () => {
    const ast = parseScript("const R = '/'; const x = sec('keys', 'Keyboard', [R + 'a', '`b']);");
    const [call] = findCalls(ast, "sec");
    const consts = collectConsts(ast);
    expect(evalExpression(call.arguments[2], { consts })).toEqual(["/a", "`b"]);
  });
});

describe("evalExpression: what it refuses", () => {
  const refuse = (src, pattern) => {
    expect(() => run(src)).toThrow(StaticEvalError);
    expect(() => run(src)).toThrow(pattern);
  };

  it("never calls anything outside the allow-list", () => {
    refuse("const out = eval('1');", /call/i);
    refuse("const out = new Function('return 1')();", /call is not allowed/);
    refuse("const out = new Function('return 1');", /NewExpression/);
    refuse("const out = fetch('/x');", /call|unknown identifier/i);
    refuse("const out = process.exit(1);", /unknown identifier/i);
    refuse("const out = globalThis.process;", /unknown identifier/i);
    refuse("const out = [].map.call([], x => x);", /not allowed|call/i);
    refuse("const out = 'a'.constructor('return 1')();", /not allowed|call|forbidden/i);
    refuse("const out = require('fs');", /call is not allowed/i);
  });

  it("refuses prototype access, this, assignment and other statements", () => {
    refuse("const out = ({}).__proto__;", /forbidden/i);
    refuse("const out = ({})['constructor'];", /forbidden/i);
    refuse("const out = this.props.theme;", /ThisExpression/);
    refuse("let a = 1; const out = (a = 2);", /AssignmentExpression/);
    refuse("let a = 1; const out = a++;", /UpdateExpression/);
    refuse("const out = import('x');", /ImportExpression/);
    refuse("const out = /a/;", /regular expression/i);
    refuse("const out = [1].map(function (x) { return x; });", /FunctionExpression/);
    refuse("const out = [1].map(x => { while (true) {} });", /WhileStatement/);
  });

  it("refuses callbacks that are not interpreter closures, and ambiguous or missing consts", () => {
    refuse("const out = [1].map(Object.keys);", /closure|not allowed|function/i);
    refuse("const out = missing;", /unknown identifier/i);
    refuse("const a = 1; function f() { const a = 2; } const out = a;", /ambiguous/i);
  });

  it("stops runaway evaluation with a step budget", () => {
    const src = "const xs = [1,2,3,4,5,6,7,8,9,10]; const out = xs.flatMap(a => xs.flatMap(b => xs.flatMap(c => xs.map(d => d))));";
    expect(() => run(src, "out", { maxSteps: 500 })).toThrow(/step budget/i);
    expect(run(src, "out", { maxSteps: 1_000_000 })).toHaveLength(10_000);
  });
});
