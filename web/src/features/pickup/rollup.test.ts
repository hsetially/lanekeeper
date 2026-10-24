import { describe, expect, it } from "vitest";
import type { PickupState } from "@/api/generated/model";
import { folderChip, pickupRollups, type PickupNodeLike } from "./rollup";

// A folder's pickup state, worked out from what the tree has loaded and nothing else. The hub lists one folder at a time, so a
// folder's files may be only partly in hand; the rollup says what it counted, never claims a folder is live, and never claims a
// state it does not have.

let n = 0;
const file = (name: string, state: PickupState | null, over: Partial<PickupNodeLike> = {}): PickupNodeLike => ({
  id: `f${++n}/${name}`,
  isDir: false,
  lazy: false,
  more: null,
  entry: { pickup_state: state },
  children: [],
  ...over,
});
const dir = (name: string, children: PickupNodeLike[], over: Partial<PickupNodeLike> = {}): PickupNodeLike => ({
  id: name,
  isDir: true,
  lazy: false,
  more: null,
  entry: { pickup_state: null },
  children,
  ...over,
});
const more = (folder: string): PickupNodeLike => ({ id: `\u0000more:${folder}`, isDir: false, lazy: false, more: folder, entry: null, children: [] });

describe("what a folder holds, from the files that are loaded", () => {
  it("counts each state once and names the most urgent", () => {
    const tree = dir("api", [file("a.yml", "needs_notify_or_restart"), file("b.yml", "needs_notify_or_restart"), file("c.yml", "live"), file("d.yml", "live_within_ttl")]);
    const rollup = pickupRollups([tree]).get("api");
    expect(rollup?.counts).toEqual([
      ["needs_notify_or_restart", 2],
      ["live_within_ttl", 1],
      ["live", 1],
    ]);
    expect(rollup?.worst).toBe("needs_notify_or_restart");
    expect(rollup?.files).toBe(4);
    expect(rollup?.unstated).toBe(0);
    expect(rollup?.complete).toBe(true);
  });

  it("looks through the folders inside it, and has one answer for every folder on the way", () => {
    const inner = dir("api/deep", [file("x.yml", "needs_config_server_restart")]);
    const outer = dir("api", [inner, file("y.yml", "live")]);
    const rollups = pickupRollups([dir("root", [outer])]);
    expect(rollups.get("api/deep")?.worst).toBe("needs_config_server_restart");
    expect(rollups.get("api")?.worst).toBe("needs_config_server_restart");
    expect(rollups.get("api")?.files).toBe(2);
    expect(rollups.get("root")?.counts).toEqual([
      ["needs_config_server_restart", 1],
      ["live", 1],
    ]);
  });

  it("counts a file the hub did not state as not stated, and not as live", () => {
    const rollup = pickupRollups([dir("api", [file("a.yml", null), file("b.yml", null)])]).get("api");
    expect(rollup?.worst).toBeNull();
    expect(rollup?.counts).toEqual([]);
    expect(rollup?.unstated).toBe(2);
    expect(rollup?.files).toBe(2);
  });

  it("takes the state of a row that stands for a base file and its tenant files, when the tree gives one", () => {
    const logical = file("core.yml", null, { pickup: "needs_notify_or_restart" });
    expect(pickupRollups([dir("api", [logical])]).get("api")?.worst).toBe("needs_notify_or_restart");
  });

  it("says a folder is not complete while a folder inside it is not open, or a page of it is not loaded", () => {
    const closed = dir("api/closed", [], { lazy: true });
    expect(pickupRollups([dir("api", [closed, file("a.yml", "live")])]).get("api")?.complete).toBe(false);
    expect(pickupRollups([dir("api", [file("a.yml", "live"), more("api")])]).get("api")?.complete).toBe(false);
    expect(pickupRollups([dir("api", [], { lazy: true })]).get("api")?.complete).toBe(false);
    expect(pickupRollups([dir("api", [file("a.yml", "live")])]).get("api")?.complete).toBe(true);
  });

  it("does not count a 'Load more' row as a file", () => {
    const rollup = pickupRollups([dir("api", [file("a.yml", "live"), more("api")])]).get("api");
    expect(rollup?.files).toBe(1);
  });

  it("lets the folder's own state count, when the hub states one (a folder first seen after the config-server started)", () => {
    const tree = dir("new", [file("a.yml", "live")], { entry: { pickup_state: "needs_config_server_restart" } });
    const rollup = pickupRollups([tree]).get("new");
    expect(rollup?.own).toBe("needs_config_server_restart");
    expect(rollup?.worst).toBe("needs_config_server_restart");
    expect(rollup?.counts).toEqual([["live", 1]]);
  });

  it("ignores a state that is not one of the four", () => {
    const rollup = pickupRollups([dir("api", [file("a.yml", "bogus" as PickupState), file("b.yml", "live")])]).get("api");
    expect(rollup?.counts).toEqual([["live", 1]]);
    expect(rollup?.unstated).toBe(1);
  });

  it("has nothing for a file: only folders have a rollup", () => {
    const f = file("a.yml", "live");
    expect(pickupRollups([f]).has(f.id)).toBe(false);
  });

  it("goes through a tree of thousands of rows in one pass", () => {
    const folders = Array.from({ length: 50 }, (_, i) => dir(`d${i}`, Array.from({ length: 200 }, (_, j) => file(`f${j}.yml`, j % 50 === 0 ? "needs_notify_or_restart" : "live"))));
    const rollups = pickupRollups(folders);
    expect(rollups.size).toBe(50);
    expect(rollups.get("d7")?.counts).toEqual([
      ["needs_notify_or_restart", 4],
      ["live", 196],
    ]);
  });
});

