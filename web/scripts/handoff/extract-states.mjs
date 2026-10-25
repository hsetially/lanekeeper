// States Index: every state, the screens it applies to, and where the screen file already has it.
import { loadPage } from "./dc-parse.mjs";
import { tsModule } from "./render.mjs";

const FILE = "Lanekeeper States Index.dc.html";
const BY = "web/scripts/handoff/extract-states.mjs";

export function buildStates() {
  const page = loadPage(FILE);
  const sections = page.evalConst("sections");
  const out = { sections: [], rows: [] };
  for (const sec of sections) {
    out.sections.push({ code: sec.code, state: sec.anchor, title: sec.title, desc: sec.desc });
    for (const r of sec.rows) {
      if (!r.id.startsWith(sec.code)) throw new Error(`row ${r.id} is not under state ${sec.code}`);
      out.rows.push({
        id: r.id,
        code: sec.code,
        state: sec.anchor,
        screen: r.screen,
        file: r.file,
        variant: r.variant === "—" ? "" : r.variant,
        wired: r.wired === "Yes",
      });
    }
  }
  return out;
}

export function renderStatesTs(data) {
  return tsModule(BY, `design/handoff/screens/${FILE}`, [["STATES", data]]);
}
