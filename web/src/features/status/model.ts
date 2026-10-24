import type { FlagStatus } from "@/api/generated/model";

// What the Feature Status screen makes of the server's flags (`getFeatureStatus`). The server answers one entry for each
// documented flag resolved to a setting, with a channel when the flag is per channel; the screen shows one row for each FLAG,
// with a cell for each swimlane, and under it one row for each channel.
//
// "Differs from the template default" is the screen's own comparison of two texts as the server wrote them. The contract has no
// field for it, so it is a display comparison, not an engine fact: a value written another way (`True`, `yes`) is not recognised
// as the default. The page says so in its notes (design/DEVIATIONS.md).

export type Cell = { state: "absent" } | { state: "value"; text: string; differs: boolean; mixed: boolean };

export type FlagGroup = { name: string; entries: FlagStatus[]; channels: FlagStatus[] };

export type StatusRow = {
  /** Unique among the rows: a flag is its name, a channel row is the flag and the channel. */
  id: string;
  kind: "flag" | "channel";
  /** The flag's name, or the channel's. */
  name: string;
  /** The flag the row belongs to. */
  flag: string;
  /** How many channels the flag has (flag rows only). */
  channels: number;
  /** The setting path of the entry, when the row stands for one entry. */
  setting: string | null;
  file: string;
  def: string | null;
  cells: ReadonlyMap<string, Cell>;
  differs: boolean;
};

export type BuildOptions = { expanded: ReadonlySet<string>; filter: string; onlyDiffering: boolean };

export type Built = {
  rows: StatusRow[];
  /** Flags in the answer, and flags shown after the filter and the switch. */
  total: number;
  shown: number;
  /** Flags that differ in at least one swimlane. */
  differing: number;
  /** For each swimlane, how many flags differ in it. */
  differPerLane: ReadonlyMap<string, number>;
};

export function groupFlags(flags: readonly FlagStatus[]): FlagGroup[] {
  const byName = new Map<string, FlagGroup>();
  for (const entry of flags) {
    let group = byName.get(entry.flag);
    if (group === undefined) {
      group = { name: entry.flag, entries: [], channels: [] };
      byName.set(entry.flag, group);
    }
    group.entries.push(entry);
    if (entry.channel !== null) group.channels.push(entry);
  }
  return [...byName.values()];
}

type Lookup = Map<string, string | null>;

function lookupOf(entry: FlagStatus): Lookup {
  const map: Lookup = new Map();
  for (const v of entry.values) map.set(v.swimlane, v.value === null ? null : v.value.text);
  return map;
}

function cellFor(entries: readonly { lookup: Lookup; def: string | null }[], lane: string): Cell {
  const present: { text: string; def: string | null }[] = [];
  for (const { lookup, def } of entries) {
    const text = lookup.get(lane);
    if (text !== undefined && text !== null) present.push({ text, def });
  }
  const first = present[0];
  if (first === undefined) return { state: "absent" };
  const mixed = new Set(present.map((p) => p.text)).size > 1;
  const differs = present.some((p) => p.def !== null && p.text !== p.def);
  return { state: "value", text: mixed ? "mixed" : first.text, differs, mixed };
}

const defaultOf = (entry: FlagStatus): string | null => entry.template_default?.text ?? null;

function matches(group: FlagGroup, needle: string): boolean {
  if (needle === "") return true;
  const lower = needle.toLowerCase();
  return group.name.toLowerCase().includes(lower) || group.entries.some((e) => e.setting.toLowerCase().includes(lower));
}

export function buildRows(flags: readonly FlagStatus[], lanes: readonly string[], { expanded, filter, onlyDiffering }: BuildOptions): Built {
  const groups = groupFlags(flags);
  const needle = filter.trim();
  const rows: StatusRow[] = [];
  const differPerLane = new Map<string, number>(lanes.map((lane) => [lane, 0]));
  let differing = 0;
  let shown = 0;

  for (const group of groups) {
    const prepared = group.entries.map((entry) => ({ lookup: lookupOf(entry), def: defaultOf(entry), entry }));
    const cells = new Map<string, Cell>();
    let groupDiffers = false;
    for (const lane of lanes) {
      const cell = cellFor(prepared, lane);
      cells.set(lane, cell);
      if (cell.state === "value" && cell.differs) {
        groupDiffers = true;
        differPerLane.set(lane, (differPerLane.get(lane) ?? 0) + 1);
      }
    }
    if (groupDiffers) differing += 1;
    if (!matches(group, needle) || (onlyDiffering && !groupDiffers)) continue;
    shown += 1;

    const defaults = new Set(group.entries.map(defaultOf));
    const def = defaults.size === 1 ? ([...defaults][0] ?? null) : "varies";
    const first = group.entries[0];
    rows.push({
      id: group.name,
      kind: "flag",
      name: group.name,
      flag: group.name,
      channels: group.channels.length,
      setting: group.entries.length === 1 ? (first?.setting ?? null) : null,
      file: first?.file ?? "",
      def,
      cells,
      differs: groupDiffers,
    });

    if (expanded.has(group.name)) {
      for (const channel of group.channels) {
        const one = prepared.filter((p) => p.entry === channel);
        const channelCells = new Map<string, Cell>(lanes.map((lane) => [lane, cellFor(one, lane)]));
        rows.push({
          id: `${group.name}\u0000${channel.channel ?? ""}`,
          kind: "channel",
          name: channel.channel ?? "",
          flag: group.name,
          channels: 0,
          setting: channel.setting,
          file: channel.file,
          def: defaultOf(channel),
          cells: channelCells,
          differs: [...channelCells.values()].some((c) => c.state === "value" && c.differs),
        });
      }
    }
  }
  return { rows, total: groups.length, shown, differing, differPerLane };
}
