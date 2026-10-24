// Problems (RFC 9457) as the client sees them (S21: the title and the request id are shown, nothing else leaks).
//
// `type` is `urn:lanekeeper:problem:<code>` on every problem the hub sends and is the discriminator (the contract's
// description of `Problem.type`). The hub's layers spell codes both as snake_case and kebab-case, so `kind` is the
// snake_case form and `rawKind` is what came on the wire. Only the response is ever read: an ApiProblem holds no part of
// the request, so a request body (a token, a file) cannot reach a screen, a log or an error report through it.

export const PROBLEM_URN_PREFIX = "urn:lanekeeper:problem:";

/** The most of an error body that is read. A problem is a few hundred bytes. */
export const MAX_PROBLEM_BYTES = 64 * 1024;

const KIND = /^[a-z][a-z0-9_-]{0,63}$/;
const REQUEST_ID = /^[A-Za-z0-9._-]{1,128}$/;
const MAX_RETRY_AFTER_SECONDS = 3600;

export type ApiProblemInit = {
  status: number;
  kind: string;
  rawKind?: string;
  title: string;
  requestId?: string | undefined;
  detail?: string | undefined;
  retryAfterSeconds?: number | undefined;
  /** The parsed problem object, as sent. Extension members (`current`, `until`, `reason`, `issues`, `link`, `base`, `tenant`). */
  body?: Readonly<Record<string, unknown>>;
};

export class ApiProblem extends Error {
  override readonly name = "ApiProblem";
  /** HTTP status; 0 when there was no response (`kind` is then `network`, `timeout` or `aborted`). */
  readonly status: number;
  /** The stable code, snake_case. `unknown` for a body that is not a problem. */
  readonly kind: string;
  /** The code as the hub spelled it. */
  readonly rawKind: string;
  readonly title: string;
  readonly requestId: string | undefined;
  /** What the server wrote in `detail`. Screens show it only for codes they know to be safe to show. */
  readonly detail: string | undefined;
  readonly retryAfterSeconds: number | undefined;
  readonly body: Readonly<Record<string, unknown>>;

  constructor(init: ApiProblemInit) {
    super(init.title);
    this.status = init.status;
    this.kind = init.kind;
    this.rawKind = init.rawKind ?? init.kind;
    this.title = init.title;
    this.requestId = init.requestId;
    this.detail = init.detail;
    this.retryAfterSeconds = init.retryAfterSeconds;
    this.body = init.body ?? {};
  }
}

/** `access-pending` and `access_pending` are one kind. */
export function normaliseKind(raw: string): string {
  return raw.replaceAll("-", "_");
}

function kindOf(type: unknown, code: unknown): string | undefined {
  if (typeof type === "string" && type.startsWith(PROBLEM_URN_PREFIX)) {
    const suffix = type.slice(PROBLEM_URN_PREFIX.length);
    if (KIND.test(suffix)) return suffix;
  }
  if (typeof code === "string" && KIND.test(code)) return code;
  return undefined;
}

function retryAfter(headers: Headers): number | undefined {
  const value = headers.get("retry-after");
  if (value === null || !/^\d{1,9}$/.test(value.trim())) return undefined;
  return Math.min(Number(value.trim()), MAX_RETRY_AFTER_SECONDS);
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : undefined;
}

/** Build a problem from the text of an error body. Never throws: whatever the body is, the result is a problem. */
export function parseProblemBody(text: string, status: number, headers: Headers): ApiProblem {
  const fallback = (): ApiProblem =>
    new ApiProblem({ status, kind: "unknown", title: `HTTP ${status}`, requestId: headerRequestId(headers), retryAfterSeconds: retryAfter(headers) });
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    return fallback();
  }
  const body = asRecord(parsed);
  if (!body) return fallback();
  const rawKind = kindOf(body["type"], body["code"]);
  if (rawKind === undefined) return fallback();
  const title = typeof body["title"] === "string" && body["title"].trim() !== "" ? body["title"].slice(0, 200) : `HTTP ${status}`;
  const fromBody = typeof body["request_id"] === "string" && REQUEST_ID.test(body["request_id"]) ? body["request_id"] : undefined;
  return new ApiProblem({
    status,
    kind: normaliseKind(rawKind),
    rawKind,
    title,
    requestId: fromBody ?? headerRequestId(headers),
    detail: typeof body["detail"] === "string" ? body["detail"].slice(0, 2000) : undefined,
    retryAfterSeconds: retryAfter(headers),
    body,
  });
}

function headerRequestId(headers: Headers): string | undefined {
  const value = headers.get("x-request-id");
  return value !== null && REQUEST_ID.test(value) ? value : undefined;
}

/** Read a body as text, at most `maxBytes` of it. The rest is cancelled, never buffered. */
export async function readBounded(response: Response, maxBytes: number): Promise<{ text: string; truncated: boolean }> {
  const body = response.body;
  if (!body) return { text: "", truncated: false };
  const reader = body.getReader();
  const decoder = new TextDecoder("utf-8", { fatal: false });
  let text = "";
  let received = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) return { text: text + decoder.decode(), truncated: false };
    const room = maxBytes - received;
    if (value.byteLength > room) {
      text += decoder.decode(value.subarray(0, room), { stream: true });
      await reader.cancel();
      return { text, truncated: true };
    }
    received += value.byteLength;
    text += decoder.decode(value, { stream: true });
  }
}

/** The problem for a non-2xx response. A body over the cap is not parsed: it is not a problem the hub made. */
export async function problemFromResponse(response: Response): Promise<ApiProblem> {
  let text = "";
  try {
    const read = await readBounded(response, MAX_PROBLEM_BYTES);
    if (!read.truncated) text = read.text;
  } catch {
    // A body that cannot be read is an empty body.
  }
  return parseProblemBody(text, response.status, response.headers);
}
