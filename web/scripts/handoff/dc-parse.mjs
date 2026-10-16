// Static reader for the Claude Design handoff files (design/handoff/screens/*.dc.html).
//
// Nothing from the handoff is ever executed: no eval, no new Function, no browser. The page script is
// parsed with acorn and the few literals the extractors need (token tables, route and key tables,
// the States index) are computed by a small interpreter over an allow-list of syntax and methods.
// Anything outside the list throws StaticEvalError, so a changed handoff fails loudly instead of
// running something unexpected.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as acorn from "acorn";

const HERE = dirname(fileURLToPath(import.meta.url));
export const WEB_ROOT = join(HERE, "..", "..");
export const HANDOFF_DIR = join(WEB_ROOT, "..", "design", "handoff", "screens");

export class StaticEvalError extends Error {
  constructor(message, node) {
    super(node && typeof node.start === "number" ? `${message} (offset ${node.start})` : message);
    this.name = "StaticEvalError";
  }
}

// ---------------------------------------------------------------------------------------------
// Reading files

export function readHandoffFile(name) {
  return readFileSync(join(HANDOFF_DIR, name), "utf8");
}

const NAMED_ENTITIES = { quot: '"', amp: "&", lt: "<", gt: ">", apos: "'", nbsp: "\u00A0" };

export function decodeEntities(text) {
  return text.replace(/&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);/g, (whole, body) => {
    if (body[0] === "#") {
      const code = body[1] === "x" ? Number.parseInt(body.slice(2), 16) : Number.parseInt(body.slice(1), 10);
      return Number.isFinite(code) ? String.fromCodePoint(code) : whole;
    }
    return NAMED_ENTITIES[body] ?? whole;
  });
}

const SCRIPT_RE = /<script\s+type="text\/x-dc"\s+data-dc-script([^>]*)>([\s\S]*?)<\/script>/g;

// The page script of a .dc.html file: its code and the declared props (data-props, JSON).
export function extractScript(html) {
  const matches = [...html.matchAll(SCRIPT_RE)];
  if (matches.length === 0) throw new Error("no page script (text/x-dc, data-dc-script) in the file");
  if (matches.length > 1) throw new Error("more than one page script in the file");
  const [, attrs, code] = matches[0];
  const propsMatch = /data-props="([^"]*)"/.exec(attrs);
  const props = propsMatch ? JSON.parse(decodeEntities(propsMatch[1])) : {};
  return { code, props };
}

export function parseScript(code) {
  return acorn.parse(code, { ecmaVersion: "latest", sourceType: "script" });
}

// ---------------------------------------------------------------------------------------------
// AST helpers

function walk(node, visit) {
  if (!node || typeof node.type !== "string") return;
  visit(node);
  for (const key of Object.keys(node)) {
    const value = node[key];
    if (Array.isArray(value)) {
      for (const item of value) if (item && typeof item === "object") walk(item, visit);
    } else if (value && typeof value === "object") {
      walk(value, visit);
    }
  }
}

// name -> every `const name = init` declaration with a plain identifier, at any depth.
export function collectConsts(ast) {
  const table = new Map();
  walk(ast, (node) => {
    if (node.type !== "VariableDeclarator" || node.id.type !== "Identifier" || !node.init) return;
    const list = table.get(node.id.name) ?? [];
    list.push(node.init);
    table.set(node.id.name, list);
  });
  return table;
}

export function findCalls(ast, calleeName) {
  const calls = [];
  walk(ast, (node) => {
    if (node.type === "CallExpression" && node.callee.type === "Identifier" && node.callee.name === calleeName) {
      calls.push(node);
    }
  });
  return calls;
}

// ---------------------------------------------------------------------------------------------
// The interpreter

const FORBIDDEN_KEYS = new Set(["__proto__", "constructor", "prototype"]);

const ARRAY_METHODS = new Set(["map", "flatMap", "filter", "some", "every", "find", "includes", "indexOf", "join", "slice", "concat"]);
const CALLBACK_METHODS = new Set(["map", "flatMap", "filter", "some", "every", "find"]);
const STRING_METHODS = new Set(["padStart", "padEnd", "startsWith", "endsWith", "includes", "slice", "split", "toLowerCase", "toUpperCase", "trim", "indexOf"]);

