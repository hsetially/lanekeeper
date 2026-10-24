// Things a spec does to the app shell that more than one spec does: sign out through the account menu, and read what the
// sidebar and the header show.
import type { Page } from "@playwright/test";

/** Sign out the way a person does: open the account menu at the foot of the sidebar and choose Sign out. */
export async function signOut(page: Page): Promise<void> {
  await page.getByRole("complementary", { name: "Primary" }).getByRole("button", { name: /Ada Lovelace|account menu/ }).click();
  await page.getByRole("menuitem", { name: "Sign out" }).click();
}
