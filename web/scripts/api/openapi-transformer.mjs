// Orval input transformer (run by `pnpm gen:api`). It only edits the in-memory copy that orval reads; api/openapi.yaml
// is never written.
//
// The contract declares three headers as parameters of operations: X-CSRF-Token and Idempotency-Key on every write, and
// If-None-Match on the content reads. The generated hooks must not make a caller supply them: the one mutator
// (src/api/client.ts) sets the CSRF header from the cookie, creates a fresh Idempotency-Key for each user attempt, and
// handles If-None-Match through its bounded ETag cache. Leaving them in the signature would make a screen able to forget
// (or fake) them. The test api/client.csrf.test.ts reads the real YAML and proves that every write declares them, so
// dropping them here cannot hide an operation that has none.
const STRIPPED = new Set(["CsrfToken", "IdempotencyKey", "IfNoneMatch"]);

// The Server-Sent Events operation is not generated at all. A generated hook for it would be a `useQuery` that calls the
// mutator and waits for one JSON body from a response that never ends, and the schema of its `text/event-stream` media
// type (`DomainEvent`, a oneOf of 17 payloads) is not one body. The stream is read by src/live/sse-client.ts (T3), a
// `fetch` stream with its own parser, timeouts and backoff, against the path below. `DomainEvent` and its 17 payload
// schemas stay in components and are generated as types; src/api/constants.ts holds the path and a test checks it
// against the YAML.
const STREAM_PATH = "/api/v1/events";

/**
 * @param {{ info?: { description?: string }; paths?: Record<string, Record<string, { parameters?: Array<{ $ref?: string }> } | undefined>> }} spec
 */
export default function transformer(spec) {
  const { [STREAM_PATH]: _stream, ...paths } = spec.paths ?? {};
  spec.paths = paths;
  // Orval copies `info.description` (the whole contract preamble, several KB) into the header of every generated file.
  spec.info = { ...spec.info, description: "Lanekeeper REST API. The contract is api/openapi.yaml; this client is generated from it." };
  for (const item of Object.values(spec.paths)) {
    for (const operation of Object.values(item)) {
      if (!operation || typeof operation !== "object" || !Array.isArray(operation.parameters)) continue;
      operation.parameters = operation.parameters.filter((p) => {
        const name = typeof p.$ref === "string" ? p.$ref.split("/").pop() : undefined;
        return !(name && STRIPPED.has(name));
      });
    }
  }
  return spec;
}