describe("the chip of a folder", () => {
  const chip = (children: PickupNodeLike[], over: Partial<PickupNodeLike> = {}) => {
    const root = dir("api", children, over);
    const rollup = pickupRollups([root]).get("api");
    if (rollup === undefined) throw new Error("no rollup");
    return folderChip(rollup);
  };

  it("is none when nothing waits: a folder is never called live", () => {
    expect(chip([file("a.yml", "live"), file("b.yml", "live")])).toBeNull();
  });

  it("is none when the hub stated nothing for the files: no state is not a state", () => {
    expect(chip([file("a.yml", null)])).toBeNull();
  });

  it("is the most urgent state, with the number of loaded files that are in it", () => {
    const result = chip([file("a.yml", "needs_notify_or_restart"), file("b.yml", "needs_notify_or_restart"), file("c.yml", "live")]);
    expect(result?.state).toBe("needs_notify_or_restart");
    expect(result?.count).toBe(2);
    expect(result?.words).toBe("2 of the 3 files loaded in this folder need a notify or a restart.");
  });

  it("says it is a count of what is loaded when something is not", () => {
    const result = chip([file("a.yml", "needs_config_server_restart"), more("api")]);
    expect(result?.words).toBe("1 of the 1 file loaded in this folder waits for the config-server to restart. Files that are not loaded yet are not counted.");
  });

  it("says so when some of the loaded files have no state from the hub", () => {
    const result = chip([file("a.yml", "needs_notify_or_restart"), file("b.yml", null)]);
    expect(result?.words).toBe("1 of the 2 files loaded in this folder needs a notify or a restart. The hub gave no pickup state for 1 of the files.");
  });

  it("covers the resource files that wait on the client cache", () => {
    const result = chip([file("a.yml", "live_within_ttl")]);
    expect(result?.state).toBe("live_within_ttl");
    expect(result?.words).toBe("1 of the 1 file loaded in this folder is read again by services when their client cache expires.");
  });

  it("speaks of the folder itself when that is what the hub states", () => {
    const result = chip([file("a.yml", "live")], { entry: { pickup_state: "needs_config_server_restart" } });
    expect(result?.state).toBe("needs_config_server_restart");
    expect(result?.count).toBeNull();
    expect(result?.words).toBe("The hub reports the folder itself as waiting for the config-server to restart.");
  });

  it("never says 'pending restart'", () => {
    for (const state of ["needs_notify_or_restart", "needs_config_server_restart", "live_within_ttl"] as const) {
      expect(chip([file("a.yml", state)])?.words).not.toMatch(/pending restart/i);
    }
  });
});
