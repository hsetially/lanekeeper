import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { forAll, type Rng } from "@/test/prng";
import { MAX_FRAME_CHARS, SseParseError, SseParser, type SseItem } from "./sse-parser";

// The parser follows the WHATWG rules for the text/event-stream format (https://html.spec.whatwg.org/#event-stream-interpretation),
// with two deliberate differences, both for the hub's frames:
//  - a frame reports ITS OWN `id:` line, and null when it has none. The WHATWG "last event ID buffer" is sticky, so a
//    `resync` the hub makes up (it has no id) would look as if it carried the id before it, and a client would move its resume
//    point onto a frame that is not an event of the log;
//  - an incomplete frame at the end of the stream is dropped, and a frame without a `data:` line is not an event.

// PROVENANCE OF THE GOLDEN FRAMES. e2e/fixtures/sse-golden/*.txt were written BY HAND, by T1, from the format of
// crates/hub-platform/src/events/sse.rs (retry_frame, heartbeat_frame, event_frame). The hub's own tests do not dump bytes,
// so they were NOT captured from a running hub. They are the best statement we have of the wire format, and this file
// replays them verbatim; when the hub gets a test that dumps real bytes, replace them and nothing here changes.
const golden = (name: string): string => readFileSync(new URL(`../../e2e/fixtures/sse-golden/${name}`, import.meta.url), "utf8");

function parse(text: string): SseItem[] {
  const parser = new SseParser();
  return parser.push(text);
}

const frames = (items: SseItem[]) => items.flatMap((i) => (i.kind === "frame" ? [i.frame] : []));

describe("the golden frames (hand-built from the hub's sse.rs, not captured from a live hub)", () => {
  it("reads the first frame as a retry hint of 3000 ms and nothing else", () => {
    expect(parse(golden("first-frame.txt"))).toEqual([{ kind: "retry", ms: 3000 }]);
  });

  it("reads a heartbeat as a comment, which is not an event and carries no id", () => {
    expect(parse(golden("heartbeat.txt"))).toEqual([{ kind: "comment", text: "heartbeat" }]);
  });

  it("reads `id: 41` with its space, `event:` and one data line, as one frame", () => {
    const [frame] = frames(parse(golden("file-observed.txt")));
    expect(frame).toEqual({
      id: "41",
      event: "file_observed",
      data: '{"type":"file_observed","swimlane":"sit1","path":"core/core-sit1.yml","hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","source":"scan"}',
    });
  });

  it("reads the second stored event with its own id", () => {
    expect(frames(parse(golden("findings-changed.txt")))[0]).toMatchObject({ id: "42", event: "findings_changed" });
  });

  it("reads a resync with NO id, even right after a frame that had one", () => {
    const items = parse(golden("file-observed.txt") + golden("resync.txt"));
    const [first, second] = frames(items);
    expect(first?.id).toBe("41");
    expect(second).toEqual({ id: null, event: "resync", data: '{"type":"resync"}' });
  });

  it("reads the final retry frame like the first", () => {
    expect(parse(golden("final-retry.txt"))).toEqual([{ kind: "retry", ms: 3000 }]);
  });

  it("reads a whole stream of every golden frame in order", () => {
    const names = ["first-frame.txt", "heartbeat.txt", "file-observed.txt", "findings-changed.txt", "resync.txt", "final-retry.txt"];
    const items = parse(names.map(golden).join(""));
    expect(items.map((i) => i.kind)).toEqual(["retry", "comment", "frame", "frame", "frame", "retry"]);
  });
});

describe("the WHATWG rules", () => {
  it("accepts LF, CRLF and CR as line ends, in any mix", () => {
    for (const eol of ["\n", "\r\n", "\r"]) {
      const text = `id: 7${eol}event: x${eol}data: a${eol}data: b${eol}${eol}`;
      expect(frames(parse(text))).toEqual([{ id: "7", event: "x", data: "a\nb" }]);
    }
    expect(frames(parse("data: a\r\ndata: b\rdata: c\n\r\n"))).toEqual([{ id: null, event: "message", data: "a\nb\nc" }]);
  });

  it("strips exactly one space after the colon, and none when there is none", () => {
    expect(frames(parse("data:  two\n\n"))[0]?.data).toBe(" two");
    expect(frames(parse("data:none\n\n"))[0]?.data).toBe("none");
    expect(frames(parse("data\n\n"))).toEqual([{ id: null, event: "message", data: "" }]); // a `data` line with no value is an empty data line: the spec dispatches it
  });

  it("names the event `message` when there is no event line", () => {
    expect(frames(parse("data: x\n\n"))[0]?.event).toBe("message");
  });

  it("ignores a BOM at the very start of the stream, and only there", () => {
    expect(frames(parse("\uFEFFdata: a\n\n"))[0]?.data).toBe("a");
    const parser = new SseParser();
    parser.push("data: a\n\n");
    expect(frames(parser.push("\uFEFFdata: b\n\n"))).toEqual([]); // a BOM in the middle makes the field name `\uFEFFdata`, which is unknown
  });

  it("does not dispatch a frame that has no data line, and forgets its fields", () => {
    expect(frames(parse("id: 9\nevent: x\n\ndata: y\n\n"))).toEqual([{ id: null, event: "message", data: "y" }]);
  });

  it("drops an `id` that contains NUL, and keeps the frame", () => {
    expect(frames(parse("id: 1\u00002\ndata: a\n\n"))[0]?.id).toBeNull();
  });

  it("takes `retry` only when it is all ASCII digits", () => {
    expect(parse("retry: 1500\n\n")).toEqual([{ kind: "retry", ms: 1500 }]);
    expect(parse("retry: 15s\n\nretry: -3\n\nretry:\n\n")).toEqual([]);
  });

  it("ignores unknown fields", () => {
    expect(frames(parse("foo: bar\ndata: a\n\n"))).toEqual([{ id: null, event: "message", data: "a" }]);
  });

  it("drops an incomplete frame at the end of the stream", () => {
    expect(frames(parse("data: a\n\ndata: b"))).toEqual([{ id: null, event: "message", data: "a" }]);
  });

  it("carries a comment that has no space, and an empty one", () => {
    expect(parse(":x\n:\n\n")).toEqual([{ kind: "comment", text: "x" }, { kind: "comment", text: "" }]);
  });
});

