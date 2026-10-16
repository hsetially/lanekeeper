// Regenerates everything derived from the design handoff, or checks that it is up to date.
//   node scripts/handoff/generate.mjs          write the files
//   node scripts/handoff/generate.mjs --check  exit 1 when a file is missing or stale
// Reads design/handoff/screens (read-only). The generated files are committed, because the image
// build only has web/ (F1): `pnpm build` never needs the design directory.
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { WEB_ROOT } from "./dc-parse.mjs";
import { buildBoards, renderBoardsTs } from "./extract-boards.mjs";
import { buildIcons, renderIconsTs } from "./extract-icons.mjs";
import { buildKeymap, renderKeymapTs } from "./extract-keymap.mjs";
import { buildPairs, renderPairsTs } from "./extract-pairs.mjs";
import { buildRoutes, renderRoutesTs } from "./extract-routes.mjs";
import { buildStates, renderStatesTs } from "./extract-states.mjs";
import { buildTokens, renderThemeCss, renderTokensCss, renderTokensTs } from "./extract-tokens.mjs";

export { WEB_ROOT };

// path (relative to web/) -> content
export function generateAll() {
  const tokens = buildTokens();
  return new Map([
    ["src/handoff/tokens.generated.ts", renderTokensTs(tokens)],
    ["src/handoff/pairs.generated.ts", renderPairsTs(buildPairs())],
    ["src/handoff/routes.generated.ts", renderRoutesTs(buildRoutes())],
    ["src/handoff/keymap.generated.ts", renderKeymapTs(buildKeymap())],
    ["src/handoff/states.generated.ts", renderStatesTs(buildStates())],
    ["src/handoff/boards.generated.ts", renderBoardsTs(buildBoards())],
    ["src/icons/icons.generated.ts", renderIconsTs(buildIcons())],
    ["src/styles/tokens.css", renderTokensCss(tokens)],
    ["src/styles/theme.generated.css", renderThemeCss(tokens)],
  ]);
}

function main(argv) {
  const check = argv.includes("--check");
  const stale = [];
  for (const [path, content] of generateAll()) {
    const file = join(WEB_ROOT, path);
    const current = existsSync(file) ? readFileSync(file, "utf8") : null;
    if (current === content) continue;
    if (check) {
      stale.push(path);
    } else {
      mkdirSync(dirname(file), { recursive: true });
      writeFileSync(file, content);
      process.stdout.write(`wrote ${path}\n`);
    }
  }
  if (stale.length > 0) {
    process.stderr.write(`stale or missing generated files (run pnpm gen:handoff):\n  ${stale.join("\n  ")}\n`);
    process.exit(1);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main(process.argv.slice(2));
