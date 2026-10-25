// What the tests of the T5 Admin sections share: opening a section on the whole app with the T5 world. The clock, the row finder and
// the held response are the Admin harness's. Test code only: nothing outside the tests imports it.
import { screen } from "@testing-library/react";
import { renderApp } from "../../../test/app.tsx";
import { FIRST } from "../admin/harness.ts";
import { t5Api, type T5World } from "./world.ts";

export { FIRST, holdUntil, installAdminClock, rowIn, writesOf } from "../admin/harness.ts";

export async function openSection(world: T5World, url: string, options: Parameters<typeof renderApp>[2] = {}) {
  const app = renderApp(url, t5Api(world), options);
  await screen.findByRole("heading", { level: 1, name: "Admin" }, FIRST);
  return app;
}
