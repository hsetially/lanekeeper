// A headless Chromium for Lighthouse: started on a free debugging port with a profile of its own, and ended, with everything it
// started, on every way out. Everything it waits for has a bound and an error that says what it was waited for.
//
// Why this is a module and not a function of the runner: the first run of the `perf` job on GitHub hung for 38 minutes with
// nothing in the log. Its Chromium had exited before it opened a port (see `launchArgs` in e2e/harness/browser.ts: on a runner
// that does not allow the browser's sandbox it does), the start-up reported that, and the clean-up then waited for the `exit`
// event of a process that had already exited, which is never sent again. Three rules came out of it:
//   - the exit of the process is recorded when it happens (`exitPromise`), so that asking for it later cannot wait for ever;
//   - every wait has a bound (`startTimeoutMs`, `stopTimeoutMs`) and an error that names it and prints the end of what the
//     browser said on stderr (`tailChars` of it, so the memory is bounded too);
//   - the browser has a process group of its own (`detached`), and the whole group is killed: the helper processes of a
//     SIGKILLed Chromium (zygote, GPU, renderers) otherwise outlive it and keep writing into the profile (remove-dir.mjs).
//     A Ctrl-C is not sent to a group of its own, so the runner's own SIGINT and SIGTERM end the groups too.
import { spawn as nodeSpawn } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { removeDir } from "./remove-dir.mjs";

const LISTENING = /DevTools listening on ws:\/\/[^:]+:(\d+)\//;
// The line is near the start of the output; this much of the end of it is enough to find it, and bounds the memory.
const SEEN_CHARS = 8192;
// After `exit`, the pipes may still hold what the browser printed last (the reason it died). This long is given to read it.
const DRAIN_MS = 250;

export const DEFAULTS = Object.freeze({ startTimeoutMs: 30_000, stopTimeoutMs: 10_000, tailChars: 2000 });

/** The browsers that are running, so that a signal to this process can end them. */
const live = new Set();
let hooked = false;

/** SIGKILL to the browser's process group (it is the leader of its own), or to the process alone when it has none. */
function killGroup(/** @type {{ pid?: number | undefined; kill: (signal: string) => unknown }} */ child) {
  if (typeof child.pid === "number") {
    try {
      process.kill(-child.pid, "SIGKILL");
      return;
    } catch {
      // No such group (it has ended, or this is not a group leader): the process itself is the next try.
    }
  }
  try {
    child.kill("SIGKILL");
  } catch {
    // Already gone.
  }
}

function killAll() {
  for (const child of live) killGroup(child);
  live.clear();
}

function hook() {
  if (hooked) return;
  hooked = true;
  process.on("exit", killAll);
  for (const signal of /** @type {const} */ (["SIGINT", "SIGTERM", "SIGHUP"])) {
    // Once: the default action is back when the clean-up is done, and the signal is sent again, so that the runner ends the way
    // the signal says (a parent that looks at the status sees what it sent), with the browsers gone.
    process.once(signal, () => {
      killAll();
      process.kill(process.pid, signal);
    });
  }
}

/** Ends every browser that this process started and has not stopped. For a deadline that gives up on the work in progress. */
export function killAllChromium() {
  killAll();
}

/** @param {{ code: number | null; signal: string | null }} ended */
const describeEnd = (ended) => (ended.code !== null ? `exited with code ${ended.code}` : `was ended by ${String(ended.signal)}`);

/**
 * @typedef {{
 *   port: number;
 *   profile: string;
 *   died: Promise<never>;
 *   tail: () => string;
 *   whileAlive: <T>(work: Promise<T>) => Promise<T>;
 *   stop: () => Promise<void>;
 * }} Chromium
 */

/**
 * @param {{
 *   binary: string;
 *   args: string[];
 *   profileRoot?: string;
 *   startTimeoutMs?: number;
 *   stopTimeoutMs?: number;
 *   tailChars?: number;
 *   report?: (message: string) => unknown;
 *   spawn?: typeof nodeSpawn;
 * }} options `args` are the flags the machine needs (e2e/harness/browser.ts `launchArgs`); the rest are fixed here. `report` is
 *   where a problem that does not fail the run goes (default: stderr). `spawn` is for tests.
 * @returns {Promise<Chromium>} resolves with the debugging port once Chromium has printed it; rejects, with the browser already
 *   ended and its profile removed, if it exits, does not answer in `startTimeoutMs` or cannot be started
 */
