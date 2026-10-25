import { execFileSync, spawn } from "node:child_process";
import { EventEmitter, once } from "node:events";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { startChromium } from "./chromium-process.mjs";

// The first run of the `perf` job on GitHub printed "3 passed" for the specs and then nothing for 38 minutes, until the job's
// timeout. Its Chromium for Lighthouse had no trace in the process list: it had EXITED before it opened its debugging port (a
// browser without `--no-sandbox` on a runner that does not allow the sandbox does), and the code that cleans up after such a
// browser then waited for an `exit` event that had already been sent. These tests use stand-ins for the browser (a shell script
// each: one that exits at once, one that never opens a port, one that does) to hold every wait of the start-up to a bound
// with a message, and the clean-up to ending the whole process group.
//
// A stand-in is a `#!/bin/sh` file. The ones that start a helper process write its pid to a file, so that a test can ask if the
// helper outlived the browser (a real Chromium has a zygote, a GPU process and renderers).

const SCRIPT_DIR = fileURLToPath(new URL("./", import.meta.url));
let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "lk-chromium-process-"));
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

/** @param {string} name @param {string} body @returns {string} the path of an executable stand-in */
function standIn(name, body) {
  const path = join(dir, name);
  writeFileSync(path, `#!/bin/sh\n${body}\n`);
  chmodSync(path, 0o755);
  return path;
}

const LISTENING = "DevTools listening on ws://127.0.0.1:43211/devtools/browser/0b1c2d3e-0000-4000-8000-000000000000";

/** True while the process exists and is not a zombie (a killed helper that nobody has reaped yet is gone). */
function alive(pid) {
  try {
    const state = execFileSync("ps", ["-o", "stat=", "-p", String(pid)], { encoding: "utf8" }).trim();
    return state !== "" && !state.startsWith("Z");
  } catch {
    return false;
  }
}

async function eventually(check, ms = 3000) {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    if (check()) return true;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  return check();
}

const quick = { startTimeoutMs: 5000, stopTimeoutMs: 2000, report: () => undefined };

describe("a Chromium that starts", () => {
  it("reports the port it printed, and was started headless with a debugging port, a profile of its own and the flags it was given", async () => {
    const argsFile = join(dir, "args.txt");
    const binary = standIn("ok", `printf '%s\\n' "$@" > '${argsFile}'\necho '${LISTENING}' >&2\nsleep 600 &\nwait`);
    const chromium = await startChromium({ binary, args: ["--disable-dev-shm-usage", "--no-sandbox"], ...quick });
    try {
      expect(chromium.port).toBe(43211);
      const argv = readFileSync(argsFile, "utf8").trim().split("\n");
      expect(argv).toEqual(expect.arrayContaining(["--headless=new", "--remote-debugging-port=0", "--disable-dev-shm-usage", "--no-sandbox"]));
      expect(argv.find((a) => a.startsWith("--user-data-dir="))).toMatch(/lk-lighthouse-/);
      expect(argv.at(-1)).toBe("about:blank");
    } finally {
      await chromium.stop();
    }
  });

  it("ends the whole process group on stop(): the helpers of the browser do not outlive it, and the profile is removed", async () => {
    const pidFile = join(dir, "helper.pid");
    const binary = standIn("ok", `echo '${LISTENING}' >&2\nsleep 600 &\necho $! > '${pidFile}'\nwait`);
    const chromium = await startChromium({ binary, args: [], ...quick });
    const profile = /** @type {string} */ (chromium.profile);
    expect(existsSync(profile)).toBe(true);
    await eventually(() => existsSync(pidFile));
    const helper = Number(readFileSync(pidFile, "utf8").trim());
    expect(alive(helper)).toBe(true);
    await chromium.stop();
    expect(await eventually(() => !alive(helper))).toBe(true);
    expect(existsSync(profile)).toBe(false);
  });

  it("stop() can be called twice, and after the browser has already exited, and returns each time (it used to wait for an `exit` that had already happened)", async () => {
    const binary = standIn("gone", `echo '${LISTENING}' >&2\nexit 0`);
    const chromium = await startChromium({ binary, args: [], ...quick });
    await expect(chromium.died).rejects.toThrow(/exited with code 0 while it was in use/);
    await Promise.race([chromium.stop(), new Promise((_, reject) => setTimeout(() => reject(new Error("stop() did not return in 2 s")), 2000))]);
    await chromium.stop();
  });

  it("tells whileAlive() to give up at once when the browser dies in the middle of the work, with what it printed", async () => {
    const binary = standIn("dies", `echo '${LISTENING}' >&2\nsleep 0.3\necho 'Check failed: renderer crashed' >&2\nexit 7`);
    const chromium = await startChromium({ binary, args: [], ...quick });
    try {
      const started = Date.now();
      await expect(chromium.whileAlive(new Promise(() => undefined))).rejects.toThrow(/exited with code 7 while it was in use[\s\S]*renderer crashed/);
      expect(Date.now() - started).toBeLessThan(2500);
    } finally {
      await chromium.stop();
    }
  });

  it("whileAlive() hands back what the work answered", async () => {
    const binary = standIn("ok", `echo '${LISTENING}' >&2\nsleep 600 &\nwait`);
    const chromium = await startChromium({ binary, args: [], ...quick });
    try {
      await expect(chromium.whileAlive(Promise.resolve("report"))).resolves.toBe("report");
    } finally {
      await chromium.stop();
    }
  });
});

