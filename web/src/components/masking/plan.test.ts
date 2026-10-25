import { describe, expect, it } from "vitest";
import type { Mask } from "@/api/generated/model";
import { forAll, type Rng } from "@/test/prng";
import { lfLines, MAX_MASKS, planMasks, segmentsOf, type MaskPlan, type Segment } from "./plan";

// What the server's `masks` mean for a text (S14b, M1). `start` and `end` are UTF-8 byte offsets into the whole `text`, half open,
// and `line` is the 1-based line that holds the marker. The plan turns them into places inside each line, and it FAILS CLOSED:
// a mask the plan cannot trust never lets the text it points at be drawn as text. A mask whose line can be found but whose span
// cannot be checked hides that whole line; a mask that cannot be placed at all (it names no line of this text, or a range that
// leaves its line) refuses the whole text. The web never guesses what the server meant.

const enc = new TextEncoder();
const bytes = (s: string): number => enc.encode(s).length;
const MARK = "[REDACTED: 12 chars, aws_key]";

/** The byte offset of `needle` in `text`, as the server would count it. */
const at = (text: string, needle: string, from = 0): number => bytes(text.slice(0, text.indexOf(needle, from)));
const maskOf = (text: string, line: number, needle = MARK, extra: Partial<Mask> = {}): Mask => {
  const start = at(text, needle);
  return { line, start, end: start + bytes(needle), ...extra };
};

function okPlan(plan: MaskPlan) {
  if (plan.status !== "ok") throw new Error(`expected an ok plan, got ${plan.status}`);
  return plan.lines;
}
/** What a line draws as text, joined: the part of the line that is not a hidden value. */
const plainOf = (segments: Segment[]): string => segments.filter((s) => s.kind === "text").map((s) => s.text).join("");

describe("lfLines", () => {
  it("splits on line feeds only, keeps a carriage return with its line, and has no empty last line", () => {
    expect(lfLines("a\r\nb\nc")).toEqual(["a\r", "b", "c"]);
    expect(lfLines("a\nb\n")).toEqual(["a", "b"]);
    expect(lfLines("a\rb")).toEqual(["a\rb"]);
    expect(lfLines("")).toEqual([]);
    expect(lfLines("\n")).toEqual([""]);
  });
});

describe("planMasks: when there is nothing to plan", () => {
  it("has no plan for absent or empty masks (absent means the server supplied none)", () => {
    expect(planMasks("a: 1\n", undefined)).toEqual({ status: "none" });
    expect(planMasks("a: 1\n", [])).toEqual({ status: "none" });
  });
});

describe("planMasks: a valid mask", () => {
  const text = `a: 1\nsecret: ${MARK}\nb: 2\n`;

  it("places the marker inside its line, with the reveal's setting and the marker's own facts", () => {
    const lines = okPlan(planMasks(text, [maskOf(text, 2, MARK, { setting: "a.secret" })]));
    const plan = lines.get(2);
    expect(plan).toMatchObject({ kind: "marks", marks: [{ start: 8, end: 8 + MARK.length, line: 2, setting: "a.secret", info: { chars: 12, rule: "aws_key" } }] });
    expect(lines.size).toBe(1);
  });

  it("has no reveal for a mask without a setting, a null one, an empty one or one that is not text", () => {
    for (const setting of [undefined, null, "", 5 as unknown as string, "x".repeat(1025)]) {
      const lines = okPlan(planMasks(text, [{ ...maskOf(text, 2), ...(setting === undefined ? {} : { setting }) }]));
      const plan = lines.get(2);
      expect(plan?.kind === "marks" ? plan.marks[0]?.setting : "?").toBeNull();
    }
  });

  it("counts bytes, not characters: a line with multi-byte characters before the marker", () => {
    const wide = `é😀 名前: ${MARK} ✓\nport: 1\n`;
    const lines = okPlan(planMasks(wide, [maskOf(wide, 1)]));
    const plan = lines.get(1);
    expect(plan?.kind).toBe("marks");
    const marks = plan?.kind === "marks" ? plan.marks : [];
    expect(wide.split("\n")[0]?.slice(marks[0]?.start, marks[0]?.end)).toBe(MARK);
  });

  it("counts the bytes of the lines before it, with their carriage returns", () => {
    const crlf = `é: 1\r\n😀: 2\r\nk: ${MARK}\r\nz: 3\r\n`;
    const lines = okPlan(planMasks(crlf, [maskOf(crlf, 3)]));
    const plan = lines.get(3);
    const marks = plan?.kind === "marks" ? plan.marks : [];
    expect(lfLines(crlf)[2]?.slice(marks[0]?.start, marks[0]?.end)).toBe(MARK);
  });

  it("handles two markers on one line, whatever order the server listed them in", () => {
    const two = `a: ${MARK} b: ${MARK.replace("12", "30")}\n`;
    const second = `[REDACTED: 30 chars, aws_key]`;
    const lines = okPlan(planMasks(two, [maskOf(two, 1, second), maskOf(two, 1, MARK)]));
    const plan = lines.get(1);
    const marks = plan?.kind === "marks" ? plan.marks : [];
    expect(marks.map((m) => m.info.chars)).toEqual([12, 30]);
  });

  it("handles a marker on the last line with no final line feed, and on the first line", () => {
    const t = `${MARK}\nlast: ${MARK}`;
    const lines = okPlan(planMasks(t, [maskOf(t, 1), { line: 2, start: bytes(`${MARK}\nlast: `), end: bytes(`${MARK}\nlast: ${MARK}`) }]));
    expect([...lines.keys()]).toEqual([1, 2]);
  });
});

