// The boards: every <dc-import> of the twelve board files, with its label, group and props.
// A regex scan, not an HTML parser, and nothing is executed. The boards share one template
// (dv-turn, dv-grp, dv-opt, dv-olabel); a board that stops following it fails the tests.
import { readdirSync } from "node:fs";
import { HANDOFF_DIR, decodeEntities, extractScript, readHandoffFile } from "./dc-parse.mjs";
import { tsModule } from "./render.mjs";

const BY = "web/scripts/handoff/extract-boards.mjs";

const TOKEN_RE =
  /<section class="dv-turn" id="([^"]+)"[^>]*>|<span class="dv-tname">([^<]*)<\/span>|<div class="dv-grp">([^<]*)<\/div>|<div class="dv-opt" id="([^"]+)">\s*<div class="dv-olabel">([\s\S]*?)<\/div>|<dc-import\b([^>]*)>/g;

const stripTags = (s) => s.replace(/<[^>]*>/g, "");

function typed(value) {
  const m = /^\{\{\s*(.*?)\s*\}\}$/.exec(value);
  if (!m) return { value, dynamic: false };
  const inner = m[1];
  if (inner === "true") return { value: true, dynamic: false };
  if (inner === "false") return { value: false, dynamic: false };
  if (/^-?\d+(\.\d+)?$/.test(inner)) return { value: Number(inner), dynamic: false };
  return { value, dynamic: true };
}

// "1440px" -> 1440; "100%" stays a string (the page fills its container).
function dimension(text) {
  const px = /^(\d+)px$/.exec(text ?? "");
  return px ? Number(px[1]) : text;
}

export function parseImportAttrs(attrText) {
  const raw = {};
  for (const m of attrText.matchAll(/([\w:-]+)="([^"]*)"/g)) raw[m[1]] = decodeEntities(m[2]);
  const { name, theme, "hint-size": hint, ...rest } = raw;
  delete rest.style; // layout of the board's own frame, not a prop of the page
  const props = {};
  let dynamic = false;
  for (const [k, v] of Object.entries(rest)) {
    const t = typed(v);
    props[k] = t.value;
    dynamic ||= t.dynamic;
  }
  const [w, h] = (hint ?? "").split(",").map(dimension);
  return { name, theme, size: { w, h }, props, dynamic };
}

export function parseBoard(board, html) {
  const frames = [];
  const embedded = [];
  const optionsWithoutImport = [];
  let turn = "";
  let turnName = "";
  let group = "";
  let option = null;
  for (const m of html.matchAll(TOKEN_RE)) {
    const [, turnId, tname, grp, optId, optLabel, importAttrs] = m;
    if (turnId) {
      turn = turnId;
      turnName = "";
      group = "";
      option = null;
    } else if (tname !== undefined) turnName = decodeEntities(tname.trim());
    else if (grp !== undefined) group = decodeEntities(grp.trim());
    else if (optId) {
      if (option && !option.used) optionsWithoutImport.push(option.id);
      const label = decodeEntities(stripTags(optLabel)).trim();
      option = { id: optId, label: label.startsWith(optId) ? label.slice(optId.length).trim() : label, used: false };
    } else if (importAttrs !== undefined) {
      const a = parseImportAttrs(importAttrs);
      if (option && !option.used) {
        option.used = true;
        frames.push({
          board, turn, turnName, group, id: option.id, label: option.label, labelled: true,
          page: a.name, theme: a.theme ?? "", size: a.size, props: a.props, dynamic: a.dynamic,
        });
      } else if (option) {
        throw new Error(`${board}: option ${option.id} holds more than one dc-import`);
      } else {
        embedded.push({ board, page: a.name, theme: a.theme ?? "", size: a.size, props: a.props, dynamic: a.dynamic });
      }
    }
  }
  if (option && !option.used) optionsWithoutImport.push(option.id);
  return { frames, embedded, optionsWithoutImport };
}

export function buildBoards() {
  const frames = [];
  const embedded = [];
  const withoutImport = {};
  const pages = new Set();
  const boardFiles = readdirSync(HANDOFF_DIR)
    .filter((n) => n.endsWith(".dc.html"))
    .sort();
  for (const file of boardFiles) {
    const html = readHandoffFile(file);
    if (!html.includes("<dc-import")) continue;
    const parsed = parseBoard(file, html);
    frames.push(...parsed.frames);
    embedded.push(...parsed.embedded);
    if (parsed.optionsWithoutImport.length) withoutImport[file] = parsed.optionsWithoutImport;
    for (const f of [...parsed.frames, ...parsed.embedded]) pages.add(f.page);
  }
  const pageProps = {};
  for (const page of [...pages].sort()) {
    const { props } = extractScript(readHandoffFile(`${page}.dc.html`));
    const { $preview: preview, ...declared } = props;
    pageProps[page] = { props: declared, preview: preview ?? null };
  }
  return { frames, embedded, optionsWithoutImport: withoutImport, pageProps };
}

export function renderBoardsTs(data) {
  return tsModule(BY, "design/handoff/screens/*.dc.html (the board files)", [
    ["FRAMES", data.frames],
    ["EMBEDDED", data.embedded],
    ["OPTIONS_WITHOUT_IMPORT", data.optionsWithoutImport],
    ["PAGE_PROPS", data.pageProps],
  ]);
}
