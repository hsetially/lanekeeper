// The one mutator: every request to the hub goes through apiFetch (the generated client calls nothing else, and the lint
// bans `fetch('/api` anywhere else).
//
// What it guarantees, for every caller:
//  - it only talks to the hub's own paths (S12: connect-src 'self' says the same; this refuses before a request exists);
//  - every request that is not a GET carries X-CSRF-Token from the cookie (S3) and a fresh Idempotency-Key (one per user
//    attempt; reused only when this function retries the same attempt after a network error);
//  - the fetch settings make the hub see the real `Origin` (mode 'cors', not 'same-origin': with Referrer-Policy
//    no-referrer a non-cors request sends `Origin: null` and the CSRF layer refuses it) and never use a cached answer;
//  - every request has a timeout (rule 5), bodies have a size cap, and an error body is read at most 64 KiB of;
//  - a failure is an ApiProblem that holds nothing of the request (S21).
//
// It also keeps the bounded ETag cache (src/api/etag.ts) and tells the rest of the app about a 401 and about a user with
// no role yet or a disabled one (src/api/session-events.ts).

import { readCsrfToken, CSRF_HEADER } from "@/lib/csrf";
import { API_PREFIX, AUTH_PREFIX } from "./constants";
import { EtagCache } from "./etag";
import { ApiProblem, problemFromResponse, readBounded } from "./problem";
import { retryDelayMs } from "./retry";
import { emitSessionSignal } from "./session-events";

/** The longest a request may take before it is abandoned. */
export const DEFAULT_TIMEOUT_MS = 30_000;
/** The most of a success body that is read. The hub's own largest file is 32 MiB. */
export const MAX_BODY_BYTES = 64 * 1024 * 1024;
const MAX_URL_LENGTH = 2048;
const IDEMPOTENCY_KEY = "Idempotency-Key";

/**
 * What a failed query or mutation throws, whatever the schema of the error response says. Orval uses this name for the
 * error type of every generated hook (a mutator that exports `ErrorType` gets it), so `error` is an ApiProblem and not a
 * guess at the body.
 */
export type ErrorType<_Schemas> = ApiProblem;
/** The request body type orval passes through; the app sends exactly what the generated function builds. */
export type BodyType<T> = T;

export type ApiRequestInit = RequestInit & {
  /** Abandon the request after this many milliseconds. Default 30 s. */
  timeoutMs?: number;
  /** One key for one attempt. A caller that retries by hand passes the same key again. */
  idempotencyKey?: string;
};

export const apiCaches = {
  etags: new EtagCache(),
  /** Empty every cache that holds an API answer: sign-out, a 401, a change of user. */
  clear(): void {
    this.etags.clear();
  },
};

// A retry of these is not safe even with the same key: a second reveal is a second audit event, and the others carry a
// secret that should cross the wire once. Every one is confirmed against api/openapi.yaml by client.test.ts.
export const NEVER_AUTO_RETRY_PATHS: readonly RegExp[] = [
  /^\/api\/v1\/swimlanes\/[^/]+\/files\/reveal$/,
  /^\/api\/v1\/me\/github-token$/,
  /^\/api\/v1\/me\/mcp-tokens(\/[^/]+)?$/,
  /^\/api\/v1\/admin\/agents\/[^/]+\/join-token$/,
  /^\/api\/v1\/admin\/notifications\/[^/]+$/,
];

// Answers that carry a secret are never kept for revalidation, whatever the server says about their ETag.
const NEVER_CACHE_PATHS: readonly RegExp[] = [/^\/api\/v1\/me\/github-token$/, /^\/api\/v1\/me\/mcp-tokens(\/[^/]+)?$/, /\/join-token$/, /\/files\/reveal$/];

