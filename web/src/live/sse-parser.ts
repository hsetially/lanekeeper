// A text/event-stream parser, by the WHATWG rules (https://html.spec.whatwg.org/#event-stream-interpretation), for the
// hub's `GET /api/v1/events`. It takes decoded text in pieces of any size and returns the items it completed.
//
// What the hub writes (crates/hub-platform/src/events/sse.rs): `retry: 3000`, then replay or `resync`, then one frame per
// event (`id: N`, `event: <kind>`, `data: <json>`), and `: heartbeat` every 15 s. Two things differ from the WHATWG
// EventSource, on purpose:
//  - a frame reports its OWN `id:` line, null when it has none. The spec keeps a sticky "last event ID", so a `resync` the
//    server makes up (it has no id) would seem to carry the id before it, and a client would move its resume point onto a
//    frame that is not an event of the log;
//  - an incomplete frame at the end of the stream is dropped, and a frame with no `data:` line is not an event (as in the spec).
//
// Everything is bounded (rule 5, S11): a frame may hold at most MAX_FRAME_CHARS of text (the hub's own events are at most
// 4 KiB), and a line that never ends is cut off at the same cap. A parser that has refused a frame stays failed: the caller
// drops the connection, because what follows a refused frame cannot be trusted to start at a frame boundary.

/** The most text one frame (its fields, summed) or one unfinished line may hold. */
export const MAX_FRAME_CHARS = 64 * 1024;

export type SseFrame = {
  /** The `id:` line of this frame, or null. Never carried over from an earlier frame. */
  readonly id: string | null;
  /** The `event:` name, `message` when the frame has none. */
  readonly event: string;
  /** The `data:` lines joined by a newline. */
  readonly data: string;
};

export type SseItem =
  | { readonly kind: "frame"; readonly frame: SseFrame }
  /** A `retry:` field: the server's reconnection hint in milliseconds. */
  | { readonly kind: "retry"; readonly ms: number }
  /** A `:` line. The hub's heartbeat. It is not an event, but it proves the stream is alive. */
  | { readonly kind: "comment"; readonly text: string };

export class SseParseError extends Error {
  override readonly name = "SseParseError";
  readonly code: "frame_too_large" | "line_too_long";
  constructor(code: "frame_too_large" | "line_too_long") {
    super(code === "frame_too_large" ? "an event frame is too large" : "an event line is too long");
    this.code = code;
  }
}

const CR = 13;
const LF = 10;
const BOM = 0xfeff;
const DIGITS = /^[0-9]+$/;

export class SseParser {
  private partial = "";
  private atStart = true;
  private skipLf = false;
  private failed: SseParseError | null = null;
  // The frame being read.
  private id: string | null = null;
  private event = "";
  private data = "";
  private hasData = false;
  private size = 0;

  /** Feed the next decoded piece. Returns the items completed by it, in order. Throws SseParseError past a limit. */
  push(chunk: string): SseItem[] {
    if (this.failed) throw this.failed;
    const out: SseItem[] = [];
    const text = this.partial + chunk;
    this.partial = "";
    let i = 0;
    if (this.atStart && text.length > 0) {
      this.atStart = false;
      if (text.charCodeAt(0) === BOM) i = 1;
    }
    if (this.skipLf && i < text.length) {
      // The piece before ended with a CR; an LF right after it belongs to the same line end.
      if (text.charCodeAt(i) === LF) i += 1;
      this.skipLf = false;
    }
    try {
      for (;;) {
        let j = i;
        while (j < text.length) {
          const c = text.charCodeAt(j);
          if (c === LF || c === CR) break;
          j += 1;
        }
        if (j >= text.length) {
          this.partial = text.slice(i);
          if (this.partial.length > MAX_FRAME_CHARS) throw new SseParseError("line_too_long");
          break;
        }
        const line = text.slice(i, j);
        if (text.charCodeAt(j) === CR) {
          if (j + 1 < text.length) i = text.charCodeAt(j + 1) === LF ? j + 2 : j + 1;
          else {
            i = j + 1;
            this.skipLf = true;
          }
        } else {
          i = j + 1;
        }
        this.line(line, out);
      }
    } catch (error) {
      if (error instanceof SseParseError) this.failed = error;
      throw error;
    }
    return out;
  }

  private line(line: string, out: SseItem[]): void {
    if (line === "") {
      this.dispatch(out);
      return;
    }
    if (line.charCodeAt(0) === 58) {
      // A comment. It does not belong to a frame, so it does not count against one.
      out.push({ kind: "comment", text: stripOneSpace(line.slice(1)) });
      return;
    }
    const colon = line.indexOf(":");
    const field = colon < 0 ? line : line.slice(0, colon);
    const value = colon < 0 ? "" : stripOneSpace(line.slice(colon + 1));
    this.size += line.length + 1;
    if (this.size > MAX_FRAME_CHARS) throw new SseParseError("frame_too_large");
    switch (field) {
      case "event":
        this.event = value;
        break;
      case "data":
        this.data += `${value}\n`;
        this.hasData = true;
        break;
      case "id":
        if (!value.includes("\u0000")) this.id = value;
        break;
      case "retry":
        // Takes effect when read, not when the frame ends, as in the spec.
        if (DIGITS.test(value) && value.length <= 9) out.push({ kind: "retry", ms: Number(value) });
        break;
      default:
        break;
    }
  }

  private dispatch(out: SseItem[]): void {
    const frame: SseFrame | null = this.hasData ? { id: this.id, event: this.event === "" ? "message" : this.event, data: this.data.slice(0, -1) } : null;
    this.id = null;
    this.event = "";
    this.data = "";
    this.hasData = false;
    this.size = 0;
    if (frame) out.push({ kind: "frame", frame });
  }
}

function stripOneSpace(value: string): string {
  return value.startsWith(" ") ? value.slice(1) : value;
}