const closures = new WeakSet();

const GLOBALS = {
  undefined,
  String: (value) => String(value),
  Number: (value) => Number(value),
  Object: Object.freeze({
    fromEntries: (entries) => Object.fromEntries(entries),
    entries: (value) => Object.entries(value),
    keys: (value) => Object.keys(value),
  }),
};
for (const fn of [GLOBALS.String, GLOBALS.Number]) closures.add(fn);

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value) && Object.getPrototypeOf(value) === Object.prototype;
}

function refuse(node, what) {
  throw new StaticEvalError(`${what} is not allowed: ${node.type}`, node);
}

class Scope {
  constructor(parent) {
    this.parent = parent;
    this.vars = new Map();
  }
  lookup(name) {
    if (this.vars.has(name)) return { found: true, value: this.vars.get(name) };
    return this.parent ? this.parent.lookup(name) : { found: false };
  }
}

function newContext(opts) {
  return { consts: opts.consts, maxSteps: opts.maxSteps ?? 2_000_000, steps: 0, memo: new Map(), resolving: new Set() };
}

function tick(ctx, node) {
  ctx.steps += 1;
  if (ctx.steps > ctx.maxSteps) throw new StaticEvalError("step budget exceeded", node);
}

function resolveConst(name, ctx, node) {
  if (ctx.memo.has(name)) return ctx.memo.get(name);
  const inits = ctx.consts?.get(name);
  if (!inits) throw new StaticEvalError(`unknown identifier ${name}`, node);
  if (inits.length > 1) throw new StaticEvalError(`ambiguous const ${name}: ${inits.length} declarations`, node);
  if (ctx.resolving.has(name)) throw new StaticEvalError(`circular const ${name}`, node);
  ctx.resolving.add(name);
  try {
    const value = ev(inits[0], new Scope(null), ctx);
    ctx.memo.set(name, value);
    return value;
  } finally {
    ctx.resolving.delete(name);
  }
}

function bindPattern(pattern, value, scope, ctx) {
  switch (pattern.type) {
    case "Identifier":
      scope.vars.set(pattern.name, value);
      return;
    case "ArrayPattern":
      if (!Array.isArray(value)) throw new StaticEvalError("array pattern needs an array", pattern);
      pattern.elements.forEach((el, i) => {
        if (el) bindPattern(el, value[i], scope, ctx);
      });
      return;
    case "ObjectPattern":
      if (!isPlainObject(value)) throw new StaticEvalError("object pattern needs a plain object", pattern);
      for (const prop of pattern.properties) {
        if (prop.type !== "Property" || prop.computed || prop.key.type !== "Identifier") refuse(prop, "pattern");
        if (FORBIDDEN_KEYS.has(prop.key.name)) refuse(prop, "forbidden key");
        bindPattern(prop.value, Object.hasOwn(value, prop.key.name) ? value[prop.key.name] : undefined, scope, ctx);
      }
      return;
    case "AssignmentPattern":
      bindPattern(pattern.left, value === undefined ? ev(pattern.right, scope, ctx) : value, scope, ctx);
      return;
    default:
      refuse(pattern, "pattern");
  }
}

function makeClosure(fn, scope, ctx) {
  const run = (...args) => {
    const local = new Scope(scope);
    fn.params.forEach((param, i) => bindPattern(param, args[i], local, ctx));
    if (fn.expression) return ev(fn.body, local, ctx);
    for (const stmt of fn.body.body) {
      tick(ctx, stmt);
      if (stmt.type === "VariableDeclaration") {
        if (stmt.kind === "var") refuse(stmt, "var");
        for (const decl of stmt.declarations) {
          if (!decl.init) refuse(decl, "declaration without value");
          bindPattern(decl.id, ev(decl.init, local, ctx), local, ctx);
        }
      } else if (stmt.type === "ReturnStatement") {
        return stmt.argument ? ev(stmt.argument, local, ctx) : undefined;
      } else {
        refuse(stmt, "statement");
      }
    }
    return undefined;
  };
  closures.add(run);
  return run;
}

function propertyKey(prop, scope, ctx) {
  let key;
  if (prop.computed) key = ev(prop.key, scope, ctx);
  else if (prop.key.type === "Identifier") key = prop.key.name;
  else if (prop.key.type === "Literal") key = prop.key.value;
  else refuse(prop, "property key");
  key = String(key);
  if (FORBIDDEN_KEYS.has(key)) throw new StaticEvalError(`forbidden key ${key}`, prop);
  return key;
}

