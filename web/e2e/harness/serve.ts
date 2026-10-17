// The e2e harness: one origin that behaves like the hub for the browser (S12 tests).
//
//  - Every path that is not the API is proxied, untouched (status, headers, body), to the real hub header code: the Rust
//    example `web_fixture` (crates/hub/examples/web_fixture.rs), which serves web/dist with the production
//    Content-Security-Policy, nonce substitution, Trusted Types list and cache headers.
//  - `/api/v1/**` and `/auth/**` are answered from the mock scenario the test chose (src/mocks), through msw's
//    `getResponse`. An operation the scenario does not know answers 501 and is recorded as unmocked, so a test can
//    prove nothing fell through to a default.
//  - `/api/v1/events` is a real, long Server-Sent Events response whose frames the test writes byte for byte
//    (`emit`), including the awkward ones (`id: N` with a space, a heartbeat, a `resync` without an id).
//  - A write is refused 403 `csrf` unless it carries exactly one Origin equal to this origin and the double-submit token
//    the hub's CsrfLayer wants, so the client's Origin and CSRF behaviour is proved in a real browser.
//  - `/__harness/*` is the control channel for the test process (never called by the page).
//
// Run alone: `node e2e/harness/serve.ts --dist dist --scenario admin --port 5174` (`just web-mock`). Node strips the types
// itself (22.18 and later), so there is no build step and no loader.
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import http, { type IncomingMessage, type ServerResponse } from "node:http";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { getResponse, type RequestHandler } from "msw";
import { DEFAULT_SCENARIO, scenarios } from "../../src/mocks/scenarios.ts";
import { problemBody, toHandlers } from "../../src/mocks/scenario.ts";

const WEB_ROOT = fileURLToPath(new URL("../../", import.meta.url));
const MAX_BODY = 1024 * 1024;
const MAX_STREAMS = 5;
const MAX_LOG = 5000;
const HOP_BY_HOP = new Set(["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailer", "transfer-encoding", "upgrade"]);

export type HarnessRequest = {
  method: string;
  path: string;
  status: number;
  origin?: string | null;
  csrfHeader?: string | null;
  idempotencyKey?: string | null;
  accept?: string | null;
  csrf?: "ok" | "refused" | "n/a";
  body?: string | undefined;
  unmocked?: boolean;
  kind?: "stream";
  lastEventId?: string | null;
};

export type Harness = {
  /** The origin the browser visits, e.g. http://127.0.0.1:40123. */
  readonly url: string;
  readonly scenario: string;
  setScenario(id: string): void;
  requests(): HarnessRequest[];
  clearRequests(): void;
  streamCount(): number;
  /** Write raw text to every open event stream. */
  emit(text: string): void;
  closeStreams(): void;
  close(): Promise<void>;
};

export type HarnessOptions = {
  dist?: string;
  scenario?: string;
  port?: number;
  heartbeatMs?: number;
  fixtureEnv?: Record<string, string>;
};

/** Where the Rust fixture binary is, or an error that says how to build it (the harness never guesses a fallback). */
export function fixtureBinary(): string {
  const fromEnv = process.env["LK_WEB_FIXTURE"];
  const target = process.env["CARGO_TARGET_DIR"] ?? resolve(WEB_ROOT, "..", "target");
  const candidates = [fromEnv, join(target, "debug", "examples", "web_fixture"), join(target, "release", "examples", "web_fixture")].filter((c): c is string => c !== undefined && c !== "");
  const found = candidates.find((c) => existsSync(c));
  if (!found) throw new Error(`the Rust fixture is not built: run \`cargo build -p hub --example web_fixture\` (looked in ${candidates.join(", ")})`);
  return found;
}

function startFixture({ dist, bin, env }: { dist: string; bin: string; env: Record<string, string> }): Promise<{ child: ChildProcess; url: string }> {
  const child = spawn(bin, [], { env: { ...process.env, LK_WEB_DIST: resolve(WEB_ROOT, dist), ...env }, stdio: ["ignore", "pipe", "pipe"] });
  return new Promise((resolvePromise, reject) => {
    let out = "";
    let err = "";
    const timer = setTimeout(() => {
      reject(new Error(`the fixture did not announce a port in 10 s: ${err.slice(0, 300)}`));
    }, 10_000);
    child.stdout.on("data", (chunk: Buffer) => {
      out += chunk.toString();
      const match = /listening (http:\/\/127\.0\.0\.1:\d+)/.exec(out);
      if (match?.[1]) {
        clearTimeout(timer);
        resolvePromise({ child, url: match[1] });
      }
    });
    child.stderr.on("data", (chunk: Buffer) => {
      err += chunk.toString();
    });
    child.on("exit", (code) => {
      clearTimeout(timer);
      reject(new Error(`the fixture exited with ${String(code)} before it was ready: ${err.slice(0, 300)}`));
    });
  });
}

type Reply = { status: number; body: string | Buffer; type?: string; headers?: Record<string, string | string[]> };

const problem = (status: number, code: string, title: string): Reply => ({ status, body: JSON.stringify(problemBody(status, code, title)), type: "application/problem+json" });

