import { useQuery } from "@tanstack/react-query";
import { screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ServicePickup } from "./ServicePickup";
import { TreePickup } from "./TreePickup";
import { fileTreeQuery } from "@/features/file/queries";
import { swimlaneQuery } from "@/features/swimlanes/queries";
import { useCursorPages } from "@/lib/query/cursor-pages";
import { useNow } from "@/live/use-now";
import { fileEntry, lane, service } from "@/mocks/design/swimlanes/fixtures";
import { newWorld, worldApi, type World } from "@/mocks/design/swimlanes/world";
import { renderLive } from "@/test/live-app";

// The chips are live (P13: a change shows within a second, with no polling). The live layer is the real one (parser, client, plan,
// batcher) on a fake hub; the world is changed between two answers, the event is pushed, and the chip has to say the new thing.
// An event that does not name what a chip shows is proved by that query not being asked for again.

const HASH = "a".repeat(64);
const SERVICES = [{ namespace: "sitb", name: "tx-infinity-core" }];
const DETAIL = "/api/v1/swimlanes/sitb";
const TREE = "/api/v1/swimlanes/sitb/tree";

let world: World;
beforeEach(() => {
  world = newWorld("operator", [lane("sitb", { tenants: ["sit7"] })]);
  world.services["sitb"] = [service("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: Date.now() - 120_000, pending_files: ["application.properties"] })];
  world.tree["sitb"] = { "": [fileEntry("application.properties", { pickup_state: "needs_notify_or_restart" }), fileEntry("channels.yml", { pickup_state: "live" })] };
});
afterEach(() => {
  Reflect.deleteProperty(document, "cookie");
});

/** The Services tab, with only the cell under test: the swimlane's services from the cache the live events keep fresh. */
function ServicesProbe() {
  const lane = useQuery(swimlaneQuery("sitb")).data;
  const now = useNow(true, 30_000);
  return (
    <ul aria-label="Services">
      {(lane?.services ?? []).map((s) => (
        <li key={s.service.name} aria-label={s.service.name}>
          <ServicePickup service={s} now={now} />
        </li>
      ))}
    </ul>
  );
}

/** The root folder of the tree, a mark on every row. */
function TreeProbe() {
  const pages = useCursorPages(fileTreeQuery("sitb", "", null));
  return (
    <ul aria-label="Files">
      {pages.items.map((entry) => (
        <li key={entry.path} aria-label={entry.path}>
          <TreePickup node={{ id: entry.path, isDir: entry.is_dir, lazy: false, more: null, entry, children: [] }} rollup={undefined} />
        </li>
      ))}
    </ul>
  );
}

const service1 = () => screen.findByRole("listitem", { name: "tx-infinity-core" });
const seen = (check: () => unknown) => waitFor(check, { timeout: 4000 });

describe("the state of a service changes when the hub says it did", () => {
  it("pickup_changed: from 'Needs notify or restart' to 'Live' once the services picked the change up", async () => {
    const live = await renderLive(worldApi(world), <ServicesProbe />, DETAIL);
    expect(within(await service1()).getByText("Needs notify or restart")).toBeInTheDocument();
    world.services["sitb"] = [service("tx-infinity-core", { pickup: "live" })];
    await live.send({ type: "pickup_changed", swimlane: "sitb", services: SERVICES });
    await seen(async () => expect(within(await service1()).getByText("Live")).toBeInTheDocument());
    expect(within(await service1()).queryByText("Needs notify or restart")).toBeNull();
    expect(within(await service1()).queryByText(/^since/)).toBeNull();
  });

  it("pending_restart_changed (the superseded kind, still streamed): the same", async () => {
    const live = await renderLive(worldApi(world), <ServicesProbe />, DETAIL);
    expect(within(await service1()).getByText("Needs notify or restart")).toBeInTheDocument();
    world.services["sitb"] = [service("tx-infinity-core", { pickup: "needs_config_server_restart", pending_since: Date.now() - 1000, pending_files: ["channels.yml"] })];
    await live.send({ type: "pending_restart_changed", swimlane: "sitb", services: SERVICES });
    await seen(async () => expect(within(await service1()).getByText("Needs config-server restart")).toBeInTheDocument());
    expect(within(await service1()).getByText("channels.yml")).toBeInTheDocument();
  });

  it("file_observed: a file changed on NFS, and the service that reads it now waits", async () => {
    world.services["sitb"] = [service("tx-infinity-core")];
    const live = await renderLive(worldApi(world), <ServicesProbe />, DETAIL);
    expect(within(await service1()).getByText("Live")).toBeInTheDocument();
    world.services["sitb"] = [service("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: Date.now(), pending_files: ["application.properties"] })];
    await live.send({ type: "file_observed", swimlane: "sitb", path: "application.properties", hash: HASH, source: "scan" });
    await seen(async () => expect(within(await service1()).getByText("Needs notify or restart")).toBeInTheDocument());
  });

  it("pickup_changed: a resource file waits on the cache, and the time the hub gives is the time shown", async () => {
    world.services["sitb"] = [service("tx-infinity-core")];
    const live = await renderLive(worldApi(world), <ServicesProbe />, DETAIL);
    world.services["sitb"] = [service("tx-infinity-core", { pickup: "live_within_ttl", live_by: Date.UTC(2099, 0, 1, 14, 32), cache_ttl_secs: 1200 })];
    await live.send({ type: "pickup_changed", swimlane: "sitb", services: SERVICES });
    await seen(async () => expect(within(await service1()).getByText("Live by 2099-01-01 14:32 UTC")).toBeInTheDocument());
    expect(within(await service1()).getByText("(client cache)")).toBeInTheDocument();
  });

  it("resync: whatever the hub says now", async () => {
    const live = await renderLive(worldApi(world), <ServicesProbe />, DETAIL);
    world.services["sitb"] = [service("tx-infinity-core", { pickup: "live" })];
    await live.send({ type: "resync" });
    await seen(async () => expect(within(await service1()).getByText("Live")).toBeInTheDocument());
  });

  it("an event for another swimlane does not ask for this one again", async () => {
    const live = await renderLive(worldApi(world), <ServicesProbe />, DETAIL);
    const before = live.count(DETAIL);
    await live.send({ type: "pickup_changed", swimlane: "sitc", services: SERVICES });
    await new Promise((resolve) => setTimeout(resolve, 400));
    expect(live.count(DETAIL)).toBe(before);
  });
});

describe("the mark on a file in the tree changes when the hub says the file did", () => {
  const mark = (name: string) => screen.findByRole("listitem", { name });

  it("file_observed: the tree page that lists the file is read again and the mark follows", async () => {
    const live = await renderLive(worldApi(world), <TreeProbe />, TREE);
    expect(within(await mark("application.properties")).getByRole("img", { name: "Needs notify or restart" })).toBeInTheDocument();
    world.tree["sitb"] = { "": [fileEntry("application.properties", { pickup_state: "needs_config_server_restart" }), fileEntry("channels.yml", { pickup_state: "live" })] };
    await live.send({ type: "file_observed", swimlane: "sitb", path: "application.properties", hash: HASH, source: "tool_write" });
    await seen(async () => expect(within(await mark("application.properties")).getByRole("img", { name: "Needs config-server restart" })).toBeInTheDocument());
  });

  it("drift_changed: the same, for the paths it names", async () => {
    const live = await renderLive(worldApi(world), <TreeProbe />, TREE);
    expect(within(await mark("application.properties")).getByRole("img", { name: "Needs notify or restart" })).toBeInTheDocument();
    world.tree["sitb"] = { "": [fileEntry("application.properties", { pickup_state: "live" }), fileEntry("channels.yml", { pickup_state: "live" })] };
    await live.send({ type: "drift_changed", swimlane: "sitb", paths: ["application.properties"] });
    await seen(async () => expect(within(await mark("application.properties")).queryByRole("img")).toBeNull());
  });

  // `pickup_changed` carries service ids and no paths, so it names the tree pages and the file heads of the swimlane (src/live/invalidation.ts):
  // when services pick a change up (a Notify, a restart, the cache expiring) the marks on the tree and in the head of a file follow the services.
  it("pickup_changed: the marks follow the services (needs 'pickup_changed' to name the tree pages of the swimlane)", async () => {
    const live = await renderLive(worldApi(world), <TreeProbe />, TREE);
    expect(within(await mark("application.properties")).getByRole("img", { name: "Needs notify or restart" })).toBeInTheDocument();
    world.tree["sitb"] = { "": [fileEntry("application.properties", { pickup_state: "live" }), fileEntry("channels.yml", { pickup_state: "live" })] };
    await live.send({ type: "pickup_changed", swimlane: "sitb", services: SERVICES });
    await seen(async () => expect(within(await mark("application.properties")).queryByRole("img")).toBeNull());
  });
});