function member(objectValue, key, node) {
  if (typeof key === "string" && FORBIDDEN_KEYS.has(key)) throw new StaticEvalError(`forbidden key ${key}`, node);
  if (Array.isArray(objectValue) || typeof objectValue === "string") {
    if (key === "length") return objectValue.length;
    if (typeof key === "number" || (typeof key === "string" && /^\d+$/.test(key))) return objectValue[Number(key)];
    return undefined;
  }
  if (isPlainObject(objectValue)) return Object.hasOwn(objectValue, key) ? objectValue[key] : undefined;
  if (objectValue === GLOBALS.Object) return Object.hasOwn(GLOBALS.Object, key) ? GLOBALS.Object[key] : undefined;
  if (objectValue === undefined || objectValue === null) throw new StaticEvalError("member access on nullish value", node);
  throw new StaticEvalError("member access on this kind of value is not allowed", node);
}

function callMethod(node, scope, ctx) {
  const callee = node.callee;
  const receiver = ev(callee.object, scope, ctx);
  const name = callee.computed ? ev(callee.property, scope, ctx) : callee.property.name;
  if (typeof name !== "string") refuse(node, "computed method name");
  const args = node.arguments.map((arg) => {
    if (arg.type === "SpreadElement") refuse(arg, "spread in call");
    return ev(arg, scope, ctx);
  });
  if (receiver === GLOBALS.Object) {
    if (!["fromEntries", "entries", "keys"].includes(name)) throw new StaticEvalError(`Object.${name} is not allowed`, node);
    return GLOBALS.Object[name](...args);
  }
  let allowed;
  if (Array.isArray(receiver)) allowed = ARRAY_METHODS;
  else if (typeof receiver === "string") allowed = STRING_METHODS;
  if (!allowed?.has(name)) throw new StaticEvalError(`method ${name} is not allowed on this value`, node);
  if (CALLBACK_METHODS.has(name) && Array.isArray(receiver)) {
    if (typeof args[0] !== "function" || !closures.has(args[0])) throw new StaticEvalError("callback must be an arrow function from the page script (closure)", node);
  }
  if (name === "concat") {
    for (const a of args) if (!Array.isArray(a) && typeof a !== "string" && typeof a !== "number") refuse(node, "concat argument");
  }
  return Function.prototype.apply.call(receiver[name], receiver, args);
}