const UNSAFE_SEGMENT = /%2f|%5c|%00/i;
const SEGMENT_CHARS = /^[A-Za-z0-9._~!$&'()*+,;=:@%-]+$/;

/**
 * The generated client interpolates path parameters without encoding them, so a parameter that holds `/` or `..` would
 * point the request at another operation. This is the last line: a path of the hub's own, made of plain segments.
 */
function assertOwnPath(url: string): string {
  const fail = (): never => {
    throw new Error("apiFetch sends only to the hub's own paths (/api/v1/ and /auth/)");
  };
  if (url.length > MAX_URL_LENGTH || url.includes("#")) return fail();
  const queryAt = url.indexOf("?");
  const path = queryAt < 0 ? url : url.slice(0, queryAt);
  if (!(path.startsWith(API_PREFIX) || path.startsWith(AUTH_PREFIX))) return fail();
  for (const segment of path.slice(1).split("/")) {
    if (segment === "" || !SEGMENT_CHARS.test(segment) || UNSAFE_SEGMENT.test(segment)) return fail();
    const decoded = segment.replace(/%2e/gi, ".");
    if (decoded === "." || decoded === "..") return fail();
  }
  return path;
}

function newKey(): string {
  return globalThis.crypto.randomUUID();
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

type Attempt = { signal: AbortSignal; cleanup: () => void; timedOut: () => boolean; callerAborted: () => boolean };

/** One abort signal for the caller's and the timeout's reasons, and a way to tell them apart. */
function attemptSignal(caller: AbortSignal | null | undefined, timeoutMs: number): Attempt {
  const controller = new AbortController();
  let timedOut = false;
  let callerAborted = false;
  const timer = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, timeoutMs);
  const onCaller = (): void => {
    callerAborted = true;
    controller.abort();
  };
  if (caller) {
    if (caller.aborted) onCaller();
    else caller.addEventListener("abort", onCaller, { once: true });
  }
  return {
    signal: controller.signal,
    cleanup: () => {
      clearTimeout(timer);
      caller?.removeEventListener("abort", onCaller);
    },
    timedOut: () => timedOut,
    callerAborted: () => callerAborted,
  };
}

function isJson(contentType: string | null): boolean {
  const media = (contentType ?? "").split(";")[0]?.trim().toLowerCase() ?? "";
  return media === "application/json" || media.endsWith("+json");
}

function tooLarge(response: Response): boolean {
  const length = Number(response.headers.get("content-length"));
  return Number.isFinite(length) && length > MAX_BODY_BYTES;
}

const badResponse = (status: number): ApiProblem => new ApiProblem({ status, kind: "bad_response", title: "The server's answer could not be read" });
const bodyTooLarge = (status: number): ApiProblem => new ApiProblem({ status, kind: "response_too_large", title: "The server's answer is too large" });

async function readBlob(response: Response): Promise<Blob> {
  if (tooLarge(response)) throw bodyTooLarge(response.status);
  const body = response.body;
  if (!body) return new Blob([]);
  const reader = body.getReader();
  const parts: Uint8Array<ArrayBuffer>[] = [];
  let received = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    received += value.byteLength;
    if (received > MAX_BODY_BYTES) {
      await reader.cancel();
      throw bodyTooLarge(response.status);
    }
    parts.push(new Uint8Array(value));
  }
  return new Blob(parts, { type: response.headers.get("content-type") ?? "" });
}

async function readText(response: Response): Promise<string> {
  if (tooLarge(response)) throw bodyTooLarge(response.status);
  const { text, truncated } = await readBounded(response, MAX_BODY_BYTES);
  if (truncated) throw bodyTooLarge(response.status);
  return text;
}

function parseJson<T>(text: string, status: number): T {
  try {
    return JSON.parse(text) as T;
  } catch {
    throw badResponse(status);
  }
}

function signalFor(problem: ApiProblem): void {
  if (problem.status === 401) {
    apiCaches.clear();
    emitSessionSignal({ kind: "unauthenticated" });
  } else if (problem.status === 403 && problem.kind === "access_pending") {
    emitSessionSignal({ kind: "access_pending" });
  } else if (problem.status === 403 && problem.kind === "access_disabled") {
    emitSessionSignal({ kind: "access_disabled" });
  }
}

