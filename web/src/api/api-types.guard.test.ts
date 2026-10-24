import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

// Acceptance 3: no API type is written by hand. A type, interface, enum or class that has the name of a schema of the
// contract must live in src/api/generated/. (A name that merely resembles one, such as ApiProblem, is fine.)
const SRC = fileURLToPath(new URL("../", import.meta.url));
const doc = parse(readFileSync(new URL("../../../api/openapi.yaml", import.meta.url), "utf8")) as { components: { schemas: Record<string, unknown> } };
const schemaNames = Object.keys(doc.components.schemas);

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) walk(full, out);
    else if (/\.(ts|tsx)$/.test(name)) out.push(full);
  }
  return out;
}

describe("hand-written API types", () => {
  const files = walk(SRC).filter((f) => !relative(SRC, f).startsWith(join("api", "generated")) && !f.endsWith(".generated.ts"));

  it("finds the contract's schemas and the source files", () => {
    expect(schemaNames.length).toBeGreaterThan(200);
    expect(files.length).toBeGreaterThan(20);
  });

  it("has none outside src/api/generated/", () => {
    const names = new Set(schemaNames);
    const offenders: string[] = [];
    for (const file of files) {
      // An import or a re-export names a type; it does not declare one.
      const text = readFileSync(file, "utf8").replace(/\b(?:import|export)\s+(?:type\s+)?(?:[^;'"]*?\s+from\s+)?["'][^"']+["'];?/g, "");
      for (const match of text.matchAll(/\b(?:export\s+)?(?:declare\s+)?(?:type|interface|enum|class|const)\s+([A-Z][A-Za-z0-9_]*)\b/g)) {
        const name = match[1];
        if (name !== undefined && names.has(name)) offenders.push(`${relative(SRC, file)}: ${name}`);
      }
    }
    expect(offenders).toEqual([]);
  });
});
