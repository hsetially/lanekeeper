// Style Guide: the 45 contrast pairs, the 82 required token names and the ten status vocabularies.
import { loadPage } from "./dc-parse.mjs";
import { tsModule } from "./render.mjs";

const FILE = "Lanekeeper Style Guide.dc.html";
const BY = "web/scripts/handoff/extract-pairs.mjs";

export function buildPairs() {
  const page = loadPage(FILE);
  const pairs = page.evalConst("PAIRS").map(([fg, bg, min]) => {
    if (typeof fg !== "string" || typeof bg !== "string" || (min !== 3 && min !== 4.5)) {
      throw new Error(`unexpected contrast pair: ${JSON.stringify([fg, bg, min])}`);
    }
    return { fg, bg, min };
  });
  const styleGuideTokens = page.evalConst("TOKENS");
  const status = page.evalConst("STATUS").map((s) => ({
    key: s.key,
    label: s.label,
    icon: s.icon,
    desc: s.desc,
    border: s.bs,
    kind: s.kind,
  }));
  return { pairs, styleGuideTokens, status };
}

export function renderPairsTs(data) {
  return tsModule(BY, `design/handoff/screens/${FILE}`, [
    ["PAIRS", data.pairs],
    ["STYLE_GUIDE_TOKENS", data.styleGuideTokens],
    ["STATUS", data.status],
  ]);
}
