// The minute that frame S22 of "As served" lets go by (frames.ts, `aMinuteLater`) must never stall the live stream, whatever the speed of the page.
//
// The client takes 45 s without a byte for a dead stream and opens it again (src/live/sse-client.ts, STALL_MS). The frame moves the page's
// clock a minute on in four jumps with a heartbeat after each, so that the stream stays alive and the pill says "Live" while it is photographed.
// The jumps and the page's reading of the heartbeats are separate tasks: when a jump overtakes the reading, the client sees 45 s with no byte
// (the first run of 150 of the frame had one such run), reopens the stream, and the header's pill is photographed mid-reconnection, which moves
// the switcher and the search box by the difference of the two pills' widths. Here the reading is made slow on purpose, 300 ms for every chunk,
// so the overtaking is certain and not one run in a hundred: the step must wait for the page to have read each heartbeat, and the stream is
// opened once and kept.
import { expect, test } from "../../fixtures/test";
import { pin } from "../access/lib/app";
import type { Env } from "../search/kit";
import { ASKED, aMinuteLater, clockThatMoves, countHeartbeats } from "./frames";

const SIZE = { width: 1440, height: 900 } as const;

test("a minute on the page's clock never stalls the stream, even when the page reads every chunk 300 ms late", async ({ page, harness }) => {
  page.setDefaultTimeout(15_000);
  harness.setScenario("design-served-text");
  // Registered before anything that touches the clock (`pin` installs it), so that it takes the real timer: the clock replaces `setTimeout`,
  // and a delay on a faked timer would end at the first jump.
  await page.addInitScript(() => {
    const real = window.setTimeout.bind(window);
    const proto = ReadableStreamDefaultReader.prototype;
    const read = Reflect.get(proto, "read") as (this: ReadableStreamDefaultReader) => Promise<ReadableStreamReadResult<unknown>>;
    Reflect.set(proto, "read", function late(this: ReadableStreamDefaultReader) {
      return Reflect.apply(read, this, []).then((result: ReadableStreamReadResult<unknown>) => new Promise<ReadableStreamReadResult<unknown>>((resolve) => real(() => resolve(result), 300)));
    });
  });
  await pin(page, "light", SIZE, "fixed");
  const env: Env = { page, harness, size: SIZE };
  await clockThatMoves(env);
  await countHeartbeats(env);
  await page.goto(`${harness.url}${ASKED}`);
  await expect(page.getByRole("group", { name: "About this answer" })).toBeVisible();
  await aMinuteLater(env);
  // A stall shows late: the pill says "Reconnecting" about 0.35 s after the jump that caused it (measured, with the reads 300 ms late) and the
  // second request is there about 1.4 s after, so the pill is watched for two seconds, every 100 ms, and what it said is what is asserted.
  const pill = page.locator("[data-slot='live-indicator']");
  const said = new Set<string>();
  for (let i = 0; i < 20; i += 1) {
    said.add((await pill.getAttribute("data-status")) ?? "no pill");
    await page.waitForTimeout(100);
  }
  expect([...said], "the pill said Live from the first jump to two seconds after the last").toEqual(["live"]);
  expect(harness.requests().filter((r) => r.kind === "stream"), "the stream was opened once and kept").toHaveLength(1);
});
