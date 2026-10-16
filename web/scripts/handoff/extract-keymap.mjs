// Engineering Handoff: the keyboard map per screen, expanded into one executable binding per key.
import { loadPage } from "./dc-parse.mjs";
import { decodeCell, sectionRows } from "./extract-routes.mjs";
import { tsModule } from "./render.mjs";

const FILE = "Lanekeeper Engineering Handoff.dc.html";
const BY = "web/scripts/handoff/extract-keymap.mjs";

const MODIFIERS = { "⌘": "ControlOrMeta", "⇧": "Shift", "⌥": "Alt" };
const NAMED = {
  "↑": "ArrowUp",
  "↓": "ArrowDown",
  "←": "ArrowLeft",
  "→": "ArrowRight",
  "↵": "Enter",
  Esc: "Escape",
  Space: "Space",
  Backspace: "Backspace",
  Tab: "Tab",
  Home: "Home",
  End: "End",
  Enter: "Enter",
};

// One key with optional modifier glyphs in front: "⌘K", "⇧Tab", "↵", "n".
function parseAtom(text) {
  let rest = text.trim();
  const mods = [];
  while (rest.length > 0 && rest[0] in MODIFIERS) {
    mods.push(MODIFIERS[rest[0]]);
    rest = rest.slice(1);
  }
  let key;
  if (rest in NAMED) key = NAMED[rest];
  else if ([...rest].length === 1) key = /[A-Za-z]/.test(rest) ? rest.toLowerCase() : rest;
  else throw new Error(`unknown key "${text}"`);
  return [...mods, key].join("+");
}

// "↵ on fold row" -> key + the context it applies in.
function withContext(part) {
  const m = /^(.+?) on (.+)$/.exec(part);
  return m ? { atom: m[1], when: m[2] } : { atom: part };
}

const ARROW_OR_ENTER = /^[↑↓←→↵]$/;

export function parseKeys(raw) {
  const text = raw.trim();

  const sequence = /^([A-Za-z]) then (.+)$/.exec(text);
  if (sequence) {
    return {
      bindings: sequence[2].split(" / ").map((second) => ({ steps: [sequence[1], parseAtom(second)] })),
    };
  }

  const range = /^(⌘)(\d) … ⌘(\d)$/.exec(text);
  if (range) {
    const bindings = [];
    for (let n = Number(range[2]); n <= Number(range[3]); n += 1) bindings.push({ steps: [`${MODIFIERS[range[1]]}+${n}`] });
    return { bindings };
  }

  const bindings = [];
  for (const part of text.split(/ \/ | or |, /)) {
    const tokens = part.trim().split(/\s+/);
    if (tokens.length > 1 && tokens.every((t) => ARROW_OR_ENTER.test(t))) {
      for (const t of tokens) bindings.push({ steps: [parseAtom(t)] });
      continue;
    }
    const { atom, when } = withContext(part.trim());
    bindings.push(when ? { steps: [parseAtom(atom)], when } : { steps: [parseAtom(atom)] });
  }
  return { bindings };
}

// Rows whose key cell is prose, not a key. Anything else without a key is a handoff change to look at.
const NO_KEY_ROWS = {
  "type-ahead": { kind: "typeahead" },
  "Match case, Regex": { skip: "toggle buttons; no shortcut assigned yet (the handoff suggests ⌥C and ⌥R, undecided)" },
};

const slug = (s) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");

function splitAction(action, count) {
  if (count > 1) {
    const parts = action.split(" / ");
    if (parts.length === count) return parts;
  }
  return Array.from({ length: count }, () => action);
}

export function buildKeymap(source) {
  const { raw, note } = source ?? sectionRows(loadPage(FILE), "keys");
  const rows = [];
  const ids = new Set();
  let scope = "";
  for (const [scopeCell, keysCell, action] of raw) {
    const scopeText = scopeCell.trim();
    if (scopeText) scope = scopeText;
    const keysRaw = keysCell;
    const keysDecoded = decodeCell(keysRaw);
    const noKey = keysRaw.trim().startsWith("_");
    const scopeName = decodeCell(scope).text;
    const keys = noKey ? keysRaw.trim().slice(1) : keysDecoded.text;
    const row = { id: "", scope: scopeName, keys, action: decodeCell(action).text, bindings: [] };

    if (noKey) {
      const known = NO_KEY_ROWS[keys];
      if (!known) throw new Error(`row "${scopeName}: ${keys}" names no key and is not in the allow-list`);
      if (known.skip) row.skip = known.skip;
      else row.bindings = [{ kind: known.kind, action: row.action }];
    } else {
      const parsed = parseKeys(keys);
      const actions = splitAction(row.action, parsed.bindings.length);
      row.bindings = parsed.bindings.map((b, i) => ({ ...b, action: actions[i] }));
    }

    let id = `${slug(scopeName)}.${slug(row.action).split("-").slice(0, 5).join("-")}`;
    for (let n = 2; ids.has(id); n += 1) id = `${id.replace(/-\d+$/, "")}-${n}`;
    ids.add(id);
    rows.push({ ...row, id });
  }
  return { rows, note };
}

export function renderKeymapTs(data) {
  return tsModule(BY, `design/handoff/screens/${FILE}`, [["KEYMAP", data]]);
}
