// Run with: node --test scripts/
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { after, before, describe, test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { decideBash, decideEdit, parseDepends, parsePrompt, promptFiles } from './lk-lib.mjs';

const SRC = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const sh = (cwd, ...a) => execFileSync('git', ['-C', cwd, ...a], { encoding: 'utf8' }).trim();

describe('prompt parsing (real prompts)', () => {
  test('every build prompt has owned paths, a gate and tasks', () => {
    for (const { id } of promptFiles(SRC)) {
      const p = parsePrompt(SRC, id);
      if (typeof p.wave !== 'number') continue;
      assert.ok(p.owns.length > 0, `${id} owns`);
      assert.ok(p.gate?.startsWith('just verify-'), `${id} gate`);
      assert.ok(p.tasks.length > 0, `${id} tasks`);
    }
  });

  test('stack detection', () => {
    const stack = (id) => parsePrompt(SRC, id).stack;
    assert.equal(stack('01'), 'rust');
    assert.equal(stack('04'), 'rust');
    assert.equal(stack('08'), 'web');
    assert.equal(stack('09'), 'ops');
    assert.equal(stack('11'), 'ops');
  });

  test('read-only paths are not owned (prompt 08)', () => {
    const p = parsePrompt(SRC, '08');
    assert.deepEqual(p.readOnly, ['design/handoff/**']);
    assert.ok(!p.owns.includes('design/handoff/**'));
  });

  test('dependencies', () => {
    assert.deepEqual(parseDepends(SRC, parsePrompt(SRC, '04')).ids, ['01']);
    assert.deepEqual(parseDepends(SRC, parsePrompt(SRC, '05')).ids, ['01']);
    assert.deepEqual(parseDepends(SRC, parsePrompt(SRC, '01')).ids, []);
    assert.ok(parseDepends(SRC, parsePrompt(SRC, '08')).manual.length > 0);
    assert.ok(parseDepends(SRC, parsePrompt(SRC, '15')).ids.includes('14'));
  });
});

describe('guard decisions', () => {
  let repo;
  const plan = (status, extra = '', cc = 'none') =>
    `# Plan 04\n\nStatus: ${status}\nContract-change: ${cc}\nExtra-paths: ${extra}\n\n## Docs read\n`;
  const put = (rel, text = 'x') => {
    mkdirSync(dirname(join(repo, rel)), { recursive: true });
    writeFileSync(join(repo, rel), text);
  };

  before(() => {
    repo = mkdtempSync(join(tmpdir(), 'lk-guard-'));
    cpSync(join(SRC, 'prompts'), join(repo, 'prompts'), { recursive: true });
    sh(repo, 'init', '-q', '-b', 'main');
    sh(repo, 'config', 'user.email', 't@example.com');
    sh(repo, 'config', 'user.name', 't');
    put('README.md');
    sh(repo, 'add', '-A');
    sh(repo, 'commit', '-q', '-m', 'init');
    sh(repo, 'checkout', '-q', '-b', 'agent/04-fact-engine');
  });
  after(() => rmSync(repo, { recursive: true, force: true }));

  const allowed = (rel, content = '') => decideEdit(join(repo, rel), content);

  test('humans on other branches are never blocked', () => {
    sh(repo, 'checkout', '-q', 'main');
    assert.equal(allowed('crates/domain/src/lib.rs').allow, true);
    sh(repo, 'checkout', '-q', 'agent/04-fact-engine');
  });

  test('before approval only the plan file is editable', () => {
    put('plans/04-fact-engine.md', plan('DRAFT'));
    assert.equal(allowed('plans/04-fact-engine.md', 'Status: DRAFT').allow, true);
    const r = allowed('crates/engine/src/lib.rs');
    assert.equal(r.allow, false);
    assert.match(r.reason, /not approved/);
  });

  test('the agent cannot stamp its own approval', () => {
    assert.equal(allowed('plans/04-fact-engine.md', 'Status: APPROVED').allow, false);
    assert.equal(allowed('plans/04-fact-engine.md', 'Contract-change: APPROVED').allow, false);
  });

  test('after approval: owned, tests and shared files pass; others fail', () => {
    put('plans/04-fact-engine.md', plan('APPROVED'));
    assert.equal(allowed('crates/engine/src/lib.rs').allow, true);
    assert.equal(allowed('crates/engine/tests/golden.rs').allow, true);
    assert.equal(allowed('Cargo.toml').allow, true);
    assert.equal(allowed('Justfile').allow, true);
    assert.equal(allowed('plans/04-fact-engine.evidence.md').allow, true);
    assert.equal(allowed('crates/hub-registry/src/lib.rs').allow, false);
    assert.equal(allowed('web/src/App.tsx').allow, false);
  });

  test('the approved plan is frozen', () => {
    assert.equal(allowed('plans/04-fact-engine.md').allow, false);
  });

  test('contract and protected paths', () => {
    assert.match(allowed('crates/domain/src/lib.rs').reason, /contract path/);
    assert.match(allowed('proto/agent.proto').reason, /contract path/);
    assert.match(allowed('AGENTS.md').reason, /protected/);
    assert.match(allowed('.claude/settings.json').reason, /protected/);
    assert.match(allowed('prompts/04-fact-engine.md').reason, /protected/);
  });

  test('a human-approved contract change or extra path unlocks exactly that', () => {
    put('plans/04-fact-engine.md', plan('APPROVED', '`docs/decisions.md`', 'APPROVED'));
    assert.equal(allowed('docs/decisions.md').allow, true);
    assert.equal(allowed('crates/domain/src/lib.rs').allow, true);
    assert.equal(allowed('docs/security.md').allow, false);
  });

  test('bash: no gate skipping, no force push, no push to main', () => {
    assert.equal(decideBash('git commit --no-verify -m x', repo).allow, false);
    assert.equal(decideBash('git push --force origin agent/04-fact-engine', repo).allow, false);
    assert.equal(decideBash('git push origin main', repo).allow, false);
    assert.equal(decideBash('git push origin agent/04-fact-engine', repo).allow, true);
    assert.equal(decideBash('cargo test', repo).allow, true);
  });

  test('the hook script exits 2 on a block and 0 otherwise', () => {
    const hook = join(SRC, '.claude', 'hooks', 'guard.mjs');
    const run = (payload) => spawnSync('node', [hook], { input: JSON.stringify(payload), encoding: 'utf8' });
    const bad = run({ tool_name: 'Write', tool_input: { file_path: join(repo, 'web/x.ts'), content: 'x' }, cwd: repo });
    assert.equal(bad.status, 2);
    assert.match(bad.stderr, /Blocked by the Lanekeeper guard/);
    const ok = run({ tool_name: 'Write', tool_input: { file_path: join(repo, 'crates/engine/src/x.rs'), content: 'x' }, cwd: repo });
    assert.equal(ok.status, 0);
    assert.equal(run('not json').status, 0);
  });
});

describe('lk worktree + approve (end to end in a temp repo)', () => {
  let repo;
  const lk = (...a) => JSON.parse(execFileSync('node', [join(repo, 'scripts', 'lk.mjs'), ...a], { encoding: 'utf8', cwd: repo }));

  before(() => {
    repo = mkdtempSync(join(tmpdir(), 'lk-e2e-'));
    cpSync(join(SRC, 'prompts'), join(repo, 'prompts'), { recursive: true });
    cpSync(join(SRC, 'scripts'), join(repo, 'scripts'), { recursive: true });
    cpSync(join(SRC, 'plans', 'TEMPLATE.md'), join(repo, 'plans', 'TEMPLATE.md'));
    sh(repo, 'init', '-q', '-b', 'main');
    sh(repo, 'config', 'user.email', 't@example.com');
    sh(repo, 'config', 'user.name', 't');
    sh(repo, 'add', '-A');
    sh(repo, 'commit', '-q', '-m', '01: contracts');
  });
  after(() => rmSync(repo, { recursive: true, force: true }));

  test('dependency on 01 is met once "01:" is on the base branch', () => {
    assert.equal(lk('deps', '04').ok, true);
    assert.equal(lk('deps', '15').ok, false);
  });

  test('worktree, plan, approve', () => {
    const wt = lk('worktree', '04');
    assert.equal(wt.branch, 'agent/04-fact-engine');
    assert.equal(lk('worktree', '04').reused, true);
    const planPath = join(wt.worktree, 'plans', '04-fact-engine.md');
    cpSync(join(repo, 'plans', 'TEMPLATE.md'), planPath);
    sh(wt.worktree, 'add', '-A');
    sh(wt.worktree, 'commit', '-q', '-m', '04/plan: draft');
    assert.equal(lk('approve', '04').plan.approved, true);
    assert.match(sh(wt.worktree, 'log', '-1', '--format=%s'), /^04\/plan: approved/);
    assert.equal(decideEdit(join(wt.worktree, 'crates/engine/src/lib.rs')).allow, true);
    assert.equal(decideEdit(join(wt.worktree, 'crates/agent/src/main.rs')).allow, false);
  });

  test('a non-build prompt is refused', () => {
    const r = spawnSync('node', [join(repo, 'scripts', 'lk.mjs'), 'worktree', '12'], { encoding: 'utf8', cwd: repo });
    assert.notEqual(r.status, 0);
  });
});
