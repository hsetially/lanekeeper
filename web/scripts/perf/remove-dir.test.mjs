import { spawn } from "node:child_process";
import { once } from "node:events";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { removeDir } from "./remove-dir.mjs";

// scripts/perf/lighthouse.mjs starts a Chromium for every run and removes its profile folder when the run is over. The runner
// kills the browser with SIGKILL, which ends the main process and not the helper processes that it started (zygote, GPU,
// renderer, network service): they keep writing into the profile until they notice that the main process is gone, so a single
// `rmSync` can find the folder not empty again (ENOTEMPTY) and throw. The first Lighthouse run of the round-1 review died that
// way after the score was already measured, and nothing was written. These tests hold the removal to waiting for them.

let dir;
beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), "lk-remove-dir-"));
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

const failure = (code) => Object.assign(new Error(`${code}: scripted`), { code });

describe("removeDir with a scripted remove", () => {
  it("tries again after ENOTEMPTY, EBUSY and EPERM, and stops at the first removal that works", async () => {
    const seen = [];
    const codes = ["ENOTEMPTY", "EBUSY", "EPERM"];
    await removeDir(dir, {
      delayMs: 0,
      remove: (target) => {
        seen.push(target);
        const code = codes.shift();
        if (code !== undefined) throw failure(code);
      },
    });
    expect(seen).toEqual([dir, dir, dir, dir]);
  });

  it("does not try again after an error that waiting cannot cure", async () => {
    let calls = 0;
    await expect(
      removeDir(dir, {
        delayMs: 0,
        remove: () => {
          calls++;
          throw failure("EACCES");
        },
      }),
    ).rejects.toMatchObject({ code: "EACCES" });
    expect(calls).toBe(1);
  });

  it("gives up after a bounded number of tries and throws the last error", async () => {
    let calls = 0;
    await expect(
      removeDir(dir, {
        attempts: 4,
        delayMs: 0,
        remove: () => {
          calls++;
          throw failure("ENOTEMPTY");
        },
      }),
    ).rejects.toMatchObject({ code: "ENOTEMPTY" });
    expect(calls).toBe(4);
  });

  it("waits between the tries (the helper processes need time to stop)", async () => {
    const stamps = [];
    await removeDir(dir, {
      attempts: 3,
      delayMs: 40,
      remove: () => {
        stamps.push(Date.now());
        if (stamps.length < 3) throw failure("ENOTEMPTY");
      },
    });
    expect(stamps[1] - stamps[0]).toBeGreaterThanOrEqual(35);
    expect(stamps[2] - stamps[1]).toBeGreaterThanOrEqual(35);
  });
});

describe("removeDir on the file system", () => {
  it("removes a tree, and a folder that is already gone is not an error", async () => {
    mkdirSync(join(dir, "a", "b"), { recursive: true });
    writeFileSync(join(dir, "a", "b", "file"), "x");
    await removeDir(dir);
    expect(existsSync(dir)).toBe(false);
    await expect(removeDir(dir)).resolves.toBeUndefined();
  });

  // The failure itself, not a model of it: a process that keeps creating files in the folder while it is removed, as Chromium's
  // helpers do after the main process is killed, and is then killed itself, as the runner kills the browser. A single rmSync fails
  // on this every time (20 of 20 when it was measured); removeDir waits until the writer has stopped.
  it("removes a folder that another process is still writing into, once that process has been killed", async () => {
    for (let i = 0; i < 300; i++) mkdirSync(join(dir, `d${i}`));
    const writer = spawn(
      process.execPath,
      [
        "-e",
        `const fs = require("node:fs");
         let i = 0;
         fs.writeFileSync(process.argv[1] + "/f0", "x");
         process.stdout.write("ready\\n");
         for (;;) fs.writeFileSync(process.argv[1] + "/f" + (i++ % 5000), "x");`,
        dir,
      ],
      { stdio: ["ignore", "pipe", "ignore"] },
    );
    const exited = once(writer, "exit");
    await once(writer.stdout, "data");
    // The same call as before the fix fails here, which is the proof that the writer does what the test says it does.
    expect(() => rmSync(dir, { recursive: true, force: true })).toThrow(/ENOTEMPTY/);
    const killer = setTimeout(() => writer.kill("SIGKILL"), 300);
    try {
      await removeDir(dir);
    } finally {
      clearTimeout(killer);
      writer.kill("SIGKILL");
    }
    await exited;
    expect(existsSync(dir)).toBe(false);
  }, 20_000);
});

describe("lighthouse.mjs", () => {
  const source = readFileSync(new URL("./lighthouse.mjs", import.meta.url), "utf8");

  it("removes the Chromium profile with removeDir and never with a bare rmSync", () => {
    expect(source).toMatch(/import \{ removeDir \} from "\.\/remove-dir\.mjs"/);
    expect(source).toMatch(/removeDir\(profile\)/);
    expect(source).not.toMatch(/\brmSync\(/);
  });

  it("does not let a profile that cannot be removed throw away a score that was measured", () => {
    // The removal is the last thing a run does; its failure is reported on stderr and the run goes on.
    expect(source).toMatch(/catch \([^)]*\) \{[^}]*process\.stderr\.write\(`lighthouse: could not remove/);
  });
});
