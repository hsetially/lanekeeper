#!/usr/bin/env node
// Reads BuildKit's own plan for one target of deploy/sentinel/Dockerfile.build and refuses it if building the target would build any other
// stage (09/T8, review round 2; S7, S19, S20).
//
//   node deploy/sentinel/tests/tools-graph.mjs TARGET PROGRESS-FILE
//
// PROGRESS-FILE is the output of `docker buildx build --target TARGET --progress=rawjson` (one JSON object per line, on stderr). BuildKit
// lists every step of the target's whole dependency graph at the start of a build, named `[stage n/m] INSTRUCTION`, before it runs any of
// them, so the list is the answer to "what would this build run?" even when the build is then stopped or fails. A stage that the target does
// not need is never listed: the Dockerfile frontend drops it before it loads the stage's base image.
//
// The job that signs a release builds the `rpm-tools` target. If that graph holds a `[binary ...]` step, BuildKit runs `cargo build` there,
// and with it the build script and proc-macro of every crate in the sentinel's tree, in the job that receives the credential and the key.
//
// Exit codes: 0 only the target's own steps; 1 another stage's step, or a cargo step, is in the graph; 2 the file holds no step of the target
// (the build stopped before it listed the graph: nothing was proved, and nothing passes on nothing).
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const STEP = /^\[([A-Za-z0-9_.-]+) (\d+)\/(\d+)\] /;
// A Rust build tool where a command can start in a RUN step: at the start, or after `&&`, `||`, `;`, `|`, `(` or `$(`, past sudo/env and
// variable assignments. The words of a message that echo prints do not start a command.
const RUST_COMMAND = /(^|&&|\|\||[;|(]|\$\()\s*((sudo|env|exec|time|nice)\s+|[A-Za-z_][A-Za-z0-9_]*=\S*\s+)*(cargo|rustc|rustdoc|rustup)(\s|$)/;

/** The command text of a `[stage n/m] RUN [--flag ...] COMMAND` step name, or '' for any other step. */
function runCommand(name) {
  const m = /^\[[^\]]*\]\s+RUN\s+(.*)$/s.exec(name);
  return m ? m[1].replace(/^(--\S+\s+)+/, '') : '';
}

/** The distinct step names in the output of `--progress=rawjson`: the bytes may hold other lines (errors), which are skipped. */
export function stepNames(text) {
  const byDigest = new Map();
  for (const line of text.split(/\r?\n/)) {
    if (!line.startsWith('{')) continue;
    let o;
    try {
      o = JSON.parse(line);
    } catch {
      continue;
    }
    for (const v of o.vertexes ?? []) if (typeof v.name === 'string') byDigest.set(v.digest ?? v.name, v.name);
  }
  return [...byDigest.values()];
}

/**
 * @param {string} text the rawjson progress of a build of `target`
 * @param {string} target the stage that was built
 * @returns {{ inconclusive: boolean, problems: string[], own: number, others: string[] }}
 */
export function checkGraph(text, target) {
  const names = stepNames(text);
  const own = names.filter((n) => STEP.exec(n)?.[1] === target);
  const others = names.filter((n) => {
    const stage = STEP.exec(n)?.[1];
    return stage !== undefined && stage !== target;
  });
  const problems = [];
  if (own.length === 0) {
    return { inconclusive: true, problems: [`BuildKit listed no step of stage ${target}: the build stopped before it planned the graph`], own: 0, others };
  }
  for (const n of others) problems.push(`building ${target} also builds: ${n.length > 110 ? `${n.slice(0, 110)}...` : n}`);
  for (const n of names) if (RUST_COMMAND.test(runCommand(n))) problems.push(`the graph runs a Rust build tool: ${n.slice(0, 110)}`);
  return { inconclusive: false, problems: [...new Set(problems)], own: own.length, others };
}

function main(argv) {
  const [target, file] = argv;
  if (!target || !file) {
    console.error('usage: tools-graph.mjs TARGET PROGRESS-FILE');
    return 2;
  }
  const r = checkGraph(readFileSync(file, 'utf8'), target);
  if (r.inconclusive) {
    console.error(`tools-graph: ${r.problems[0]}`);
    return 2;
  }
  if (r.problems.length > 0) {
    for (const p of r.problems) console.error(`tools-graph: ${p}`);
    console.error(`tools-graph: building ${target} would build ${r.others.length} steps of other stages`);
    return 1;
  }
  console.log(`tools-graph: building ${target} runs ${r.own} steps, all of stage ${target}, and no step of any other stage`);
  return 0;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = main(process.argv.slice(2));
}