export async function startChromium({ binary, args, profileRoot = tmpdir(), startTimeoutMs = DEFAULTS.startTimeoutMs, stopTimeoutMs = DEFAULTS.stopTimeoutMs, tailChars = DEFAULTS.tailChars, report = (message) => process.stderr.write(message), spawn = nodeSpawn }) {
  const profile = mkdtempSync(join(profileRoot, "lk-lighthouse-"));
  const child = spawn(binary, ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check", "--disable-extensions", "--disable-background-networking", ...args, "about:blank"], {
    stdio: ["ignore", "ignore", "pipe"],
    detached: true,
  });
  live.add(child);
  hook();

  let tail = "";
  let seen = "";
  /** @type {{ code: number | null; signal: string | null } | undefined} */
  let ended;
  /** @type {Error | undefined} */
  let spawnError;
  let stopping = false;
  /** @type {number | undefined} */
  let found;
  /** @type {(port: number) => void} */
  let gotPort = () => undefined;
  const portPromise = new Promise((resolve) => {
    gotPort = resolve;
  });
  const exitPromise = new Promise((resolve) => {
    child.once("exit", (code, signal) => {
      ended = { code, signal };
      resolve(ended);
    });
    // An error event is a browser that could not be started, or a signal that could not be sent. It is never left unhandled.
    child.on("error", (error) => {
      spawnError ??= error;
      ended ??= { code: null, signal: null };
      resolve(ended);
    });
  });
  const closed = new Promise((resolve) => child.once("close", resolve));
  child.stderr.on("error", () => undefined);
  child.stderr.on("data", (chunk) => {
    const text = String(chunk);
    tail = (tail + text).slice(-tailChars);
    seen = (seen + text).slice(-SEEN_CHARS);
    const match = LISTENING.exec(seen);
    if (match) {
      found = Number(match[1]);
      gotPort(found);
    }
  });

  const printed = () => (tail === "" ? "\nChromium printed nothing on stderr." : `\nChromium stderr (the last ${tailChars} characters at most):\n${tail}`);
  const sandboxHint = () =>
    /sandbox/i.test(tail)
      ? "\nChromium's sandbox could not start on this machine (Ubuntu 24.04 restricts unprivileged user namespaces). e2e/harness/browser.ts leaves the sandbox off as root, on GitHub Actions and with LK_NO_SANDBOX=1: set LK_NO_SANDBOX=1 here."
      : "";
  /** What was printed last may still be in the pipe when `exit` comes; read it before the message is made. */
  const drained = () => Promise.race([closed, new Promise((resolve) => setTimeout(resolve, DRAIN_MS))]);

  /** @type {ReturnType<typeof setTimeout> | undefined} */
  let startTimer;
  /** @type {Promise<void> | undefined} */
  let stopped;
  const stop = () =>
    (stopped ??= (async () => {
      stopping = true;
      live.delete(child);
      killGroup(child);
      if (ended === undefined) {
        /** @type {ReturnType<typeof setTimeout> | undefined} */
        let timer;
        const gave = await Promise.race([exitPromise.then(() => "exited"), new Promise((resolve) => (timer = setTimeout(resolve, stopTimeoutMs, "timeout")))]);
        clearTimeout(timer);
        if (gave === "timeout") report(`lighthouse: Chromium (${binary}, pid ${String(child.pid)}) did not confirm that it had exited in ${stopTimeoutMs} ms; going on\n`);
      }
      // SIGKILL ends the browser and not its helper processes unless they are in its group (they are); the ones that still write
      // into the profile for a moment are waited for by removeDir. The profile is only a temporary folder, so one that cannot be
      // removed is reported and never costs the measurement.
      try {
        await removeDir(profile);
      } catch (/** @type {unknown} */ error) {
        report(`lighthouse: could not remove ${profile}: ${error instanceof Error ? error.message : String(error)}\n`);
      }
    })());

  /** @type {number} */
  let port;
  try {
    port = /** @type {number} */ (
      await Promise.race([
        portPromise,
        exitPromise.then(async () => {
          await drained();
          // `exit` can be announced before the last of the output is read: a browser that printed its port and then ended started,
          // and `died` says that it ended.
          if (found !== undefined) return found;
          if (spawnError !== undefined && ended?.code === null && ended.signal === null) throw new Error(`could not start ${binary}: ${spawnError.message}`);
          throw new Error(`Chromium (${binary}) ${describeEnd(/** @type {NonNullable<typeof ended>} */ (ended))} before it opened a debugging port: it never printed "DevTools listening on ws://..." on stderr, which is how this runner learns the port.${printed()}${sandboxHint()}`);
        }),
        new Promise((_, reject) => {
          startTimer = setTimeout(() => reject(new Error(`Chromium (${binary}) did not open a debugging port in ${startTimeoutMs} ms: it is still running and has not printed "DevTools listening on ws://..." on stderr (its remote debugging port).${printed()}${sandboxHint()}`)), startTimeoutMs);
        }),
      ])
    );
  } catch (error) {
    await stop();
    throw error;
  } finally {
    clearTimeout(startTimer);
  }

  // From here on, a browser that exits without being stopped is a failure of whatever is using it, and `whileAlive` says so.
  /** @type {Promise<never>} */
  const died = exitPromise.then(async () => {
    await drained();
    if (stopping) return new Promise(() => undefined);
    throw new Error(`Chromium (${binary}) ${describeEnd(/** @type {NonNullable<typeof ended>} */ (ended))} while it was in use.${printed()}${sandboxHint()}`);
  });
  died.catch(() => undefined);
  return {
    port,
    profile,
    died,
    tail: () => tail,
    whileAlive: (work) => Promise.race([work, died]),
    stop,
  };
}
