// Scenarios made for a journey out of the design scenarios (src/mocks/design/*), for the answers a journey needs that no design
// frame draws: a list that has changed after a write, a second answer to the same call, a comparison whose hunks agree with the
// text of the file.
//
// A scenario is data (src/mocks/scenario.ts), and the harness looks one up by id in the same `scenarios` record that this file
// adds to, in the same process as the test. So a journey derives its scenario from a design one, changes the responses it cares
// about, and tells the harness to serve it, now or in the middle of the journey (`harness.setScenario(id)`): the page is not
// told, and asks again, as it would of a hub whose data had changed. Nothing here is a hub of its own and nothing is registered
// outside the test process.
import { scenarios } from "../../src/mocks/scenarios.ts";
import { resolveBody, type MockContext, type MockResponse, type Scenario } from "../../src/mocks/scenario.ts";

export type { MockContext, MockResponse };

let counter = 0;

/** The responses of a scenario that is already known, or an error that names it. */
export function responsesOf(id: string): readonly MockResponse[] {
  const scenario = scenarios[id];
  if (scenario === undefined) throw new Error(`no scenario ${id}`);
  return scenario.responses;
}

const same = (a: MockResponse, b: Pick<MockResponse, "method" | "path">): boolean => a.method === b.method && a.path === b.path;

/**
 * Register a scenario made from `base`, where each of `changes` replaces the response for its method and path (or is added
 * when the base has none), and return its id. Ids are unique, so two journeys of one process never share a scenario.
 */
export function derive(base: string, changes: readonly MockResponse[], label = "journey"): string {
  const id = `${label}-${String((counter += 1))}`;
  const kept = responsesOf(base).filter((response) => !changes.some((change) => same(response, change)));
  const scenario: Scenario = { id, description: `${base} with ${String(changes.length)} changed response(s)`, responses: [...kept, ...changes] };
  scenarios[id] = scenario;
  return id;
}

/** The response of `base` for a method and path, so that a journey can change one thing in it. */
export function responseOf(base: string, method: MockResponse["method"], path: string): MockResponse {
  const found = responsesOf(base).find((response) => same(response, { method, path }));
  if (found === undefined) throw new Error(`${base} has no ${method} ${path}`);
  return found;
}

/** The body of `response` for a request, as the harness would build it. */
export function bodyOf(response: MockResponse, context: Partial<MockContext> = {}): unknown {
  return resolveBody(response, { params: {}, query: new URLSearchParams(), headers: new Headers(), body: undefined, ...context });
}