describe("limits (rule 5)", () => {
  it("refuses a frame over 64 KiB of text, whatever the number of lines", () => {
    expect(MAX_FRAME_CHARS).toBe(64 * 1024);
    const big = "x".repeat(MAX_FRAME_CHARS + 1);
    expect(() => parse(`data: ${big}\n\n`)).toThrow(SseParseError);
    const parser = new SseParser();
    const line = `data: ${"y".repeat(1000)}\n`;
    expect(() => {
      for (let i = 0; i < 100; i++) parser.push(line);
    }).toThrow(/too large/i);
  });

  it("refuses a line that never ends before it grows past the cap", () => {
    const parser = new SseParser();
    expect(() => {
      for (let i = 0; i < 100; i++) parser.push("z".repeat(1000));
    }).toThrow(SseParseError);
  });

  it("accepts a frame just under the cap", () => {
    const ok = "x".repeat(MAX_FRAME_CHARS - 10);
    expect(frames(parse(`data: ${ok}\n\n`))[0]?.data).toHaveLength(MAX_FRAME_CHARS - 10);
  });

  it("does not count comments (heartbeats) against a frame, however many there are", () => {
    const parser = new SseParser();
    for (let i = 0; i < 20_000; i++) parser.push(": heartbeat\n\n");
    expect(frames(parser.push("data: a\n\n"))).toHaveLength(1);
  });

  it("stays failed once it has failed: a poisoned stream is not read on", () => {
    const parser = new SseParser();
    expect(() => parser.push(`data: ${"x".repeat(MAX_FRAME_CHARS + 1)}\n\n`)).toThrow(SseParseError);
    expect(() => parser.push("data: a\n\n")).toThrow(SseParseError);
  });
});

// A stream made of random frames, with every kind of line end, comments and field noise. Splitting it anywhere (inside a
// CRLF, inside a field name, inside a multi-byte character) must give the same items as parsing it whole.
const ALPHABET = "abcXYZ019 :{}\",é漢😀";

function randomStream(rng: Rng): string {
  const eols = ["\n", "\r\n", "\r"] as const;
  let out = rng.bool() ? "\uFEFF" : "";
  const count = rng.int(8);
  for (let f = 0; f < count; f++) {
    const eol = () => rng.pick(eols);
    const lines: string[] = [];
    if (rng.int(4) === 0) lines.push(`: ${rng.string(ALPHABET, 12)}`);
    if (rng.int(3) === 0) lines.push(`retry: ${rng.int(10_000)}`);
    if (rng.int(2) === 0) lines.push(rng.bool() ? `id: ${1 + rng.int(1_000_000)}` : `id:${1 + rng.int(99)}`);
    if (rng.int(2) === 0) lines.push(`event: ${rng.pick(["file_observed", "resync", "findings_changed", "x"])}`);
    const dataLines = rng.int(4);
    for (let d = 0; d < dataLines; d++) lines.push(rng.bool() ? `data: ${rng.string(ALPHABET, 20)}` : `data:${rng.string(ALPHABET, 20)}`);
    if (rng.int(6) === 0) lines.push(rng.string("ab", 3));
    for (const line of lines) out += line + eol();
    out += eol();
  }
  return out;
}

function splitAt(text: string, rng: Rng): string[] {
  const parts: string[] = [];
  let i = 0;
  while (i < text.length) {
    const size = 1 + rng.int(7);
    parts.push(text.slice(i, i + size));
    i += size;
  }
  return parts;
}

describe("property: the parse does not depend on where the stream is cut (10,000 cases)", () => {
  const generate = (rng: Rng) => {
    const text = randomStream(rng);
    return { text, pieces: splitAt(text, rng) };
  };

  it("gives the same items whole, one character at a time, and in random pieces", () => {
    forAll(10_000, generate, ({ text, pieces }, index) => {
      const whole = parse(text);
      const single = new SseParser();
      expect(Array.from(text).flatMap((c) => single.push(c)), `case ${index} char by char`).toEqual(whole);
      const cut = new SseParser();
      expect(pieces.flatMap((p) => cut.push(p)), `case ${index} in pieces`).toEqual(whole);
    });
  }, 60_000);

  it("never throws on any stream, and never reports an id the stream did not carry", () => {
    forAll(10_000, randomStream, (text) => {
      const items = parse(text);
      const ids = new Set([...text.matchAll(/id: ?(\d+)/g)].map((m) => m[1]));
      for (const frame of frames(items)) if (frame.id !== null) expect(ids.has(frame.id)).toBe(true);
    });
  }, 60_000);
});