describe("a Chromium that does not start (every wait has a bound and a message)", () => {
  it("fails at once, with the exit code and what it printed, when it exits before it opens a port (the failure of the first CI run)", async () => {
    const binary = standIn("sandbox", `echo '[1234:1234:FATAL:zygote_host_impl_linux.cc(132)] No usable sandbox! If you are running on Ubuntu 23.10+ or another Linux distribution that has disabled unprivileged user namespaces with AppArmor' >&2\nexit 133`);
    const started = Date.now();
    const error = await startChromium({ binary, args: [], ...quick }).then(
      () => undefined,
      (/** @type {unknown} */ e) => e,
    );
    expect(Date.now() - started).toBeLessThan(3000);
    expect(error?.message).toMatch(/exited with code 133 before it opened a debugging port/);
    expect(error?.message).toContain("No usable sandbox!");
    // A sandbox that cannot start has a cure, and the message names it.
    expect(error?.message).toMatch(/LK_NO_SANDBOX=1/);
  });

  it("reads what the browser printed last even when its exit is announced before the end of the pipe is read (the reason it died is the last thing it says)", async () => {
    const child = Object.assign(new EventEmitter(), { pid: undefined, exitCode: null, signalCode: null, stderr: new EventEmitter(), kill: () => true });
    setTimeout(() => {
      child.emit("exit", 133, null);
      setTimeout(() => {
        child.stderr.emit("data", Buffer.from("FATAL: the last words, read after the exit\n"));
        child.emit("close", 133, null);
      }, 40);
    }, 20);
    const error = await startChromium({ binary: "late", args: [], ...quick, spawn: () => child }).then(
      () => undefined,
      (/** @type {unknown} */ e) => e,
    );
    expect(error?.message).toMatch(/exited with code 133 before it opened a debugging port/);
    expect(error?.message).toContain("the last words, read after the exit");
  });

  it("counts a browser that printed its port as started even when its exit is announced first, and then says that it ended", async () => {
    const child = Object.assign(new EventEmitter(), { pid: undefined, exitCode: null, signalCode: null, stderr: new EventEmitter(), kill: () => true });
    setTimeout(() => {
      child.emit("exit", 0, null);
      setTimeout(() => {
        child.stderr.emit("data", Buffer.from(`${LISTENING}\n`));
        child.emit("close", 0, null);
      }, 40);
    }, 20);
    const chromium = await startChromium({ binary: "fast", args: [], ...quick, spawn: () => child });
    expect(chromium.port).toBe(43211);
    await expect(chromium.died).rejects.toThrow(/exited with code 0 while it was in use/);
    await chromium.stop();
  });

  it("does not say anything about the sandbox when the output does not", async () => {
    const binary = standIn("other", `echo 'error while loading shared libraries: libnss3.so' >&2\nexit 127`);
    const error = await startChromium({ binary, args: [], ...quick }).then(
      () => undefined,
      (/** @type {unknown} */ e) => e,
    );
    expect(error?.message).toContain("libnss3.so");
    expect(error?.message).not.toMatch(/LK_NO_SANDBOX/);
  });

  it("fails within startTimeoutMs, naming what it waited for and printing what the browser said, when it runs and never opens a port; its helpers are ended", async () => {
    const pidFile = join(dir, "helper.pid");
    const binary = standIn("silent", `echo 'booting, nothing to say' >&2\nsleep 600 &\necho $! > '${pidFile}'\nwait`);
    const started = Date.now();
    const error = await startChromium({ binary, args: [], ...quick, startTimeoutMs: 400 }).then(
      () => undefined,
      (/** @type {unknown} */ e) => e,
    );
    expect(Date.now() - started).toBeLessThan(3000);
    expect(error?.message).toMatch(/did not open a debugging port in 400 ms/);
    expect(error?.message).toContain("DevTools listening on ws://");
    expect(error?.message).toContain("booting, nothing to say");
    const helper = Number(readFileSync(pidFile, "utf8").trim());
    expect(await eventually(() => !alive(helper))).toBe(true);
  });

  it("fails with the system's reason when the browser cannot be started at all (no such file, not executable)", async () => {
    const error = await startChromium({ binary: join(dir, "does-not-exist"), args: [], ...quick }).then(
      () => undefined,
      (/** @type {unknown} */ e) => e,
    );
    expect(error?.message).toMatch(/could not start .*does-not-exist.*ENOENT/);
  });

  it("removes the profile of a browser that failed to start", async () => {
    const binary = standIn("fails", "exit 1");
    const root = join(dir, "profiles");
    mkdirSync(root);
    await startChromium({ binary, args: [], ...quick, profileRoot: root }).catch(() => undefined);
    expect(readdirSync(root)).toEqual([]);
  });

  it("keeps only the end of what the browser printed (everything is bounded)", async () => {
    const binary = standIn("loud", `i=0\nwhile [ $i -lt 3000 ]; do echo "line $i: ${"x".repeat(60)}" >&2; i=$((i+1)); done\necho 'the last word' >&2\nexit 2`);
    const error = await startChromium({ binary, args: [], ...quick, tailChars: 500 }).then(
      () => undefined,
      (/** @type {unknown} */ e) => e,
    );
    expect(error?.message).toContain("the last word");
    expect(error?.message).not.toContain("line 0:");
    expect(error?.message.length).toBeLessThan(1500);
  });
});

