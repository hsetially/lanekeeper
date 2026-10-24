// The product source files of the app, for the tests that scan the source for a pattern the lint cannot see (what imports
// what, what is typed with what). Tests, mocks, test helpers, the generated client and the extracted handoff data are not
// product code and are left out.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

export const WEB = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (/\.(ts|tsx)$/.test(name)) out.push(full);
  }
  return out;
}

export type SourceFile = { path: string; text: string };

export function productSources(): SourceFile[] {
  return walk(join(WEB, "src"))
    .map((full) => ({ full, path: relative(WEB, full).replaceAll("\\", "/") }))
    .filter(({ path }) => !/\.test\.tsx?$/.test(path) && !/\.generated\.ts$/.test(path) && !/^src\/(api\/generated|mocks|test|handoff)\//.test(path))
    .map(({ full, path }) => ({ path, text: readFileSync(full, "utf8") }));
}

/** The text without comments, so that a comment that explains a ban is not mistaken for the thing it bans. */
export function withoutComments(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "").replace(/([^:])\/\/.*$/gm, "$1");
}
