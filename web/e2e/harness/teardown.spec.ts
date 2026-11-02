// A test that ends while one of its route handlers is still waiting on the harness must not fail because of it.
//
// What happened on main's last `web-e2e` run (e5ed17e): the handler of `attribution.spec.ts` called `route.fetch()` for the list of
// drifted files, the page asked for the list a second time just before the test body ended, and the fixtures tore down with the handler
// inside `route.fetch()` or `response.json()`. The harness closes every connection first (`closeAllConnections`), so the request was
// reset (`route.fetch: read ECONNRESET`); or the context was already closed, so the answer was disposed (`apiResponse.json: Response has
// been disposed`). The handler's throw was reported as the test's failure, about one run in fifty, with retries off.
//
// Here the race is made certain instead of rare: the handler waits on `/api/v1/events`, a Server-Sent Events response that never ends,
// so `route.fetch()` can only finish when the harness drops the connection, which only happens at teardown. Before the fixture
// (e2e/fixtures/test.ts) removed the handlers first, this test failed every time with the reset; with it, never.
import { expect, test } from "../fixtures/test";

test.describe("teardown", () => {
  test("a route handler that is still inside route.fetch() when the test ends does not fail the test", async ({ page, harness }) => {
    let inside = false;
    await page.route("**/api/v1/events", async (route) => {
      inside = true;
      const response = await route.fetch();
      await route.fulfill({ response });
    });
    await page.goto(`${harness.url}/swimlanes`);
    // The page opened its stream and the handler is waiting for an answer that only the harness closing the connection can end: the harness
    // holds the stream open (it is polled, because the handler is entered a moment before its request reaches the harness).
    await expect.poll(() => inside).toBe(true);
    await expect.poll(() => harness.streamCount()).toBe(1);
  });
});
