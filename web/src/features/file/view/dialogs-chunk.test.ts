import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// P10: the first load of the file view (the Lighthouse page for a file) must not carry the code of the dialogs, their primitives or
// their forms. They are one chunk (FileDialogs.tsx) that FileScreen loads when the browser is idle or when the first one is asked
// for. If FileScreen imports one of them again, the chunk is joined to the screen's and the first load grows by about 90 kB, in
// twenty-five more requests, and the page's score falls by three points. The check is on the source, which is where it is made.
const screen = readFileSync(new URL("./FileScreen.tsx", import.meta.url), "utf8");
const imports = [...screen.matchAll(/^import\s+(type\s+)?[^;]*?from\s+"([^"]+)";/gms)].map((m) => ({ typeOnly: m[1] !== undefined, from: m[2] ?? "" }));

describe("the file screen's first load", () => {
  it("imports no dialog as code", () => {
    const code = imports.filter((i) => !i.typeOnly).map((i) => i.from);
    const dialogs = code.filter((from) => /Dialog/.test(from) || /\/dialogs\//.test(from));
    expect(dialogs).toEqual([]);
  });

  it("loads the dialogs' chunk with import(), lazily", () => {
    expect(screen).toMatch(/import\("\.\/FileDialogs"\)/);
    expect(screen).toMatch(/lazy\(/);
  });
});
