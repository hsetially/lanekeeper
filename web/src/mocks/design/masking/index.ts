// The design-parity scenarios of the masking pieces (T5 slice C): the masked value of the File page and its Reveal, in the states the
// File view board draws (frames 1d, 1e and 1f) and the answers the hub can give (a refusal for a Viewer, the rate limit, an audit log
// that is down, a value that is no longer there). The page is the File page of the file scenarios (S3) with the answer of `revealValue`
// added; the loader (../index.ts) picks up what this file exports. A scenario id is `design-masking-<name>`.
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension.
import { problemBody, type MockResponse, type Scenario } from "../../scenario.ts";
import { fileScenario } from "../file/build.ts";
import { coreFile, CORE_PATH } from "../file/data.ts";
import { multiMaskFile, REVEAL_PATH, REVEALED_VALUE, SECRET_SETTING } from "./data.ts";

const REVEAL_SAMPLE = { params: { id: "sitb" }, body: { path: CORE_PATH, setting: SECRET_SETTING, reason: "Teller login fails on sit7; checking the secret matches the vault entry (OPS-2291)." } };
const NO_STORE = { "cache-control": "no-store" };

function withReveal(scenario: Scenario, response: Omit<MockResponse, "method" | "path" | "sample">): Scenario {
  return { ...scenario, responses: [...scenario.responses, { method: "POST", path: REVEAL_PATH, sample: REVEAL_SAMPLE, ...response }] };
}

const core = coreFile({ masked: true });

export const scenarios: readonly Scenario[] = [
  withReveal(fileScenario({ id: "design-masking-reveal", description: "File, masked value with Reveal (frames 1d, 1e, 1f): the hub answers a reveal with the value.", file: core }), { headers: NO_STORE, body: { value: REVEALED_VALUE } }),
  withReveal(fileScenario({ id: "design-masking-viewer", description: "File, masked value for a Viewer: Reveal is disabled and says it needs Editor; the hub would answer 403.", role: "viewer", file: core }), {
    status: 403,
    headers: NO_STORE,
    body: problemBody(403, "forbidden", "Forbidden"),
  }),
  withReveal(fileScenario({ id: "design-masking-throttled", description: "File, Reveal over the limit of 30 a minute: 429 with Retry-After.", file: core }), {
    status: 429,
    headers: { ...NO_STORE, "retry-after": "30" },
    body: problemBody(429, "too_many_requests", "Too many requests"),
  }),
  withReveal(fileScenario({ id: "design-masking-unavailable", description: "File, Reveal while the audit log is down: 503 with Retry-After, nothing is shown.", file: core }), {
    status: 503,
    headers: { ...NO_STORE, "retry-after": "5" },
    body: problemBody(503, "service_unavailable", "Service unavailable"),
  }),
  withReveal(fileScenario({ id: "design-masking-gone", description: "File, Reveal of a value that is no longer hidden or a setting that is gone: 404, nothing to reveal.", file: core }), {
    status: 404,
    headers: NO_STORE,
    body: problemBody(404, "not_found", "Not found"),
  }),
  withReveal(fileScenario({ id: "design-masking-multi", description: "File, three masks: one can be revealed, one names no setting (token only), one is recognised by its form.", file: multiMaskFile() }), {
    headers: NO_STORE,
    body: { value: REVEALED_VALUE },
  }),
];