function cookieValues(header: string | undefined, name: string): string[] {
  const found: string[] = [];
  for (const part of (header ?? "").split(";")) {
    const eq = part.indexOf("=");
    if (eq > 0 && part.slice(0, eq).trim() === name) found.push(part.slice(eq + 1).trim());
  }
  return found;
}

type Stream = { res: ServerResponse; timer: ReturnType<typeof setInterval> };

export async function startHarness(options: HarnessOptions = {}): Promise<Harness> {
  const { dist = "dist", port = 0, heartbeatMs = 15_000, fixtureEnv = {} } = options;
  let scenarioId = options.scenario ?? DEFAULT_SCENARIO;
  const fixture = await startFixture({ dist, bin: fixtureBinary(), env: fixtureEnv });
  const upstream = new URL(fixture.url);
  const log: HarnessRequest[] = [];
  const streams = new Set<Stream>();
  const scenarioOf = (id: string) => {
    const scenario = scenarios[id];
    if (!scenario) throw new Error(`no scenario ${id}`);
    return scenario;
  };
  let handlers: RequestHandler[] = toHandlers(scenarioOf(scenarioId));

  const setScenario = (id: string): void => {
    handlers = toHandlers(scenarioOf(id));
    scenarioId = id;
  };

  const record = (entry: HarnessRequest): void => {
    log.push(entry);
    if (log.length > MAX_LOG) log.shift();
  };

  async function readBody(req: IncomingMessage): Promise<Buffer> {
    const chunks: Buffer[] = [];
    let size = 0;
    for await (const chunk of req) {
      const buffer = chunk as Buffer;
      size += buffer.length;
      if (size > MAX_BODY) throw new Error("body too large");
      chunks.push(buffer);
    }
    return Buffer.concat(chunks);
  }

  function send(res: ServerResponse, { status, body, type = "application/json", headers = {} }: Reply): void {
    res.writeHead(status, { "content-type": type, "cache-control": "no-store", "x-content-type-options": "nosniff", "x-request-id": "req-harness-0001", ...headers });
    res.end(body);
  }

  function csrfVerdict(req: IncomingMessage, origin: string): "ok" | "refused" | "n/a" {
    const method = req.method ?? "GET";
    if (["GET", "HEAD", "OPTIONS"].includes(method)) return "n/a";
    const cookies = cookieValues(req.headers.cookie, "__Host-lk_csrf");
    if (cookies.length === 0) return "n/a"; // no live session: nothing to protect, the route guard answers
    const origins = [req.headers.origin].flat().filter((o): o is string => o !== undefined);
    const tokens = [req.headers["x-csrf-token"]].flat().filter((t): t is string => t !== undefined);
    if (origins.length !== 1 || origins[0] !== origin) return "refused";
    if (cookies.length !== 1 || tokens.length !== 1 || tokens[0] !== cookies[0]) return "refused";
    return "ok";
  }

  function openStream(req: IncomingMessage, res: ServerResponse): void {
    const path = req.url ?? "";
    if (streams.size >= MAX_STREAMS) {
      record({ method: "GET", path, status: 429, kind: "stream" });
      send(res, { ...problem(429, "too_many_requests", "Too many streams"), headers: { "retry-after": "3" } });
      return;
    }
    record({ method: "GET", path, status: 200, kind: "stream", lastEventId: (req.headers["last-event-id"] as string | undefined) ?? null, accept: req.headers.accept ?? null });
    res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store", "x-accel-buffering": "no", "x-content-type-options": "nosniff" });
    res.write("retry: 3000\n\n");
    const timer = setInterval(() => res.write(": heartbeat\n\n"), heartbeatMs);
    const stream: Stream = { res, timer };
    streams.add(stream);
    req.on("close", () => {
      clearInterval(timer);
      streams.delete(stream);
    });
  }

  const closeAll = (): void => {
    for (const stream of [...streams]) {
      clearInterval(stream.timer);
      stream.res.end();
      streams.delete(stream);
    }
  };

  async function control(req: IncomingMessage, res: ServerResponse, url: URL): Promise<void> {
    const json = (value: unknown, status = 200): void => send(res, { status, body: JSON.stringify(value) });
    if (req.method === "GET" && url.pathname === "/__harness/state") {
      json({ scenario: scenarioId, streams: streams.size });
      return;
    }
    if (req.method === "GET" && url.pathname === "/__harness/requests") {
      json(log);
      return;
    }
    if (req.method === "POST") {
      const text = (await readBody(req)).toString("utf8");
      const data = (text ? JSON.parse(text) : {}) as { id?: string; text?: string };
      if (url.pathname === "/__harness/scenario") {
        setScenario(String(data.id));
        json({ scenario: scenarioId });
        return;
      }
      if (url.pathname === "/__harness/requests/clear") {
        log.length = 0;
        json({});
        return;
      }
      if (url.pathname === "/__harness/events/emit") {
        for (const { res: out } of streams) out.write(String(data.text));
        json({ streams: streams.size });
        return;
      }
      if (url.pathname === "/__harness/events/close") {
        closeAll();
        json({});
        return;
      }
    }
    json({ error: "unknown control request" }, 404);
    return;
  }

  async function api(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const method = req.method ?? "GET";
    const path = req.url ?? "/";
    const origin = `http://${req.headers.host ?? ""}`;
    const body = ["GET", "HEAD"].includes(method) ? undefined : await readBody(req);
    const verdict = csrfVerdict(req, origin);
    const entry: Omit<HarnessRequest, "status"> = {
      method,
      path,
      origin: req.headers.origin ?? null,
      csrfHeader: req.headers["x-csrf-token"] ? "present" : null,
      idempotencyKey: (req.headers["idempotency-key"] as string | undefined) ?? null,
      accept: req.headers.accept ?? null,
      csrf: verdict,
      body: body && body.length <= 4096 ? body.toString("utf8") : undefined,
    };
    if (verdict === "refused") {
      record({ ...entry, status: 403 });
      send(res, problem(403, "csrf", "Forbidden"));
      return;
    }
    const headerPairs = Object.entries(req.headers).flatMap(([k, v]): Array<[string, string]> => (typeof v === "string" ? [[k, v]] : []));
    const request = new Request(`${origin}${path}`, { method, headers: headerPairs, ...(body && body.length > 0 ? { body: new Uint8Array(body) } : {}) });
    const response = await getResponse(handlers, request);
    if (!response) {
      record({ ...entry, status: 501, unmocked: true });
      send(res, problem(501, "mock_not_defined", "No mock for this operation in this scenario"));
      return;
    }
    record({ ...entry, status: response.status });
    const headers: Record<string, string> = {};
    for (const [name, value] of response.headers) if (name !== "set-cookie") headers[name] = value;
    const cookies = response.headers.getSetCookie();
    res.writeHead(response.status, { "cache-control": "no-store", "x-content-type-options": "nosniff", "x-request-id": "req-harness-0001", ...headers, ...(cookies.length > 0 ? { "set-cookie": cookies } : {}) });
    res.end(Buffer.from(await response.arrayBuffer()));
  }

  function proxy(req: IncomingMessage, res: ServerResponse): void {
    const headers = { ...req.headers, host: upstream.host };
    const upstreamReq = http.request({ host: upstream.hostname, port: upstream.port, method: req.method, path: req.url, headers }, (upstreamRes) => {
      const out: Record<string, string | string[]> = {};
      for (const [name, value] of Object.entries(upstreamRes.headers)) if (!HOP_BY_HOP.has(name) && value !== undefined) out[name] = value;
      res.writeHead(upstreamRes.statusCode ?? 502, out);
      upstreamRes.pipe(res);
    });
    upstreamReq.on("error", () => {
      send(res, problem(502, "bad_gateway", "The fixture is not reachable"));
    });
    req.pipe(upstreamReq);
  }

  const server = http.createServer((req, res) => {
    const url = new URL(req.url ?? "/", "http://harness.invalid");
    const isApi = url.pathname.startsWith("/api/v1/") || url.pathname.startsWith("/auth/");
    const handle = async (): Promise<void> => {
      if (url.pathname.startsWith("/__harness/")) await control(req, res, url);
      else if (url.pathname === "/api/v1/events" && req.method === "GET") openStream(req, res);
      else if (isApi) await api(req, res);
      else proxy(req, res);
    };
    handle().catch(() => {
      if (!res.headersSent) send(res, problem(500, "internal", "Harness error"));
      else res.end();
    });
  });
  await new Promise<void>((resolvePromise) => server.listen(port, "127.0.0.1", resolvePromise));
  const address = server.address();
  const origin = `http://127.0.0.1:${typeof address === "object" && address ? address.port : port}`;

  return {
    url: origin,
    get scenario() {
      return scenarioId;
    },
    setScenario,
    requests: () => [...log],
    clearRequests: () => {
      log.length = 0;
    },
    streamCount: () => streams.size,
    emit(text: string) {
      for (const { res } of streams) res.write(text);
    },
    closeStreams: closeAll,
    async close() {
      for (const stream of streams) clearInterval(stream.timer);
      streams.clear();
      server.closeAllConnections();
      await new Promise<void>((resolvePromise) => server.close(() => resolvePromise()));
      fixture.child.removeAllListeners("exit");
      fixture.child.kill("SIGTERM");
    },
  };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const arg = (name: string, fallback: string): string => {
    const i = process.argv.indexOf(`--${name}`);
    return i >= 0 ? (process.argv[i + 1] ?? fallback) : fallback;
  };
  const harness = await startHarness({ dist: arg("dist", "dist"), scenario: arg("scenario", DEFAULT_SCENARIO), port: Number(arg("port", "0")) });
  process.stdout.write(`harness listening ${harness.url} (scenario ${harness.scenario}); scenarios: ${Object.keys(scenarios).join(", ")}\n`);
  const stop = (): void => {
    void harness.close().finally(() => process.exit(0));
  };
  process.on("SIGINT", stop);
  process.on("SIGTERM", stop);
}