export async function apiFetch<T>(url: string, init: ApiRequestInit = {}): Promise<T> {
  const path = assertOwnPath(url);
  const { timeoutMs = DEFAULT_TIMEOUT_MS, idempotencyKey, signal: callerSignal, ...fetchInit } = init;
  const method = (fetchInit.method ?? "GET").toUpperCase();
  const isRead = method === "GET" || method === "HEAD";

  const headers = new Headers(fetchInit.headers);
  if (!headers.has("accept")) headers.set("accept", "application/json, application/problem+json;q=0.9, */*;q=0.1");
  // A caller cannot name these two: the cookie and a fresh key are the only sources.
  headers.delete(CSRF_HEADER);
  headers.delete(IDEMPOTENCY_KEY);
  if (!isRead) {
    const token = readCsrfToken();
    if (token !== undefined) headers.set(CSRF_HEADER, token);
    headers.set(IDEMPOTENCY_KEY, idempotencyKey ?? newKey());
  }
  const cacheable = method === "GET" && !NEVER_CACHE_PATHS.some((re) => re.test(path));
  const kept = cacheable ? apiCaches.etags.get(url) : undefined;
  if (kept && !headers.has("if-none-match")) headers.set("if-none-match", kept.etag);

  const retryable = !isRead && !NEVER_AUTO_RETRY_PATHS.some((re) => re.test(path));
  let networkRetries = retryable ? 1 : 0;

  for (;;) {
    const attempt = attemptSignal(callerSignal, timeoutMs);
    let response: Response;
    try {
      response = await globalThis.fetch(url, {
        ...fetchInit,
        method,
        headers,
        credentials: "same-origin",
        mode: "cors",
        cache: "no-store",
        redirect: "error",
        referrerPolicy: "no-referrer",
        signal: attempt.signal,
      });
    } catch {
      attempt.cleanup();
      if (attempt.callerAborted()) throw new ApiProblem({ status: 0, kind: "aborted", title: "The request was cancelled" });
      if (attempt.timedOut()) throw new ApiProblem({ status: 0, kind: "timeout", title: "The server took too long to answer" });
      if (networkRetries > 0) {
        networkRetries -= 1;
        await sleep(150 + retryDelayMs(1));
        continue;
      }
      throw new ApiProblem({ status: 0, kind: "network", title: "Could not reach the server" });
    }
    try {
      return await interpret<T>(response, url, cacheable, kept);
    } catch (error) {
      if (attempt.callerAborted() && !(error instanceof ApiProblem)) throw new ApiProblem({ status: 0, kind: "aborted", title: "The request was cancelled" });
      if (attempt.timedOut() && !(error instanceof ApiProblem)) throw new ApiProblem({ status: 0, kind: "timeout", title: "The server took too long to answer" });
      throw error;
    } finally {
      attempt.cleanup();
    }
  }
}

async function interpret<T>(response: Response, url: string, cacheable: boolean, kept: { etag: string; text: string } | undefined): Promise<T> {
  if (response.status === 304) {
    if (!kept) throw badResponse(304);
    return parseJson<T>(kept.text, 304);
  }
  if (!response.ok) {
    const problem = await problemFromResponse(response);
    signalFor(problem);
    throw problem;
  }
  if (response.status === 204 || response.status === 205) return undefined as T;
  const contentType = response.headers.get("content-type");
  if (isJson(contentType)) {
    const text = await readText(response);
    const etag = response.headers.get("etag");
    if (cacheable && etag !== null && etag !== "") apiCaches.etags.set(url, etag, text);
    return parseJson<T>(text, response.status);
  }
  if ((contentType ?? "").toLowerCase().startsWith("text/")) return (await readText(response)) as T;
  return (await readBlob(response)) as T;
}