describe("planMasks: a mask it can place but not trust hides the whole line", () => {
  it("hides the line when the span is not a marker (the server did not replace the value)", () => {
    const text = "a: 1\nclientSecret: hunter2hunter2\nb: 2\n";
    const start = at(text, "hunter2");
    const lines = okPlan(planMasks(text, [{ line: 2, start, end: start + bytes("hunter2hunter2") }]));
    expect(lines.get(2)).toMatchObject({ kind: "hidden" });
    expect(segmentsOf("clientSecret: hunter2hunter2", lines.get(2))).toEqual([{ kind: "hidden" }]);
  });

  it("hides the line when the span is only part of a marker, or a marker and more", () => {
    const text = `k: ${MARK} tail\n`;
    const start = at(text, MARK);
    for (const [from, to] of [
      [start, start + 5],
      [start + 1, start + bytes(MARK)],
      [start - 1, start + bytes(MARK)],
      [start, start + bytes(MARK) + 1],
    ] as const) {
      const lines = okPlan(planMasks(text, [{ line: 1, start: from, end: to }]));
      expect(lines.get(1)?.kind, `${from}-${to}`).toBe("hidden");
    }
  });

  it("hides the line when the span cuts a multi-byte character in two", () => {
    const text = `😀 ${MARK}\n`;
    const lines = okPlan(planMasks(text, [{ line: 1, start: 1, end: 1 + bytes(MARK) }]));
    expect(lines.get(1)?.kind).toBe("hidden");
  });

  it("hides the line when two masks overlap, touch or repeat", () => {
    const text = `k: ${MARK} tail\n`;
    const a = maskOf(text, 1);
    for (const b of [a, { ...a, start: a.start + 2 }, { line: 1, start: a.end, end: a.end + 1 }]) {
      const lines = okPlan(planMasks(text, [a, b]));
      expect(lines.get(1)?.kind).toBe("hidden");
    }
  });

  it("hides only the lines it cannot trust and still places the others", () => {
    const text = `a: ${MARK}\nb: plain-secret\nc: ${MARK}\n`;
    const start = at(text, "plain-secret");
    const lines = okPlan(planMasks(text, [maskOf(text, 1), { line: 2, start, end: start + 12 }, { line: 3, start: at(text, MARK, text.indexOf("c:")), end: at(text, MARK, text.indexOf("c:")) + bytes(MARK) }]));
    expect(lines.get(1)?.kind).toBe("marks");
    expect(lines.get(2)?.kind).toBe("hidden");
    expect(lines.get(3)?.kind).toBe("marks");
  });

  it("hides a line too long to check", () => {
    const text = `${"x".repeat(150_000)}${MARK}\n`;
    const start = 150_000;
    const lines = okPlan(planMasks(text, [{ line: 1, start, end: start + bytes(MARK) }]));
    expect(lines.get(1)?.kind).toBe("hidden");
  });
});

describe("planMasks: a mask it cannot place refuses the whole text", () => {
  const text = `a: 1\nk: ${MARK}\nb: 2\n`;
  const good = maskOf(text, 2);

  it("refuses a malformed mask: a fractional, negative, backwards, empty or non-number offset", () => {
    for (const bad of [
      { ...good, start: 1.5 },
      { ...good, start: -1 },
      { ...good, end: good.start },
      { ...good, end: good.start - 1 },
      { ...good, start: Number.NaN },
      { ...good, end: Number.POSITIVE_INFINITY },
      { ...good, line: 0 },
      { ...good, line: 1.2 },
      { ...good, line: "2" as unknown as number },
    ]) {
      expect(planMasks(text, [bad]), JSON.stringify(bad)).toEqual({ status: "refused", reason: "malformed" });
    }
  });

  it("refuses a mask that names a line the text does not have", () => {
    expect(planMasks(text, [{ ...good, line: 4 }])).toEqual({ status: "refused", reason: "missing_line" });
    expect(planMasks("", [{ line: 1, start: 0, end: 5 }])).toEqual({ status: "refused", reason: "missing_line" });
  });

  it("refuses a range that leaves the line it names, either way, even when it is a marker", () => {
    expect(planMasks(text, [{ ...good, line: 1 }])).toEqual({ status: "refused", reason: "outside_line" });
    expect(planMasks(text, [{ ...good, line: 3 }])).toEqual({ status: "refused", reason: "outside_line" });
    expect(planMasks(text, [{ ...good, end: bytes(text) + 10 }])).toEqual({ status: "refused", reason: "outside_line" });
  });

  it("refuses a range that runs across a line break, whichever line it names", () => {
    const across = `a: ${MARK}\nb: ${MARK}\n`;
    const start = at(across, MARK);
    const crossing = { start, end: at(across, MARK, across.indexOf("b:")) + bytes(MARK) };
    for (const line of [1, 2]) expect(planMasks(across, [{ line, ...crossing }]), `line ${line}`).toEqual({ status: "refused", reason: "outside_line" });
  });

  it("refuses more masks than the contract allows", () => {
    const many = Array.from({ length: MAX_MASKS + 1 }, () => good);
    expect(planMasks(text, many)).toEqual({ status: "refused", reason: "too_many" });
    expect(planMasks(text, many.slice(0, MAX_MASKS)).status).not.toBe("refused");
  });

  it("refuses the whole text even when only one mask of many is bad", () => {
    expect(planMasks(text, [good, { ...good, line: 9 }])).toEqual({ status: "refused", reason: "missing_line" });
  });
});

