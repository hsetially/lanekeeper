import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Me, Role } from "@/api/generated/model";
import { SessionContext, type SessionState } from "@/auth/session";
import { LiveProvider, type LiveStatusView } from "@/live/LiveProvider";
import { OFFLINE_AFTER_FAILURES } from "@/live/sse-client";
import { FakeHub } from "@/test/fake-hub";
import { FIRST_FRAME } from "@/test/sse-golden";
import { expectNoAxeViolations } from "@/test/axe";
import { LiveIndicator, LiveIndicatorView } from "./LiveIndicator";
import { PausedBanner, PausedBannerView } from "./PausedBanner";

// The live engine is a chunk of its own, imported when a signed-in user with a role is on screen. Warm in the module cache, so
// that the import resolves in the next turn of the event loop that the fake timers yield.
beforeAll(async () => {
  await import("@/live/live");
});

// The header pill and the banner of the live stream, after the Shell's frames 1h, 1i and 1j and the States index (the U rows):
// "Live", "Reconnecting... retry in 3s", "Offline, data from 10:42", and the banner "Live updates paused, retrying". A status is
// never colour alone: each has its own shape or icon and its own words.

const T = Date.UTC(2026, 9, 17, 10, 42, 0);
/** What a sighted person reads in the pill: the words that are hidden from assistive technology (the sentence it hears is `.sr-only`). */
const seen = (): string => screen.getByRole("status").querySelector("[aria-hidden='true']:not([data-shape])")?.textContent ?? "";
const view = (patch: Partial<LiveStatusView>): LiveStatusView => ({ status: "live", failures: 0, retryAt: null, dataAsOf: null, stopReason: null, ...patch });