function ev(node, scope, ctx) {
  tick(ctx, node);
  switch (node.type) {
    case "Literal":
      if (node.regex) throw new StaticEvalError("regular expression literal is not allowed", node);
      if (typeof node.value === "bigint") refuse(node, "bigint");
      return node.value;
    case "TemplateLiteral": {
      let out = node.quasis[0].value.cooked;
      node.expressions.forEach((expr, i) => {
        const value = ev(expr, scope, ctx);
        if (!["string", "number", "boolean"].includes(typeof value)) throw new StaticEvalError("template value must be a primitive", expr);
        out += String(value) + node.quasis[i + 1].value.cooked;
      });
      return out;
    }
    case "Identifier": {
      if (node.name === "undefined") return undefined;
      const local = scope.lookup(node.name);
      if (local.found) return local.value;
      if (Object.hasOwn(GLOBALS, node.name)) return GLOBALS[node.name];
      return resolveConst(node.name, ctx, node);
    }
    case "ArrayExpression": {
      const out = [];
      for (const el of node.elements) {
        if (!el) out.push(undefined);
        else if (el.type === "SpreadElement") {
          const spread = ev(el.argument, scope, ctx);
          if (!Array.isArray(spread)) throw new StaticEvalError("spread needs an array", el);
          out.push(...spread);
        } else out.push(ev(el, scope, ctx));
      }
      return out;
    }
    case "ObjectExpression": {
      const out = {};
      for (const prop of node.properties) {
        if (prop.type === "SpreadElement") {
          const spread = ev(prop.argument, scope, ctx);
          if (!isPlainObject(spread)) throw new StaticEvalError("object spread needs a plain object", prop);
          Object.assign(out, spread);
        } else if (prop.type === "Property" && prop.kind === "init" && !prop.method) {
          out[propertyKey(prop, scope, ctx)] = ev(prop.value, scope, ctx);
        } else refuse(prop, "object member");
      }
      return out;
    }
    case "MemberExpression": {
      if (node.optional) refuse(node, "optional chaining");
      const objectValue = ev(node.object, scope, ctx);
      const key = node.computed ? ev(node.property, scope, ctx) : node.property.name;
      return member(objectValue, key, node);
    }
    case "CallExpression": {
      if (node.optional) refuse(node, "optional call");
      if (node.callee.type === "MemberExpression") return callMethod(node, scope, ctx);
      if (node.callee.type === "Identifier") {
        const local = scope.lookup(node.callee.name);
        const fn = local.found ? local.value : Object.hasOwn(GLOBALS, node.callee.name) ? GLOBALS[node.callee.name] : undefined;
        if (typeof fn === "function" && closures.has(fn)) return fn(...node.arguments.map((a) => ev(a, scope, ctx)));
      }
      throw new StaticEvalError("call is not allowed", node);
    }
    case "ArrowFunctionExpression":
      if (node.async || node.generator) refuse(node, "async or generator function");
      return makeClosure(node, scope, ctx);
    case "BinaryExpression": {
      const left = ev(node.left, scope, ctx);
      const right = ev(node.right, scope, ctx);
      switch (node.operator) {
        case "+": return left + right;
        case "-": return left - right;
        case "*": return left * right;
        case "/": return left / right;
        case "%": return left % right;
        case "===": return left === right;
        case "!==": return left !== right;
        case "==": return left == right;
        case "!=": return left != right;
        case "<": return left < right;
        case ">": return left > right;
        case "<=": return left <= right;
        case ">=": return left >= right;
        default: throw new StaticEvalError(`operator ${node.operator} is not allowed`, node);
      }
    }
    case "LogicalExpression": {
      const left = ev(node.left, scope, ctx);
      if (node.operator === "&&") return left ? ev(node.right, scope, ctx) : left;
      if (node.operator === "||") return left ? left : ev(node.right, scope, ctx);
      if (node.operator === "??") return left ?? ev(node.right, scope, ctx);
      throw new StaticEvalError(`operator ${node.operator} is not allowed`, node);
    }
    case "ConditionalExpression":
      return ev(node.test, scope, ctx) ? ev(node.consequent, scope, ctx) : ev(node.alternate, scope, ctx);
    case "UnaryExpression": {
      if (!["!", "-", "+"].includes(node.operator)) throw new StaticEvalError(`operator ${node.operator} is not allowed`, node);
      const value = ev(node.argument, scope, ctx);
      return node.operator === "!" ? !value : node.operator === "-" ? -value : +value;
    }
    default:
      return refuse(node, "syntax");
  }
}

// Evaluate one expression node. `opts.consts` is the table from collectConsts.
export function evalExpression(node, opts = {}) {
  return ev(node, new Scope(null), newContext(opts));
}

// Parse `code`, then compute the single const called `name`.
export function evalConst(code, name, opts = {}) {
  const ast = parseScript(code);
  const consts = collectConsts(ast);
  const inits = consts.get(name);
  if (!inits) throw new StaticEvalError(`no const ${name}`);
  if (inits.length > 1) throw new StaticEvalError(`ambiguous const ${name}`);
  return evalExpression(inits[0], { ...opts, consts });
}

// Convenience for the extractors: the page script of a handoff file, parsed once.
export function loadPage(fileName) {
  const html = readHandoffFile(fileName);
  const { code, props } = extractScript(html);
  const ast = parseScript(code);
  const consts = collectConsts(ast);
  return {
    fileName,
    html,
    props,
    ast,
    consts,
    evalConst(name, opts = {}) {
      const inits = consts.get(name);
      if (!inits) throw new StaticEvalError(`no const ${name} in ${fileName}`);
      if (inits.length > 1) throw new StaticEvalError(`ambiguous const ${name} in ${fileName}`);
      return evalExpression(inits[0], { ...opts, consts });
    },
    calls: (name) => findCalls(ast, name),
    evalArg: (call, index, opts = {}) => evalExpression(call.arguments[index], { ...opts, consts }),
  };
}
