import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ServiceState } from "@/api/generated/model";
import { service } from "@/mocks/design/swimlanes/fixtures";
import { ServicePickup } from "./ServicePickup";

// The "Config status" cell of the Services tab: whether a service has picked up its config (D85), since when it has been waiting,
// what it is waiting on, and (in the tooltip) the cache lifetime and the notifications flag as the hub reported them.

const NOW = Date.UTC(2026, 9, 17, 14, 0, 0);
const MIN = 60_000;
const draw = (over: Partial<ServiceState>, now = NOW) => render(<ServicePickup service={service("tx-infinity-core", over)} now={now} />);

describe("the state of a service", () => {
  it("is Live", () => {
    draw({ pickup: "live" });
    expect(screen.getByText("Live")).toBeInTheDocument();
    expect(screen.queryByText(/since/)).toBeNull();
  });

  it("is 'Live by 14:32 UTC (client cache)' for a resource file that waits on the cache", () => {
    draw({ pickup: "live_within_ttl", live_by: Date.UTC(2026, 9, 17, 14, 32) });
    expect(screen.getByText("Live by 14:32 UTC")).toBeInTheDocument();
    expect(screen.getByText("(client cache)")).toBeInTheDocument();
  });

  it("does not make up a time: no live_by is 'Live within client cache'", () => {
    draw({ pickup: "live_within_ttl", live_by: null });
    expect(screen.getByText("Live within client cache")).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/\d\d:\d\d/);
  });

  it("says a time that has passed has passed", () => {
    draw({ pickup: "live_within_ttl", live_by: NOW - 5 * MIN });
    expect(screen.getByText("Live by 13:55 UTC")).toBeInTheDocument();
    expect(screen.getByText("(time passed)")).toBeInTheDocument();
  });

  it("is 'Needs notify or restart' for a property file, 'since' when it changed", () => {
    draw({ pickup: "needs_notify_or_restart", pending_since: NOW - 12 * MIN });
    expect(screen.getByText("Needs notify or restart")).toBeInTheDocument();
    expect(screen.getByText("since 12 min ago")).toBeInTheDocument();
  });

  it("is 'Needs config-server restart' for a new folder or channels.yml", () => {
    draw({ pickup: "needs_config_server_restart", pending_since: NOW - MIN, pending_files: ["channels.yml"] });
    expect(screen.getByText("Needs config-server restart")).toBeInTheDocument();
    expect(screen.getByText("channels.yml")).toBeInTheDocument();
  });

  it("never says 'pending restart'", () => {
    for (const pickup of ["live", "live_within_ttl", "needs_notify_or_restart", "needs_config_server_restart"] as const) {
      const { container, unmount } = draw({ pickup, pending_since: NOW - MIN, live_by: NOW + MIN, pending_files: ["a.yml"] });
      expect(container.textContent, pickup).not.toMatch(/pending restart/i);
      unmount();
    }
  });
});

describe("since when", () => {
  it("gives way with an ellipsis instead of running under the Restart button at 1024 wide (found by axe and the layout run)", () => {
    draw({ pickup: "needs_notify_or_restart", pending_since: NOW - 12 * MIN });
    const since = screen.getByText("since 12 min ago");
    expect(since.className).toMatch(/(^|\s)truncate(\s|$)/);
    expect(since.className).not.toMatch(/whitespace-nowrap/);
    expect(since).toHaveAttribute("title", "since 12 min ago");
    expect(since.parentElement?.className).toMatch(/(^|\s)min-w-0(\s|$)/);
  });

  it("is not said for a service that is live or will be by itself: nothing is waiting on a person", () => {
    draw({ pickup: "live_within_ttl", pending_since: NOW - 4 * MIN, live_by: NOW + MIN });
    expect(screen.queryByText(/^since/)).toBeNull();
  });

  it.each([Number.NaN, -1, 253_402_300_800_000])("is not made up when the hub's time is %s", (bad) => {
    draw({ pickup: "needs_notify_or_restart", pending_since: bad });
    expect(screen.queryByText(/since/)).toBeNull();
    expect(document.body.textContent).not.toMatch(/unknown time/);
  });

  it("is not said when the hub gave none", () => {
    draw({ pickup: "needs_notify_or_restart", pending_since: null });
    expect(screen.queryByText(/since/)).toBeNull();
  });
});

describe("what it waits on", () => {
  it("names the first file by its file name and counts the rest, with the files in the title", () => {
    draw({ pickup: "needs_notify_or_restart", pending_files: ["application.properties", "tx-infinity-api/a.yml", "tx-infinity-api/b.yml"] });
    const line = screen.getByText(/Waiting on/).closest("span[title]");
    expect(line).not.toBeNull();
    expect(within(line as HTMLElement).getByText("application.properties")).toBeInTheDocument();
    expect(line?.textContent).toContain("and 2 more");
    expect(line?.getAttribute("title")).toBe("application.properties\ntx-infinity-api/a.yml\ntx-infinity-api/b.yml");
  });

  it("says nothing when no file is named", () => {
    draw({ pickup: "needs_notify_or_restart", pending_files: [] });
    expect(screen.queryByText(/Waiting on/)).toBeNull();
  });

  it("keeps the title short: the first twenty files and how many more, never the 500 a service may wait on", () => {
    const files = Array.from({ length: 500 }, (_, i) => `d/f${i}.yml`);
    draw({ pickup: "needs_notify_or_restart", pending_files: files });
    const title = screen.getByText(/Waiting on/).closest("span[title]")?.getAttribute("title") ?? "";
    expect(title.split("\n")).toHaveLength(21);
    expect(title.split("\n")[20]).toBe("and 480 more");
    expect(screen.getByText(/and 499 more/)).toBeInTheDocument();
  });
});

describe("what the tooltip adds", () => {
  const sentences = (container: HTMLElement): string => container.querySelector(".sr-only")?.textContent ?? "";

  it("gives the cache lifetime for a service that waits on its cache, and the notifications flag as reported", () => {
    const { container } = draw({ pickup: "live_within_ttl", live_by: Date.UTC(2026, 9, 17, 14, 32), cache_ttl_secs: 300, notifications_enabled: true });
    expect(sentences(container)).toContain("Client cache lifetime: 5 min.");
    expect(sentences(container)).toContain("Notifications: on (as reported by the hub).");
  });

  it("says 'off' when the hub did", () => {
    const { container } = draw({ pickup: "needs_notify_or_restart", notifications_enabled: false });
    expect(sentences(container)).toContain("Notifications: off (as reported by the hub).");
    expect(sentences(container)).not.toContain("Client cache lifetime");
  });

  it("is on the hover too", async () => {
    const user = userEvent.setup();
    const { container } = draw({ pickup: "needs_notify_or_restart", notifications_enabled: false });
    await user.hover(container.querySelector("[data-slot='pickup-chip']") as HTMLElement);
    expect(await screen.findByRole("tooltip")).toHaveTextContent("Notifications: off (as reported by the hub)");
  });

  it("does not say what the flag does", () => {
    const { container } = draw({ pickup: "needs_notify_or_restart", notifications_enabled: true });
    expect(sentences(container)).not.toMatch(/subscrib|listen|receiv|will receive|enabled|disabled/i);
  });

  it("leaves out a cache lifetime that is not a whole number of seconds", () => {
    const { container } = draw({ pickup: "live_within_ttl", live_by: NOW + MIN, cache_ttl_secs: Number.NaN });
    expect(sentences(container)).not.toContain("Client cache lifetime");
  });
});