describe("LiveIndicatorView", () => {
  it("shows nothing for a stream nobody started (a public screen)", () => {
    const { container } = render(<LiveIndicatorView view={view({ status: "idle" })} now={T} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("says Live, with a filled dot", () => {
    render(<LiveIndicatorView view={view({ status: "live" })} now={T} />);
    const pill = screen.getByRole("status");
    expect(pill).toHaveTextContent("Live");
    expect(pill.querySelector("[data-shape]")).toHaveAttribute("data-shape", "filled");
  });

  it("says Connecting before the first frame", () => {
    render(<LiveIndicatorView view={view({ status: "connecting" })} now={T} />);
    expect(screen.getByRole("status")).toHaveTextContent("Connecting");
  });

  it("counts down to the next attempt while reconnecting, with a pulsing ring (and no number once the time is up)", () => {
    const { rerender } = render(<LiveIndicatorView view={view({ status: "reconnecting", failures: 1, retryAt: T + 2500 })} now={T} />);
    const pill = screen.getByRole("status");
    expect(pill).toHaveTextContent("Reconnecting… retry in 3s");
    const dot = pill.querySelector("[data-shape]");
    expect(dot).toHaveAttribute("data-shape", "ring");
    expect(dot?.className).toContain("animate-lk-pulse");
    rerender(<LiveIndicatorView view={view({ status: "reconnecting", failures: 1, retryAt: T + 2500 })} now={T + 2600} />);
    expect(seen()).toBe("Reconnecting…");
  });

  it("says Offline with the time the data is from, a wifi-off icon, and a dashed outline", () => {
    render(<LiveIndicatorView view={view({ status: "offline", failures: 3, retryAt: T + 9000, dataAsOf: T })} now={T} />);
    const pill = screen.getByRole("status");
    expect(pill).toHaveTextContent("Offline · data from 10:42 UTC");
    expect(pill.className).toContain("border-dashed");
    expect(pill.querySelector("svg")).not.toBeNull();
  });

  it("says Offline without a time when it never heard from the hub", () => {
    render(<LiveIndicatorView view={view({ status: "offline", failures: 3, retryAt: T + 9000, dataAsOf: null })} now={T} />);
    expect(seen()).toBe("Offline");
  });

  it("says Live updates stopped when asking again will not help", () => {
    render(<LiveIndicatorView view={view({ status: "stopped", stopReason: "forbidden" })} now={T} />);
    expect(screen.getByRole("status")).toHaveTextContent("Live updates stopped");
  });

  it("announces a change of status, not every tick of the countdown", () => {
    const { rerender } = render(<LiveIndicatorView view={view({ status: "reconnecting", failures: 1, retryAt: T + 3000 })} now={T} />);
    const announced = () => screen.getByRole("status").querySelector(".sr-only")?.textContent;
    const before = announced();
    expect(before).toMatch(/reconnecting/i);
    rerender(<LiveIndicatorView view={view({ status: "reconnecting", failures: 1, retryAt: T + 3000 })} now={T + 2000} />);
    expect(announced()).toBe(before);
    // The ticking text is hidden from assistive technology; the stable sentence is what a screen reader gets.
    expect(screen.getByRole("status").querySelector("[aria-hidden='true']:not([data-shape])")?.textContent).toMatch(/retry in 1s/);
  });

  it("has no accessibility violations in any state", async () => {
    for (const v of [view({ status: "live" }), view({ status: "connecting" }), view({ status: "reconnecting", retryAt: T + 3000 }), view({ status: "offline", dataAsOf: T }), view({ status: "stopped", stopReason: "bad_request" })]) {
      const { unmount } = render(<LiveIndicatorView view={v} now={T} />);
      await expectNoAxeViolations();
      unmount();
    }
  });
});

describe("PausedBannerView", () => {
  it.each(["idle", "connecting", "live", "reconnecting"] as const)("is not shown while the stream is %s", (status) => {
    const { container } = render(<PausedBannerView view={view({ status, retryAt: T + 3000, failures: 1 })} now={T} onRetry={() => undefined} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("says updates are paused and retrying, when the data is from, and that saving and restarts are off", () => {
    render(<PausedBannerView view={view({ status: "offline", failures: 3, retryAt: T + 12_000, dataAsOf: T })} now={T} onRetry={() => undefined} />);
    const banner = screen.getByRole("status");
    expect(banner).toHaveTextContent("Live updates paused, retrying");
    expect(banner).toHaveTextContent("Showing data from 10:42 UTC. Saving and restarts are off until the connection returns.");
    expect(banner).toHaveTextContent("Next attempt in 12s");
  });

  it("offers Retry now, and asks for an attempt when it is pressed", async () => {
    const onRetry = vi.fn();
    render(<PausedBannerView view={view({ status: "offline", failures: 3, retryAt: T + 12_000, dataAsOf: T })} now={T} onRetry={onRetry} />);
    await userEvent.setup().click(screen.getByRole("button", { name: "Retry now" }));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("says when the data is from even if the stream never opened", () => {
    render(<PausedBannerView view={view({ status: "offline", failures: 3, retryAt: T + 12_000, dataAsOf: null })} now={T} onRetry={() => undefined} />);
    expect(screen.getByRole("status")).toHaveTextContent(/Showing the data from the last time this page loaded/);
  });

  it.each([
    ["forbidden", /do not have access to live updates/i],
    ["bad_request", /live connection was refused/i],
  ] as const)("says why updates stopped (%s), and offers no retry that cannot work", (stopReason, text) => {
    render(<PausedBannerView view={view({ status: "stopped", stopReason, dataAsOf: T })} now={T} onRetry={() => undefined} />);
    const banner = screen.getByRole("status");
    expect(banner).toHaveTextContent("Live updates stopped");
    expect(banner).toHaveTextContent(text);
    expect(screen.queryByRole("button", { name: "Retry now" })).not.toBeInTheDocument();
  });

  it("has no accessibility violations", async () => {
    render(<PausedBannerView view={view({ status: "offline", failures: 3, retryAt: T + 12_000, dataAsOf: T })} now={T} onRetry={() => undefined} />);
    await expectNoAxeViolations();
  });
});

describe("connected to the stream", () => {
  const ME = { id: "u1", email: "ada@example.test", display_name: "Ada", role: "viewer" } as unknown as Me;
  const session: SessionState = { status: "ready", me: ME, role: "viewer" satisfies Role };
  let hub: FakeHub;

  beforeEach(() => {
    vi.useFakeTimers();
    hub = new FakeHub();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  function App() {
    return (
      <QueryClientProvider client={new QueryClient()}>
        <SessionContext.Provider value={session}>
          <LiveProvider config={{ stream: { fetch: hub.fetch, random: () => 0.5, onlineTarget: new EventTarget() } }}>
            <LiveIndicator />
            <PausedBanner />
          </LiveProvider>
        </SessionContext.Provider>
      </QueryClientProvider>
    );
  }
  const advance = (ms: number) =>
    act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });

  it("goes Connecting, Live, Reconnecting, Offline with the banner, and back to Live when the stream returns", async () => {
    render(<App />);
    await advance(1);
    expect(screen.getByRole("status")).toHaveTextContent("Connecting");
    await act(async () => {
      hub.last.push(FIRST_FRAME);
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByRole("status")).toHaveTextContent("Live");
    expect(screen.queryByText("Live updates paused, retrying")).not.toBeInTheDocument();

    await act(async () => {
      hub.last.fail();
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByRole("status")).toHaveTextContent(/Reconnecting/);
    for (let i = 1; i < OFFLINE_AFTER_FAILURES; i++) {
      await advance(20_000);
      await act(async () => {
        hub.last.fail();
        await vi.advanceTimersByTimeAsync(0);
      });
    }
    expect(screen.getAllByRole("status").map((s) => s.textContent).join(" | ")).toMatch(/Offline · data from 10:4?\d UTC|Offline · data from \d\d:\d\d UTC/);
    expect(screen.getByText("Live updates paused, retrying")).toBeInTheDocument();

    const before = hub.connections.length;
    await act(async () => {
      screen.getByRole("button", { name: "Retry now" }).click();
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(hub.connections.length).toBe(before + 1);
    await act(async () => {
      hub.last.push(FIRST_FRAME);
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByRole("status")).toHaveTextContent("Live");
    expect(screen.queryByText("Live updates paused, retrying")).not.toBeInTheDocument();
  });
});
