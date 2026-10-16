// Engineering Handoff: the screen inventory and routes table.
import { loadPage } from "./dc-parse.mjs";
import { tsModule } from "./render.mjs";

const FILE = "Lanekeeper Engineering Handoff.dc.html";
const BY = "web/scripts/handoff/extract-routes.mjs";

// The page styles a cell by its first character (see `cell` in the page script). Same rules here.
export function decodeCell(cell) {
  const raw = typeof cell === "string" ? { t: cell } : cell;
  let text = raw.t;
  let mono = Boolean(raw.m);
  if (text.startsWith("`")) {
    text = text.slice(1);
    mono = true;
  }
  if ("!~+^_".includes(text[0] ?? "") && text.length > 0) text = text.slice(1);
  return { text, mono };
}

export function sectionCall(page, id) {
  const call = page.calls("sec").find((c) => c.arguments[0]?.type === "Literal" && c.arguments[0].value === id);
  if (!call) throw new Error(`no sec('${id}', ...) call in ${FILE}`);
  return call;
}

// sec(id, nav, title, lead, head, cols, min, rows, note). `raw` keeps the style markers of each cell.
export function sectionRows(page, id) {
  const call = sectionCall(page, id);
  const raw = page.evalArg(call, 7).map((row) => row.map((c) => (typeof c === "string" ? c : c.t)));
  const rows = raw.map((row) => row.map((c) => decodeCell(c).text));
  const note = call.arguments[8] ? decodeCell(page.evalArg(call, 8)).text : "";
  const head = page.evalArg(call, 4);
  return { head, rows, raw, note };
}

const PATH_RE = /(?:^|[\s·])(\/[A-Za-z0-9_:\-./]*)/g;

export function buildRoutes() {
  const page = loadPage(FILE);
  const { head, rows, note } = sectionRows(page, "screens");
  if (head.join("|") !== "Screen|Route|Query|Role|Source file") throw new Error(`unexpected screens columns: ${head.join("|")}`);
  return {
    rows: rows.map(([screen, route, query, role, source]) => ({
      screen,
      route,
      query,
      role,
      source,
      paths: [...route.matchAll(PATH_RE)].map((m) => m[1]),
    })),
    note,
  };
}

export function renderRoutesTs(data) {
  return tsModule(BY, `design/handoff/screens/${FILE}`, [["ROUTES", data]]);
}
