// A hub that is a function of the test, for the tests of the live stream: each connection gets a response whose frames the
// test pushes by hand, and which it can end or break on purpose. Nothing here is a mock of the client: the client under test
// is the real LiveStream, reading a real ReadableStream through a real Response.
//
// The frames the tests push are the golden ones of e2e/fixtures/sse-golden, or built by `frame` in the same format. The golden
// files were written BY HAND by T1 from crates/hub-platform/src/events/sse.rs; they were not captured from a live hub.

export const frame = (id: number | null, event: object): string => `${id === null ? "" : `id: ${id}\n`}event: ${(event as { type: string }).type}\ndata: ${JSON.stringify(event)}\n\n`;

export type Connection = {
  url: string;
  headers: Headers;
  signal: AbortSignal;
  init: RequestInit;
  push(text: string): void;
  end(): void;
  fail(): void;
};

export class FakeHub {
  readonly connections: Connection[] = [];
  /** What the next connection answers with; when empty, a 200 event stream that opens with the hub's `retry: 3000`. */
  private readonly script: Array<(conn: Connection) => Response | undefined> = [];
  fetch = (input: string | URL | Request, init: RequestInit = {}): Promise<Response> => {
    const signal = init.signal ?? new AbortController().signal;
    let controller!: ReadableStreamDefaultController<Uint8Array>;
    const encoder = new TextEncoder();
    const body = new ReadableStream<Uint8Array>({
      start(c) {
        controller = c;
      },
    });
    const conn: Connection = {
      url: input instanceof Request ? input.url : String(input),
      headers: new Headers(init.headers),
      signal,
      init,
      // A real connection that is already over ignores what the test does to it afterwards.
      push: (text) => {
        try {
          controller.enqueue(encoder.encode(text));
        } catch {
          // closed or errored
        }
      },
      end: () => {
        try {
          controller.close();
        } catch {
          // closed or errored
        }
      },
      fail: () => {
        try {
          controller.error(new TypeError("network error"));
        } catch {
          // closed or errored
        }
      },
    };
    signal.addEventListener("abort", () => {
      try {
        controller.error(new DOMException("aborted", "AbortError"));
      } catch {
        // already closed
      }
    });
    this.connections.push(conn);
    const custom = this.script.shift()?.(conn);
    if (signal.aborted) return Promise.reject(new DOMException("aborted", "AbortError"));
    if (custom) return Promise.resolve(custom);
    return Promise.resolve(new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } }));
  };

  /** The next connection answers with this response instead of a stream. */
  answer(make: (conn: Connection) => Response | undefined): void {
    this.script.push(make);
  }
  respond(status: number, init: { type?: string; headers?: Record<string, string>; body?: string } = {}): void {
    this.answer(() => new Response(init.body ?? JSON.stringify({ type: `urn:lanekeeper:problem:${init.type ?? "x"}`, title: "t", status }), { status, headers: { "content-type": "application/problem+json", ...init.headers } }));
  }
  get last(): Connection {
    const conn = this.connections.at(-1);
    if (!conn) throw new Error("no connection yet");
    return conn;
  }
  /** Connections the client still holds open (not aborted). */
  get open(): number {
    return this.connections.filter((c) => !c.signal.aborted).length;
  }
}