describe("segmentsOf", () => {
  it("draws the text around a placed marker as text and the marker as a marker", () => {
    const line = `k: ${MARK} # note`;
    const text = `${line}\n`;
    const lines = okPlan(planMasks(text, [maskOf(text, 1, MARK, { setting: "k" })]));
    const segments = segmentsOf(line, lines.get(1));
    expect(segments.map((s) => s.kind)).toEqual(["text", "mark", "text"]);
    expect(segments[1]).toMatchObject({ kind: "mark", text: MARK, planned: { setting: "k" } });
  });

  it("recognises a marker past the list by its form, with no reveal", () => {
    const segments = segmentsOf(`k: ${MARK}`, undefined);
    expect(segments.map((s) => s.kind)).toEqual(["text", "mark"]);
    expect(segments[1]).toMatchObject({ kind: "mark", planned: null });
  });

  it("recognises a marker in the stretches between placed ones", () => {
    const line = `a: ${MARK} b: [REDACTED: 5 chars, jwt] c: ${MARK}`;
    const text = `${line}\n`;
    const first = maskOf(text, 1);
    const lastStart = bytes(line.slice(0, line.lastIndexOf(MARK)));
    const lines = okPlan(planMasks(text, [first, { line: 1, start: lastStart, end: lastStart + bytes(MARK) }]));
    const segments = segmentsOf(line, lines.get(1));
    expect(segments.flatMap((s) => (s.kind === "mark" ? [s.planned !== null] : []))).toEqual([true, false, true]);
    expect(plainOf(segments)).toBe("a:  b:  c: ");
  });

  it("is the line itself when there is nothing to hide", () => {
    expect(segmentsOf("port: 8080", undefined)).toEqual([{ kind: "text", text: "port: 8080" }]);
  });
});

// The property that matters: whatever the server sends, the bytes a mask covers are never drawn as text. A text is built with a
// planted secret on some lines; the masks are valid, damaged or random; nothing the plan says to draw as text may contain the secret
// from a line a mask names.
describe("property: a covered span never comes out as text", () => {
  const SECRET = "SECRETSECRET";
  const PIECES = ["a", "b", " ", ":", "é", "😀", "名", "\r", "-", MARK];

  function build(rng: Rng): { text: string; masks: Mask[] } {
    const lines: string[] = [];
    const masks: Mask[] = [];
    let offset = 0;
    const count = 1 + rng.int(6);
    for (let i = 0; i < count; i++) {
      const before = Array.from({ length: rng.int(5) }, () => rng.pick(PIECES.slice(0, 9))).join("");
      const planted = rng.bool();
      const line = planted ? `${before}${SECRET}` : before;
      if (planted) {
        const start = offset + bytes(before);
        // Valid, shifted by a byte, widened, cut short, wrong line, or random.
        const kind = rng.int(6);
        const end = start + bytes(SECRET);
        if (kind === 0) masks.push({ line: i + 1, start, end });
        else if (kind === 1) masks.push({ line: i + 1, start: start + 1, end: end + 1 });
        else if (kind === 2) masks.push({ line: i + 1, start: start - 1, end });
        else if (kind === 3) masks.push({ line: i + 1, start, end: end - 3 });
        else if (kind === 4) masks.push({ line: Math.max(1, i), start, end });
        else masks.push({ line: 1 + rng.int(8), start: rng.int(80), end: rng.int(80) });
      }
      lines.push(line);
      offset += bytes(line) + 1;
    }
    return { text: `${lines.join("\n")}\n`, masks };
  }

  it("never draws, as text, a line that a mask names and whose span is not a marker", () => {
    forAll(
      400,
      build,
      ({ text, masks }) => {
        const plan = planMasks(text, masks);
        const rows = lfLines(text);
        if (plan.status === "refused") return;
        for (const mask of masks) {
          const display = (rows[mask.line - 1] ?? "").replace(/\r$/, "");
          const segments = segmentsOf(display, plan.status === "ok" ? plan.lines.get(mask.line) : undefined);
          // The span a mask covers is a marker or the line is hidden: either way the planted secret is not drawn as text on that line.
          expect(plainOf(segments), `line ${mask.line}`).not.toContain(SECRET);
        }
      },
    );
  });
});
