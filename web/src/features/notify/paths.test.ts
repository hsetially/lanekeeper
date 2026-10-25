import { describe, expect, it } from "vitest";
import type { ServiceState } from "@/api/generated/model";
import { MAX_NOTIFY_PATHS, planForFile, planForServices } from "./paths";

// What a notification is asked to tell the config-server, worked out from what the agent reports and nothing else: the
// files that services are waiting on. A notification helps a service that waits for a notify or a restart; it does not help
// one that will go live by itself, or one that waits for the config-server to restart.

const service = (name: string, over: Partial<ServiceState> = {}): ServiceState => ({
  service: { namespace: "sitb", name },
  pickup: "needs_notify_or_restart",
  pending_since: null,
  live_by: null,
  notifications_enabled: true,
  cache_ttl_secs: 300,
  pending_files: [],
  ...over,
});

describe("planForServices", () => {
  it("collects the files that services wait on, once each, in a fixed order", () => {
    const plan = planForServices([
      service("tx-infinity-core", { pending_files: ["tx-infinity-api/b.yml", "application.properties"] }),
      service("tx-infinity-api", { pending_files: ["application.properties", "tx-infinity-api/a.yml"] }),
    ]);
    expect(plan.paths).toEqual(["application.properties", "tx-infinity-api/a.yml", "tx-infinity-api/b.yml"]);
    expect(plan.left).toBe(0);
  });

  it("lists the services that wait on one of those files, by name", () => {
    const plan = planForServices([service("zeta", { pending_files: ["a.yml"] }), service("alpha", { pending_files: ["a.yml"] }), service("idle")]);
    expect(plan.services.map((s) => s.service.name)).toEqual(["alpha", "zeta"]);
  });

  it("ignores a service that will go live by itself, one that is live, and one that waits for the config-server to restart", () => {
    const plan = planForServices([
      service("ttl", { pickup: "live_within_ttl", live_by: 1, pending_files: ["ttl.yml"] }),
      service("live", { pickup: "live", pending_files: ["live.yml"] }),
      service("config", { pickup: "needs_config_server_restart", pending_files: ["channels.yml"] }),
      service("waiting", { pending_files: ["waiting.yml"] }),
    ]);
    expect(plan.paths).toEqual(["waiting.yml"]);
    expect(plan.services.map((s) => s.service.name)).toEqual(["waiting"]);
  });

  it("has nothing to send when no service waits on a file", () => {
    expect(planForServices([])).toEqual({ paths: [], left: 0, services: [] });
    expect(planForServices([service("a")]).paths).toEqual([]);
  });

  it("sends at most 100 paths and says how many more files are waiting", () => {
    const files = Array.from({ length: 130 }, (_, i) => `app/f${String(i).padStart(3, "0")}.yml`);
    const plan = planForServices([service("many", { pending_files: files })]);
    expect(MAX_NOTIFY_PATHS).toBe(100);
    expect(plan.paths).toHaveLength(100);
    expect(plan.paths[0]).toBe("app/f000.yml");
    expect(plan.paths[99]).toBe("app/f099.yml");
    expect(plan.left).toBe(30);
  });

  it("does not list a service whose only files fell outside the 100", () => {
    const first = Array.from({ length: 100 }, (_, i) => `a/f${String(i).padStart(3, "0")}.yml`);
    const plan = planForServices([service("one", { pending_files: first }), service("two", { pending_files: ["z/late.yml"] })]);
    expect(plan.left).toBe(1);
    expect(plan.services.map((s) => s.service.name)).toEqual(["one"]);
  });

  it("does not change what it was given", () => {
    const given = [service("a", { pending_files: ["b.yml", "a.yml"] })];
    planForServices(given);
    expect(given[0]?.pending_files).toEqual(["b.yml", "a.yml"]);
  });
});

describe("planForFile", () => {
  it("sends the one file, and lists the services that wait on it", () => {
    const plan = planForFile("application.properties", [service("a", { pending_files: ["application.properties"] }), service("b", { pending_files: ["other.yml"] })]);
    expect(plan.paths).toEqual(["application.properties"]);
    expect(plan.services.map((s) => s.service.name)).toEqual(["a"]);
    expect(plan.left).toBe(0);
  });

  it("still sends the file when no service is listed as waiting on it", () => {
    const plan = planForFile("new.yml", [service("a")]);
    expect(plan.paths).toEqual(["new.yml"]);
    expect(plan.services).toEqual([]);
  });

  it("does not list a service that waits on the file for the config-server to restart", () => {
    const plan = planForFile("channels.yml", [service("config", { pickup: "needs_config_server_restart", pending_files: ["channels.yml"] })]);
    expect(plan.services).toEqual([]);
  });
});
