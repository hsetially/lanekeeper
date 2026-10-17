// Every file of the built app is served, with the right type, and the hashed ones as immutable under the API policy.
import { readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "../fixtures/test";

const DIST = fileURLToPath(new URL("../../dist/", import.meta.url));
const TYPES: Record<string, RegExp> = { ".js": /^text\/javascript/, ".css": /^text\/css/, ".woff2": /^font\/woff2/, ".html": /^text\/html/, ".json": /^application\/json/, ".ttf": /^font\/ttf|^application\/x-font-ttf|^application\/octet-stream/ };

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

const files = walk(DIST)
  .map((f) => relative(DIST, f).split("\\").join("/"))
  .filter((f) => !f.endsWith(".map") && !f.startsWith(".vite/"));

test("the dist has files to serve", () => {
  expect(files.length).toBeGreaterThan(3);
  expect(files).toContain("index.html");
});

for (const file of files) {
  test(`serves ${file}`, async ({ request, harness }) => {
    const path = file === "index.html" ? "/" : `/${file}`;
    const response = await request.get(`${harness.url}${path}`);
    expect(response.status()).toBe(200);
    const ext = file.slice(file.lastIndexOf("."));
    expect(response.headers()["content-type"] ?? "").toMatch(TYPES[ext] ?? /./);
    expect(response.headers()["x-content-type-options"]).toBe("nosniff");
    if (file.startsWith("assets/")) {
      expect(response.headers()["cache-control"]).toContain("immutable");
      expect(response.headers()["content-security-policy"]).toBe("default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'");
    } else {
      expect(response.headers()["cache-control"]).toBe("no-store");
    }
  });
}
