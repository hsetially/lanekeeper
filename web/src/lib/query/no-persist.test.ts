import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

// S2 and S10: nothing the API returns is written to storage. There is no query persister and no storage library, so a shared
// machine keeps nothing after the tab closes. (The only web storage is src/lib/prefs: layout choices of a typed whitelist.)
const WEB = fileURLToPath(new URL("../../../", import.meta.url));
const pkg = JSON.parse(readFileSync(join(WEB, "package.json"), "utf8")) as { dependencies?: Record<string, string>; devDependencies?: Record<string, string> };
const names = Object.keys({ ...pkg.dependencies, ...pkg.devDependencies });

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (/\.(ts|tsx)$/.test(name) && !/\.test\.tsx?$/.test(name)) out.push(full);
  }
  return out;
}

describe("no persistence of API data", () => {
  it("has no persister or storage library among the dependencies", () => {
    const banned = names.filter((n) => /persist|idb|localforage|dexie|pouchdb|store2|redux-persist|zustand-persist/i.test(n));
    expect(banned).toEqual([]);
  });

  it("never persists or hydrates a query client", () => {
    const files = walk(join(WEB, "src")).filter((f) => !relative(WEB, f).startsWith("src/api/generated"));
    const offenders = files.filter((f) => /persistQueryClient|createSyncStoragePersister|createAsyncStoragePersister|PersistQueryClientProvider|hydrate\(|dehydrate\(/.test(readFileSync(f, "utf8")));
    expect(offenders.map((f) => relative(WEB, f))).toEqual([]);
  });

  it("uses the browser's storage in exactly one module", () => {
    const files = walk(join(WEB, "src")).filter((f) => !relative(WEB, f).startsWith("src/api/generated"));
    const users = files.filter((f) => /\b(localStorage|sessionStorage|indexedDB)\b/.test(readFileSync(f, "utf8").replace(/^\s*(\/\/|\*).*$/gm, "")));
    expect(users.map((f) => relative(WEB, f))).toEqual(["src/lib/prefs/index.ts"]);
  });
});