describe("stop() is bounded", () => {
  it("returns after stopTimeoutMs, and says so, when the process cannot be confirmed gone", async () => {
    // A browser that never reports its exit: a child of a process in an uninterruptible state does this.
    const child = Object.assign(new EventEmitter(), { pid: undefined, exitCode: null, signalCode: null, stderr: new EventEmitter(), kill: () => true });
    const told = [];
    const chromium = await new Promise((resolve, reject) => {
      startChromium({ binary: "stuck", args: [], startTimeoutMs: 1000, stopTimeoutMs: 200, report: (m) => told.push(m), spawn: () => child }).then(resolve, reject);
      setTimeout(() => child.stderr.emit("data", Buffer.from(`${LISTENING}\n`)), 20);
    });
    const started = Date.now();
    await chromium.stop();
    expect(Date.now() - started).toBeLessThan(2000);
    expect(told.join("")).toMatch(/did not confirm that it had exited in 200 ms/);
  });
});

describe("a Chromium that is left behind when the runner is stopped", () => {
  it("is ended with its helpers when the runner gets SIGINT or SIGTERM (it has a process group of its own, which a Ctrl-C does not reach)", async () => {
    for (const signal of ["SIGINT", "SIGTERM"]) {
      const pidFile = join(dir, `helper-${signal}.pid`);
      const binary = standIn(`ok-${signal}`, `echo '${LISTENING}' >&2\nsleep 600 &\necho $! > '${pidFile}'\nwait`);
      const runner = spawn(
        process.execPath,
        [
          "--input-type=module",
          "-e",
          `import { startChromium } from ${JSON.stringify(`${SCRIPT_DIR}chromium-process.mjs`)};
           await startChromium({ binary: ${JSON.stringify(binary)}, args: [], report: () => undefined });
           process.stdout.write("ready\\n");
           setInterval(() => undefined, 1000);`,
        ],
        { stdio: ["ignore", "pipe", "inherit"] },
      );
      const exited = once(runner, "exit");
      await once(runner.stdout, "data");
      await eventually(() => existsSync(pidFile));
      const helper = Number(readFileSync(pidFile, "utf8").trim());
      expect(alive(helper)).toBe(true);
      runner.kill(signal);
      const [, killedBy] = await exited;
      // The runner dies of the signal it was sent (the default action is restored after the clean-up), not of its own exit.
      expect(killedBy).toBe(signal);
      expect(await eventually(() => !alive(helper))).toBe(true);
    }
  }, 20_000);
});
